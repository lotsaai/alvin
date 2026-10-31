//! Directory walk and head reads over omp's sessions directory.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::header::{SessionHead, parse};
use crate::{Error, Result};

/// omp reads the first 4 KiB of a session file for its header; so does the index.
const HEAD_BYTES: u64 = 4096;

fn entries(dir: &Path) -> Result<Vec<(PathBuf, std::fs::FileType)>> {
  let read_dir = |source| Error::ReadDir {
    path: dir.to_path_buf(),
    source,
  };
  std::fs::read_dir(dir)
    .map_err(read_dir)?
    .map(|entry| {
      let entry = entry.map_err(read_dir)?;
      let file_type = entry.file_type().map_err(read_dir)?;
      Ok((entry.path(), file_type))
    })
    .collect()
}

fn read_head(path: &Path) -> Result<SessionHead> {
  let mut head = Vec::new();
  File::open(path)
    .and_then(|file| file.take(HEAD_BYTES).read_to_end(&mut head))
    .map_err(|source| Error::ReadFile {
      path: path.to_path_buf(),
      source,
    })?;
  parse(path, &head)
}

/// Reads the head of every session file under `dir`, which has omp's layout `<dir>/<encoded cwd>/<name>.jsonl`.
///
/// Only `*.jsonl` files are read; backups and other files are skipped. Each file reports its own result, so one
/// malformed file does not hide the rest. Results are ordered by path. Failing to list a directory fails the scan.
pub fn scan(dir: &Path) -> Result<Vec<Result<SessionHead>>> {
  let mut files = Vec::new();
  for (project_dir, file_type) in entries(dir)? {
    if !file_type.is_dir() {
      continue;
    }
    for (path, file_type) in entries(&project_dir)? {
      if file_type.is_file() && path.extension().is_some_and(|extension| extension == "jsonl") {
        files.push(path);
      }
    }
  }
  files.sort();
  Ok(files.iter().map(|path| read_head(path)).collect())
}

#[cfg(test)]
mod tests {
  use std::io::Write;

  use alvin_protocol::SessionId;

  use super::scan;
  use crate::Error;
  use crate::test_support::{TempDir, run_omp};

  /// A real omp session for a fresh project; returns (sessions root, project cwd, session file).
  fn real_session(tmp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = tmp.path().join("sessions");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let file = run_omp(&project, &root.join("-project"), "Reply with the single word ok.");
    (root, project, file)
  }

  #[test]
  fn reads_the_header_of_a_session_omp_wrote() {
    let tmp = TempDir::new();
    let (root, project, file) = real_session(&tmp);

    let heads = scan(&root).unwrap();

    let head = heads.into_iter().next().unwrap().unwrap();
    let name = file.file_name().unwrap().to_str().unwrap();
    assert!(
      name.ends_with(&format!("_{}.jsonl", head.id.as_str())),
      "file {name} does not carry id {:?}",
      head.id
    );
    // omp records the cwd as the OS reported it, which may be a symlinked spelling (`/var` for `/private/var`).
    assert_eq!(head.cwd.canonicalize().unwrap(), project);
    assert_eq!(head.path, file);
    assert_eq!(head.title, None);
  }

  #[test]
  fn a_torn_last_line_does_not_fail_the_scan() {
    let tmp = TempDir::new();
    let (root, _project, file) = real_session(&tmp);
    std::fs::OpenOptions::new()
      .append(true)
      .open(&file)
      .unwrap()
      .write_all(br#"{"type":"message","id":"cut"#)
      .unwrap();

    let heads = scan(&root).unwrap();

    assert_eq!(heads.len(), 1);
    assert!(heads[0].is_ok(), "{:?}", heads[0]);
  }

  #[test]
  fn bytes_past_the_head_window_are_never_decoded() {
    let tmp = TempDir::new();
    let (root, _project, file) = real_session(&tmp);
    // Not JSON and not newline-terminated: decoding any of it would fail the file.
    std::fs::OpenOptions::new()
      .append(true)
      .open(&file)
      .unwrap()
      .write_all(&vec![0xff; 16 * 1024 * 1024])
      .unwrap();

    let heads = scan(&root).unwrap();

    assert!(heads[0].is_ok(), "{:?}", heads[0]);
  }

  #[test]
  fn only_jsonl_files_are_read() {
    let tmp = TempDir::new();
    let (root, _project, file) = real_session(&tmp);
    let dir = file.parent().unwrap();
    let backup = dir.join(format!("{}.abc123.bak", file.file_name().unwrap().to_str().unwrap()));
    std::fs::copy(&file, backup).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a session").unwrap();
    std::fs::write(root.join("stray.jsonl"), "not in a project directory").unwrap();

    let heads = scan(&root).unwrap();

    assert_eq!(heads.len(), 1);
  }

  #[test]
  fn a_malformed_file_is_an_error_for_that_file_only() {
    let tmp = TempDir::new();
    let (root, _project, file) = real_session(&tmp);
    let bad = file.parent().unwrap().join("2000-01-01T00-00-00-000Z_bad.jsonl");
    std::fs::write(&bad, "this is not json\n").unwrap();

    let heads = scan(&root).unwrap();

    assert_eq!(heads.len(), 2);
    assert!(heads.iter().any(|head| head.is_ok()));
    let error = heads.iter().find_map(|head| head.as_ref().err()).unwrap();
    assert!(
      matches!(error, Error::DecodeRecord { path, offset: 0, .. } if *path == bad),
      "{error:?}"
    );
  }

  #[test]
  fn a_missing_sessions_directory_fails_the_scan_with_its_path() {
    let tmp = TempDir::new();
    let missing = tmp.path().join("nope");

    let error = scan(&missing).unwrap_err();

    assert!(matches!(error, Error::ReadDir { path, .. } if path == missing));
  }

  #[test]
  fn session_ids_are_whatever_string_omp_wrote() {
    // omp's header id is a plain string; a non-UUID id must not fail the file.
    let tmp = TempDir::new();
    let dir = tmp.path().join("sessions/-p");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
      dir.join("2026-01-01T00-00-00-000Z_x.jsonl"),
      "{\"type\":\"session\",\"version\":3,\"id\":\"not-a-uuid\",\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"cwd\":\"/p\"}\n",
    )
    .unwrap();

    let heads = scan(&tmp.path().join("sessions")).unwrap();

    assert_eq!(heads[0].as_ref().unwrap().id, SessionId::from("not-a-uuid".to_owned()));
  }
}
