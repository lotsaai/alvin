//! Helpers for the app's startup test: a real git repository and a gateway config that starts real omp in isolation.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use alvin_gateway::{GatewayConfig, HostConfig};
use alvin_protocol::EventId;
use gpui_kit::TestAppContext;

/// The model the test runs against.
const MODEL: &str = "zai/glm-5.3-flash";

/// How long a test waits for omp before failing.
const PATIENCE: Duration = Duration::from_secs(120);

/// The records of one real omp run, used only so the temporary repository is a listed project.
const SEED_RECORDS: &str = include_str!("../fixtures/seed-session.jsonl");
const SEED_CWD: &str = "/tmp/alvin-probe.hnBA/proj";

/// A temporary git repository with its own omp sessions directory. omp runs with a throwaway `HOME`; it still reads the
/// machine's agent configuration so model credentials resolve, and nothing here opens or prints anything in it.
pub(crate) struct Fixture {
  base: PathBuf,
  sessions: PathBuf,
  home: PathBuf,
  session_dir: PathBuf,
}

impl Fixture {
  pub(crate) fn new() -> Self {
    let base = std::env::temp_dir().join(format!("alvin-app-{}", EventId::new().as_uuid()));
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let repo = base.join("repo");
    let sessions = base.join("sessions");
    let home = base.join("home");
    let session_dir = sessions.join("-repo");
    for dir in [&repo, &session_dir, &home] {
      std::fs::create_dir_all(dir).unwrap();
    }
    let git = Command::new("git").args(["init", "--quiet"]).current_dir(&repo).output().unwrap();
    assert!(git.status.success(), "{}", String::from_utf8_lossy(&git.stderr));
    let seed = SEED_RECORDS.replace(SEED_CWD, repo.to_str().unwrap());
    std::fs::write(session_dir.join("2026-01-01T00-00-00-000Z_seed.jsonl"), seed).unwrap();
    Self {
      base,
      sessions,
      home,
      session_dir,
    }
  }

  /// A gateway configuration over the repository's sessions that starts real omp in isolation.
  pub(crate) fn config(&self) -> GatewayConfig {
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".omp/agent"));
    let args = [
      "--no-tools",
      "--no-skills",
      "--no-rules",
      "--no-extensions",
      "--no-context-files",
      "--session-dir",
    ]
    .map(OsString::from)
    .into_iter()
    .chain([self.session_dir.clone().into_os_string()])
    .collect();
    let omp = HostConfig::builder()
      .omp(PathBuf::from("omp"))
      .extra_args(args)
      .env(vec![
        (OsString::from("HOME"), self.home.clone().into_os_string()),
        (OsString::from("PI_CODING_AGENT_DIR"), agent_dir.into_os_string()),
      ])
      .build();
    GatewayConfig::builder()
      .sessions_dir(self.sessions.clone())
      .omp(omp)
      .default_model(Some(MODEL.to_owned()))
      .build()
  }
}

impl Drop for Fixture {
  fn drop(&mut self) {
    // Best effort: a leftover temporary directory must not fail an otherwise passing test.
    drop(std::fs::remove_dir_all(&self.base));
  }
}

/// Whether process `pid` exists.
pub(crate) fn alive(pid: u32) -> bool {
  Command::new("kill")
    .args(["-0", &pid.to_string()])
    .output()
    .unwrap()
    .status
    .success()
}

/// Runs the foreground executor until `done` holds. omp and the gateway run on tokio threads, so progress arrives in real
/// time: this polls rather than advancing GPUI's test clock.
pub(crate) fn until(cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) {
  let deadline = Instant::now() + PATIENCE;
  loop {
    cx.run_until_parked();
    if done(cx) {
      return;
    }
    assert!(Instant::now() < deadline, "timed out waiting for {what}");
    std::thread::sleep(Duration::from_millis(25));
  }
}
