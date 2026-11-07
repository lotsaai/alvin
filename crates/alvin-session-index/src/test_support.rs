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
    if let Err(error) = std::fs::remove_dir_all(&self.0) {
      if std::thread::panicking() {
        eprintln!("could not remove {}: {error}", self.0.display());
      } else {
        panic!("could not remove {}: {error}", self.0.display());
      }
    }
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
/// Each invocation uses a temporary home and agent directory, separate from the supplied session directory.
pub fn run_omp(cwd: &Path, session_dir: &Path, prompt: &str) -> PathBuf {
  let home = TempDir::new();
  let paths: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
    .filter(|path| path.is_absolute())
    .collect();
  let executable = |name: &str| paths.iter().map(|path| path.join(name)).find(|path| path.is_file());
  let omp = executable("omp").expect("omp not found in PATH");
  let mut safe_path = vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
  for name in ["bun", "node"] {
    if let Some(path) = executable(name) {
      safe_path.insert(0, path.parent().unwrap().to_path_buf());
    }
  }
  let mut command = Command::new(omp);
  command
    .env_clear()
    .env("HOME", home.path())
    .env("PI_CODING_AGENT_DIR", home.path().join("agent"))
    .env("PATH", std::env::join_paths(safe_path).unwrap());
  command.env(
    "OPENROUTER_API_KEY",
    std::env::var_os("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY is required"),
  );
  let output = command
    .args([
      "-p",
      prompt,
      "--model",
      "openrouter/inclusionai/ling-3.0-flash",
      "--no-tools",
      "--no-skills",
      "--no-rules",
      "--no-extensions",
    ])
    .args(["--no-context-files", "--no-title", "--session-dir"])
    .arg(session_dir)
    .current_dir(cwd)
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
