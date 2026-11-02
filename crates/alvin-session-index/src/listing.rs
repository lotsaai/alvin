//! One page of a project's sessions, newest first.

use std::cmp::Reverse;
use std::fs::File;
use std::io::{BufRead, BufReader};

use alvin_protocol::{SessionCursor, SessionPage, SessionStatus, SessionSummary};

use crate::header::{SessionHead, first_user_title, has_assistant_marker};
use crate::status::{is_assistant_message, read_tail, status_of};
use crate::{Error, Result};

/// The sessions of `heads` after `after`, newest first, at most `limit` of them.
///
/// Order is by modification time, then id, so a [`SessionCursor`] names an exact position and paging returns every
/// session exactly once. Only files whose turn comes are read beyond their head window. If the head has no title,
/// the first user message is sought in the rest of the file. Sessions with no title, a `Pending` or `Unknown`
/// status, and no assistant turn anywhere in the file are skipped. They do not count toward `limit`.
pub fn list_page(mut heads: Vec<SessionHead>, after: Option<&SessionCursor>, limit: usize) -> Result<SessionPage> {
  heads.sort_by(|a, b| (Reverse(a.modified), Reverse(&a.id)).cmp(&(Reverse(b.modified), Reverse(&b.id))));
  let remaining = heads
    .into_iter()
    .skip_while(|head| after.is_some_and(|cursor| (head.modified, &head.id) >= (cursor.modified, &cursor.id)));

  let mut sessions = Vec::new();
  let mut more = false;
  for mut head in remaining {
    let (status, assistant_in_tail) = tail_status(&head)?;
    if head.title.is_none() {
      let file = File::open(&head.path).map_err(|source| read_file(&head, source))?;
      head.title = first_user_title(&head.path, BufReader::new(file))?;
    }
    if head.title.is_none()
      && matches!(status, SessionStatus::Pending | SessionStatus::Unknown)
      && !head.assistant_in_head
      && !assistant_in_tail
      && !has_assistant_turn(&head)?
    {
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

/// The status from the tail window, and whether that window shows an assistant message.
fn tail_status(head: &SessionHead) -> Result<(SessionStatus, bool)> {
  let mut file = File::open(&head.path).map_err(|source| read_file(head, source))?;
  let tail = read_tail(&mut file).map_err(|source| read_file(head, source))?;
  Ok((status_of(&tail), has_assistant_marker(&tail)))
}

/// Whether any record of the whole file is an assistant message. This is the one read past the windows: omp makes
/// it when neither window shows an assistant turn, so that a long transcript is never mistaken for a stub.
fn has_assistant_turn(head: &SessionHead) -> Result<bool> {
  let file = File::open(&head.path).map_err(|source| read_file(head, source))?;
  for line in BufReader::new(file).split(b'\n') {
    let line = line.map_err(|source| read_file(head, source))?;
    if is_assistant_message(&line) {
      return Ok(true);
    }
  }
  Ok(false)
}

fn read_file(head: &SessionHead, source: std::io::Error) -> Error {
  Error::ReadFile {
    path: head.path.clone(),
    source,
  }
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

  #[test]
  fn a_session_whose_only_assistant_turn_is_outside_both_windows_is_listed_and_a_stub_is_not() {
    let tmp = TempDir::new();
    let dir = real_project(&tmp, 1);
    let real = std::fs::read_to_string(
      std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| !p.to_string_lossy().contains("empty"))
        .unwrap(),
    )
    .unwrap();
    let lines: Vec<&str> = real.lines().collect();
    let assistant = lines.iter().find(|line| line.contains(r#""role":"assistant""#)).unwrap();
    let filler = format!(r#"{{"type":"custom","customType":"filler","data":"{}"}}"#, "x".repeat(1000));
    let fillers = |count: usize| vec![filler.as_str(); count].join("\n");
    let header = lines[..2].join("\n").replace("session-", "x-");
    // The head window ends inside the first filler records, the tail window starts after the assistant record.
    let long = format!("{header}\n{}\n{assistant}\n{}\n", fillers(6), fillers(40));
    let long = long.replacen(r#""id":""#, r#""id":"long-"#, 1);
    std::fs::write(dir.join("2000-01-02T00-00-00-000Z_long.jsonl"), long).unwrap();
    let stub = format!("{header}\n{}\n", fillers(46)).replacen(r#""id":""#, r#""id":"stub-"#, 1);
    std::fs::write(dir.join("2000-01-03T00-00-00-000Z_stub.jsonl"), stub).unwrap();

    let page = list_page(heads(&dir), None, 10).unwrap();

    let ids: Vec<_> = page.sessions.iter().map(|session| session.id.as_str().to_owned()).collect();
    assert!(ids.iter().any(|id| id.starts_with("long-")), "{ids:?}");
    assert!(!ids.iter().any(|id| id.starts_with("stub-") || id.starts_with("empty-")), "{ids:?}");
    let long = page
      .sessions
      .iter()
      .find(|session| session.id.as_str().starts_with("long-"))
      .unwrap();
    assert_eq!(long.status, SessionStatus::Unknown);
  }
}
