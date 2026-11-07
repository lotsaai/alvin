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

/// A temporary git repository with isolated omp configuration and sessions.
pub(crate) struct Fixture {
  base: PathBuf,
  sessions: PathBuf,
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
    let server = match self.server.take() {
      Some(server) => server.shutdown().await,
      None => Ok(()),
    };
    let gateway = self.gateway.shutdown().await;
    server.unwrap();
    gateway.unwrap();
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
    let session_dir = sessions.join("-repo");
    let home = base.join("home");
    let agent = base.join("agent");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&agent).unwrap();
    for dir in [&repo, &session_dir] {
      std::fs::create_dir_all(dir).unwrap();
    }
    let git = Command::new("git").args(["init", "--quiet"]).current_dir(&repo).output().unwrap();
    assert!(git.status.success(), "{}", String::from_utf8_lossy(&git.stderr));
    let seed = SEED_RECORDS.replace(SEED_CWD, repo.to_str().unwrap());
    std::fs::write(session_dir.join("2026-01-01T00-00-00-000Z_seed.jsonl"), seed).unwrap();
    Self {
      base,
      sessions,
      session_dir,
    }
  }

  /// A gateway configuration over the repository's sessions that starts real omp in isolation.
  pub(crate) fn config(&self) -> GatewayConfig {
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
    let paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
      .map(|dir| {
        if dir.is_absolute() {
          dir
        } else {
          std::env::current_dir().unwrap().join(dir)
        }
      })
      .collect();
    let find = |name: &str| paths.iter().map(|dir| dir.join(name)).find(|path| path.is_file());
    let omp_path = find("omp").expect("omp must be on PATH");
    let bun_path = find("bun");
    let mut runtime_paths = vec![omp_path.parent().unwrap(), Path::new("/usr/bin"), Path::new("/bin")];
    if let Some(bun) = &bun_path {
      runtime_paths.insert(1, bun.parent().unwrap());
    }
    let path = std::env::join_paths(runtime_paths).unwrap();
    let env = vec![
      ("HOME".into(), self.base.join("home").into_os_string()),
      ("PI_CODING_AGENT_DIR".into(), self.base.join("agent").into_os_string()),
      ("PATH".into(), path),
      (
        "OPENROUTER_API_KEY".into(),
        std::env::var_os("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY"),
      ),
    ];
    let omp = HostConfig::builder()
      .omp(omp_path)
      .extra_args(args)
      .clear_env(true)
      .env(env)
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
    if let Err(error) = std::fs::remove_dir_all(&self.base) {
      if std::thread::panicking() {
        eprintln!("could not remove {}: {error}", self.base.display());
      } else {
        panic!("could not remove {}: {error}", self.base.display());
      }
    }
  }
}
