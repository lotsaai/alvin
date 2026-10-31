//! Helpers for headless UI tests: a real git repository, a real embedded gateway, and real omp.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use alvin_client::{ChannelPair, Client, Endpoint};
use alvin_gateway::{Gateway, GatewayConfig};
use alvin_omp_host::HostConfig;
use alvin_protocol::EventId;
use gpui_kit::TestAppContext;

/// The model every test runs against.
pub(crate) const MODEL: &str = "zai/glm-5.3-flash";

/// How long a test waits for omp before failing.
const PATIENCE: Duration = Duration::from_secs(120);

/// The records of one real omp run, used only so the temporary repository is a listed project.
const SEED_RECORDS: &str = include_str!("../fixtures/seed-session.jsonl");
const SEED_CWD: &str = "/tmp/alvin-probe.hnBA/proj";

/// A temporary git repository with its own omp sessions directory and an embedded gateway over it. omp runs with a
/// throwaway `HOME`, so it touches nothing under the real home; it still reads the machine's agent configuration so model
/// credentials resolve, and nothing here opens or prints anything in it.
pub(crate) struct Fixture {
  base: PathBuf,
  repo: PathBuf,
  sessions: PathBuf,
  home: PathBuf,
  agent_dir: PathBuf,
  pub(crate) gateway: Gateway,
}

impl Fixture {
  pub(crate) fn new() -> Self {
    Self::with_tools(&["--no-tools"])
  }

  /// A fixture whose omp runs with these tool flags: `--no-tools`, or `--tools ask` for a tool that opens a dialog.
  pub(crate) fn with_tools(tools: &[&str]) -> Self {
    let base = std::env::temp_dir().join(format!("alvin-ui-{}", EventId::new().as_uuid()));
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

    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".omp/agent"));
    let args = tools
      .iter()
      .copied()
      .chain([
        "--no-skills",
        "--no-rules",
        "--no-extensions",
        "--no-context-files",
        "--session-dir",
      ])
      .map(OsString::from)
      .chain([session_dir.into_os_string()])
      .collect();
    let omp = HostConfig::builder()
      .omp(PathBuf::from("omp"))
      .extra_args(args)
      .env(vec![
        (OsString::from("HOME"), home.clone().into_os_string()),
        (OsString::from("PI_CODING_AGENT_DIR"), agent_dir.clone().into_os_string()),
      ])
      .build();
    let gateway = Gateway::new(
      GatewayConfig::builder()
        .sessions_dir(sessions.clone())
        .omp(omp)
        .default_model(Some(MODEL.to_owned()))
        .build(),
    );
    Self {
      base,
      repo,
      sessions,
      home,
      agent_dir,
      gateway,
    }
  }

  /// A client over the gateway's in-process channel. Call inside a tokio runtime.
  pub(crate) async fn client(&self) -> Client {
    Client::connect(Endpoint::Channel(ChannelPair::from(self.gateway.connect_channel(16).unwrap())))
      .await
      .unwrap()
  }

  /// Has real omp write one session in the repository, as an earlier run of the app would have: one prompt, one reply.
  pub(crate) fn write_session(&self) {
    let output = Command::new("omp")
      .args(["-p", "Reply with the single word ok.", "--model", MODEL])
      .args([
        "--no-tools",
        "--no-skills",
        "--no-rules",
        "--no-extensions",
        "--no-context-files",
        "--no-title",
        "--session-dir",
      ])
      .arg(self.sessions.join("-repo"))
      .current_dir(&self.repo)
      .env("HOME", &self.home)
      .env("PI_CODING_AGENT_DIR", &self.agent_dir)
      .output()
      .unwrap();
    assert!(output.status.success(), "omp failed: {}", String::from_utf8_lossy(&output.stderr));
  }

  /// Appends `pairs` user and assistant messages of `bytes` of text each to the session `write_session` made, enough for omp to
  /// compact it: omp refuses a session too small to be worth summarizing.
  pub(crate) fn grow_session(&self, pairs: usize, bytes: usize) {
    let dir = self.sessions.join("-repo");
    let file = std::fs::read_dir(&dir)
      .unwrap()
      .map(|entry| entry.unwrap().path())
      .find(|path| {
        path.extension().is_some_and(|extension| extension == "jsonl")
          && path.file_name().is_some_and(|name| !name.to_string_lossy().contains("seed"))
      })
      .expect("write_session wrote a session file");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut records: Vec<String> = text.lines().map(str::to_owned).collect();
    let entries: Vec<serde_json::Value> = records
      .iter()
      .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
      .filter(|value| value.get("parentId").is_some())
      .collect();
    let role = |wanted: &str| {
      entries
        .iter()
        .find(|entry| entry["message"]["role"].as_str() == Some(wanted))
        .unwrap_or_else(|| panic!("the turn wrote a {wanted} message"))
        .clone()
    };
    let (user, assistant) = (role("user"), role("assistant"));
    let mut parent = entries.last().and_then(|entry| entry["id"].as_str()).expect("entries").to_owned();
    let filler = "lorem ipsum dolor sit amet ".repeat(bytes / 27 + 1);
    for pair in 0..pairs {
      for (template, turn) in [(&user, 0), (&assistant, 1)] {
        let mut entry = template.clone();
        let id = format!("grown{:04}", pair * 2 + turn);
        entry["id"] = serde_json::Value::String(id.clone());
        entry["parentId"] = serde_json::Value::String(parent.clone());
        entry["message"]["content"] = serde_json::json!([{ "type": "text", "text": filler }]);
        records.push(entry.to_string());
        parent = id;
      }
    }
    records.push(String::new());
    std::fs::write(&file, records.join("\n")).unwrap();
  }

  /// Closes every live session and removes the temporary directory.
  pub(crate) async fn finish(self) {
    self.gateway.shutdown().await.unwrap();
    // Best effort: a leftover temporary directory must not fail an otherwise passing test.
    drop(std::fs::remove_dir_all(&self.base));
  }
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
