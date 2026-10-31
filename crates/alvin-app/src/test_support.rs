//! Helpers for the app's startup test: a real git repository and a gateway config that starts real omp in isolation.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use alvin_gateway::{GatewayConfig, HostConfig};
use alvin_protocol::EventId;

/// The model the test runs against.
const MODEL: &str = "openrouter/inclusionai/ling-3.0-flash";

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
    // omp gets the fixture's own home and agent directory, and the model key from the environment the tests run in.
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
      .env(
        [
          (OsString::from("HOME"), self.home.clone().into_os_string()),
          (OsString::from("PI_CODING_AGENT_DIR"), self.home.join("agent").into_os_string()),
        ]
        .into_iter()
        .chain([(
          OsString::from("OPENROUTER_API_KEY"),
          std::env::var_os("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY is set in the environment the tests run in"),
        )])
        .collect(),
      )
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

/// A shell command that reports its pid through the fifo at `fifo`, then becomes `/bin/sleep 600` still holding the
/// fifo's write end, so reading the fifo reaches end-of-file exactly when the process is gone.
pub(crate) fn sleeper(fifo: &Path) -> String {
  format!("/bin/sh -c 'exec 3> {}; echo $$ >&3; exec /bin/sleep 600'", fifo.display())
}

impl Fixture {
  /// Creates the fifo a [`sleeper`] reports through.
  pub(crate) fn make_fifo(&self) -> PathBuf {
    let fifo = self.base.join("child.fifo");
    let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(status.success());
    fifo
  }
}

/// A [`sleeper`] that has reported in.
pub(crate) struct Child {
  reader: BufReader<File>,
}

impl Child {
  /// Waits for the sleeper to report through `fifo`. The fifo is read with blocking calls on the blocking pool: kqueue
  /// does not report end-of-file on a fifo, so tokio's pipe types cannot tell when the writer died.
  pub(crate) async fn report(fifo: &Path) -> Self {
    let fifo = fifo.to_owned();
    tokio::task::spawn_blocking(move || {
      // Opening the read end blocks until the sleeper opens the write end.
      let mut reader = BufReader::new(File::open(fifo).unwrap());
      let mut line = String::new();
      reader.read_line(&mut line).unwrap();
      Self { reader }
    })
    .await
    .unwrap()
  }

  /// Resolves once the sleeper has died: its write end of the fifo closes with it.
  pub(crate) async fn died(mut self) {
    tokio::task::spawn_blocking(move || {
      let mut rest = Vec::new();
      self.reader.read_to_end(&mut rest).unwrap();
    })
    .await
    .unwrap();
  }
}
