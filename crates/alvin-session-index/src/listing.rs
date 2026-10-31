//! One page of a project's sessions, newest first.

use std::cmp::Reverse;
use std::fs::File;

use alvin_protocol::{SessionCursor, SessionPage, SessionStatus, SessionSummary};

use crate::header::SessionHead;
use crate::status::{read_tail, status_of};
use crate::{Error, Result};

/// The sessions of `heads` after `after`, newest first, at most `limit` of them.
///
/// Order is by modification time, then id, so a [`SessionCursor`] names an exact position and paging returns every
/// session exactly once. Only the tail of a file whose turn comes is read: pages before it never touch it.
/// Sessions omp would hide from its own picker are skipped: no title and nothing to classify (`Pending` or
/// `Unknown`). They do not count toward `limit`.
pub fn list_page(mut heads: Vec<SessionHead>, after: Option<&SessionCursor>, limit: usize) -> Result<SessionPage> {
  heads.sort_by(|a, b| (Reverse(a.modified), Reverse(&a.id)).cmp(&(Reverse(b.modified), Reverse(&b.id))));
  let remaining = heads
    .into_iter()
    .skip_while(|head| after.is_some_and(|cursor| (head.modified, &head.id) >= (cursor.modified, &cursor.id)));

  let mut sessions = Vec::new();
  let mut more = false;
  for head in remaining {
    let status = tail_status(&head)?;
    if head.title.is_none() && matches!(status, SessionStatus::Pending | SessionStatus::Unknown) {
      continue;
    }
    if sessions.len() == limit {
      more = true;
      break;
    }
    sessions.push(SessionSummary {
      id: head.id,
      path: head.path,
      title: head.title,
      created: head.created,
      modified: head.modified,
      status,
      parent: head.parent,
    });
  }
  let next = more.then(|| sessions.last()).flatten().map(|last| SessionCursor {
    modified: last.modified,
    id: last.id.clone(),
  });
  Ok(SessionPage { sessions, next })
}

fn tail_status(head: &SessionHead) -> Result<SessionStatus> {
  let read_file = |source| Error::ReadFile {
    path: head.path.clone(),
    source,
  };
  let mut file = File::open(&head.path).map_err(read_file)?;
  Ok(status_of(&read_tail(&mut file).map_err(read_file)?))
}

#[cfg(test)]
mod tests {
  use std::path::{Path, PathBuf};
  use std::time::{Duration, SystemTime};

  use alvin_protocol::{SessionId, SessionStatus};

  use super::list_page;
  use crate::scan;
  use crate::test_support::{TempDir, run_omp};

  /// Writes `count` real omp sessions into one project directory, session `n` modified `n` seconds after the
  /// first, plus an empty one (just the real header records) modified last. Returns the directory.
  fn real_project(tmp: &TempDir, count: usize) -> PathBuf {
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let dir = tmp.path().join("sessions/-project");
    std::fs::create_dir_all(&dir).unwrap();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let files: Vec<PathBuf> = std::thread::scope(|scope| {
      let runs: Vec<_> = (0..count)
        .map(|n| {
          let (project, root) = (&project, tmp.path().join(format!("run-{n}")));
          scope.spawn(move || run_omp(project, &root, &format!("Reply with the single word {n}.")))
        })
        .collect();
      runs.into_iter().map(|run| run.join().unwrap()).collect()
    });
    for (n, file) in files.iter().enumerate() {
      let target = dir.join(file.file_name().unwrap());
      std::fs::rename(file, &target).unwrap();
      set_modified(&target, base + Duration::from_secs(n as u64));
    }
    let real = std::fs::read_to_string(dir.join(files[0].file_name().unwrap())).unwrap();
    let header_only = real
      .lines()
      .take(2)
      .collect::<Vec<_>>()
      .join("\n")
      .replacen("\"id\":\"", "\"id\":\"empty-", 1);
    let empty = dir.join("2000-01-01T00-00-00-000Z_empty.jsonl");
    std::fs::write(&empty, format!("{header_only}\n")).unwrap();
    set_modified(&empty, base + Duration::from_secs(count as u64));
    dir
  }

  fn set_modified(path: &Path, at: SystemTime) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
  }

  fn heads(dir: &Path) -> Vec<crate::SessionHead> {
    scan(dir.parent().unwrap()).unwrap().into_iter().map(|head| head.unwrap()).collect()
  }

  #[test]
  fn paging_returns_every_real_session_once_newest_first_and_skips_empty_ones() {
    let tmp = TempDir::new();
    let dir = real_project(&tmp, 5);
    let heads = heads(&dir);
    let mut expected: Vec<_> = heads.iter().filter(|head| !head.id.as_str().starts_with("empty-")).collect();
    expected.sort_by_key(|head| std::cmp::Reverse(head.modified));
    let expected: Vec<SessionId> = expected.into_iter().map(|head| head.id.clone()).collect();

    let mut seen = Vec::new();
    let mut pages = Vec::new();
    let mut after = None;
    loop {
      let page = list_page(heads.clone(), after.as_ref(), 2).unwrap();
      pages.push(page.sessions.len());
      seen.extend(page.sessions.iter().map(|session| session.id.clone()));
      assert!(page.sessions.iter().all(|session| session.status == SessionStatus::Complete));
      match page.next {
        Some(next) => after = Some(next),
        None => break,
      }
    }

    assert_eq!(seen, expected);
    assert_eq!(pages, [2, 2, 1]);
  }

  #[test]
  fn a_page_that_ends_exactly_at_the_last_session_has_no_next_cursor() {
    let tmp = TempDir::new();
    let dir = real_project(&tmp, 2);

    let page = list_page(heads(&dir), None, 2).unwrap();

    assert_eq!(page.sessions.len(), 2);
    assert_eq!(page.next, None);
  }
}
