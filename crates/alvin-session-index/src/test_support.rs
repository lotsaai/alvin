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

/// Runs real `omp` once in `cwd` and returns the session file it wrote under `session_dir`.
///
/// The run is isolated: `HOME` is a throwaway directory, so omp reads and writes nothing under the real home
/// (the sandbox denies parts of it, and tests must not touch it). omp still reads the machine's agent
/// configuration, so its model credentials resolve; this code never opens or prints anything in it.
pub fn run_omp(cwd: &Path, session_dir: &Path, prompt: &str) -> PathBuf {
  let home = TempDir::new();
  let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".omp/agent"));
  let output = Command::new("omp")
    .args([
      "-p",
      prompt,
      "--model",
      "zai/glm-5.3-flash",
      "--no-tools",
      "--no-skills",
      "--no-rules",
      "--no-extensions",
    ])
    .args(["--no-context-files", "--no-title", "--session-dir"])
    .arg(session_dir)
    .current_dir(cwd)
    .env("HOME", home.path())
    .env("PI_CODING_AGENT_DIR", agent_dir)
    .output()
    .unwrap();
  assert!(
    output.status.success(),
    "omp failed in {}: {}",
    cwd.display(),
    String::from_utf8_lossy(&output.stderr)
  );
  let mut files: Vec<PathBuf> = std::fs::read_dir(session_dir)
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
    .collect();
  files.sort();
  files.pop().unwrap()
}
