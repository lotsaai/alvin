//! Helpers for tests that need real directories and real `git` and `jj` binaries.

use std::path::{Path, PathBuf};
use std::process::Command;

use alvin_protocol::EventId;

/// A uniquely named canonical temporary directory, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
  pub fn new() -> Self {
    let path = std::env::temp_dir().join(format!("alvin-session-index-{}", EventId::new().as_uuid()));
    std::fs::create_dir_all(&path).unwrap();
    Self(path.canonicalize().unwrap())
  }

  pub fn path(&self) -> &Path {
    &self.0
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    // Best effort: a leftover temp directory must not fail an otherwise passing test.
    drop(std::fs::remove_dir_all(&self.0));
  }
}

/// Runs a real binary in `dir` with an isolated identity, panicking with its stderr on failure.
pub fn run(dir: &Path, program: &str, args: &[&str]) {
  let output = Command::new(program)
    .args(args)
    .current_dir(dir)
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_NAME", "alvin")
    .env("GIT_AUTHOR_EMAIL", "alvin@example.invalid")
    .env("GIT_COMMITTER_NAME", "alvin")
    .env("GIT_COMMITTER_EMAIL", "alvin@example.invalid")
    .env("JJ_USER", "alvin")
    .env("JJ_EMAIL", "alvin@example.invalid")
    .output()
    .unwrap();
  assert!(
    output.status.success(),
    "{program} {args:?} failed in {}: {}",
    dir.display(),
    String::from_utf8_lossy(&output.stderr)
  );
}
