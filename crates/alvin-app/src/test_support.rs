//! Helpers for the app's startup test: a real git repository and a gateway config that starts real omp in isolation.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use alvin_gateway::{Gateway, GatewayConfig, HostConfig, WsServer};
use alvin_protocol::EventId;
use test_context::AsyncTestContext;
use url::Url;

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

pub(crate) struct ServerFixture {
  pub(crate) project: Fixture,
  pub(crate) gateway: Gateway,
  pub(crate) server: Option<WsServer>,
  pub(crate) endpoint: Url,
}

impl AsyncTestContext for ServerFixture {
  async fn setup() -> Self {
    let project = Fixture::new();
    let gateway = Gateway::new(project.config());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = gateway.serve_ws(listener).await.unwrap();
    let endpoint = format!("ws://{}", server.local_addr()).parse().unwrap();
    Self {
      project,
      gateway,
      server: Some(server),
      endpoint,
    }
  }

  async fn teardown(mut self) {
    if let Some(server) = self.server.take() {
      server.shutdown().await.unwrap();
    }
    self.gateway.shutdown().await.unwrap();
  }
}

impl Fixture {
  pub(crate) fn base(&self) -> &Path {
    &self.base
  }

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
