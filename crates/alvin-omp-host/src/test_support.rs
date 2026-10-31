//! Helpers for tests that start real omp.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use alvin_protocol::{AssistantMessageEvent, EventBody, LiveSessionId, PromptStatus, RpcAgentEvent};
use tokio::sync::mpsc::Receiver;

use crate::{Host, HostConfig, Session, StartRequest};

/// The model every test runs against.
pub const MODEL: &str = "zai/glm-5.3-flash";

/// How long a test waits for omp before failing it.
pub const PATIENCE: Duration = Duration::from_secs(120);

/// A uniquely named canonical temporary directory, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
  pub fn new() -> Self {
    let path = std::env::temp_dir().join(format!("alvin-omp-host-{}", LiveSessionId::new().as_uuid()));
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

/// A host over an isolated omp: `HOME` is a throwaway directory, so omp touches nothing under the real home (the sandbox
/// denies parts of it), while the machine's agent configuration stays reachable so model credentials resolve. This code
/// never opens or prints anything in it.
pub struct Fixture {
  pub host: Host,
  pub cwd: TempDir,
  sessions: TempDir,
  _home: TempDir,
}

impl Fixture {
  pub fn new() -> Self {
    Self::with_capacity(1024)
  }

  pub fn with_capacity(capacity: usize) -> Self {
    let home = TempDir::new();
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".omp/agent"));
    let config = HostConfig::builder()
      .omp(PathBuf::from("omp"))
      .extra_args(
        ["--no-tools", "--no-skills", "--no-rules", "--no-extensions", "--no-context-files"]
          .map(OsString::from)
          .to_vec(),
      )
      .env(vec![
        (OsString::from("HOME"), home.path().as_os_str().to_owned()),
        (OsString::from("PI_CODING_AGENT_DIR"), agent_dir.into_os_string()),
      ])
      .event_capacity(NonZeroUsize::new(capacity).unwrap())
      .build();
    Self {
      host: Host::new(config),
      cwd: TempDir::new(),
      sessions: TempDir::new(),
      _home: home,
    }
  }

  pub fn request(&self) -> StartRequest {
    StartRequest::builder()
      .cwd(self.cwd.path().to_path_buf())
      .model(MODEL.to_owned())
      .session_dir(self.sessions.path().to_path_buf())
      .build()
  }

  pub async fn start(&self) -> Session {
    self.host.start(self.request()).await.unwrap()
  }
}

/// Receives the next event, failing the test if omp stays silent for [`PATIENCE`].
pub async fn next(events: &mut Receiver<EventBody>) -> EventBody {
  tokio::time::timeout(PATIENCE, events.recv())
    .await
    .unwrap()
    .expect("the event stream ended")
}

/// Collects events up to and including the prompt result.
pub async fn until_prompt_result(events: &mut Receiver<EventBody>) -> Vec<EventBody> {
  let mut seen = Vec::new();
  loop {
    let event = next(events).await;
    let done = matches!(event, EventBody::PromptResult(_));
    seen.push(event);
    if done {
      return seen;
    }
  }
}

/// The text deltas of an assistant message, joined, and the text its `text_end` event reports.
pub fn streamed_text(events: &[EventBody]) -> (String, Option<String>) {
  let mut streamed = String::new();
  let mut ended = None;
  for event in events {
    if let EventBody::Agent(RpcAgentEvent::MessageUpdate(update)) = event {
      match &update.assistant_message_event {
        AssistantMessageEvent::TextDelta(delta) => streamed.push_str(delta.delta.as_deref().unwrap_or_default()),
        AssistantMessageEvent::TextEnd(end) => ended = end.content.clone(),
        _ => {}
      }
    }
  }
  (streamed, ended)
}

/// Whether the prompt result of `events` reports `status`.
pub fn prompt_status(events: &[EventBody]) -> Option<PromptStatus> {
  events.iter().find_map(|event| match event {
    EventBody::PromptResult(result) => Some(result.status),
    _ => None,
  })
}

/// Whether process `pid` exists.
pub fn alive(pid: u32) -> bool {
  Command::new("kill")
    .args(["-0", &pid.to_string()])
    .output()
    .unwrap()
    .status
    .success()
}

/// Waits until `pid` is gone, failing the test after ten seconds.
pub async fn gone(pid: u32) {
  for _ in 0..200 {
    if !alive(pid) {
      return;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  panic!("process {pid} is still running");
}

/// A shell command that writes its own pid to `file`, then becomes `/bin/sleep 600`. omp runs commands in its own shell
/// where `sleep` is a builtin without a process and `$!` is empty, so the child reports its pid itself; the sandbox
/// denies listing processes.
pub fn sleeper(file: &Path) -> String {
  format!("/bin/sh -c 'echo $$ > {}; exec /bin/sleep 600'", file.display())
}

/// Waits for `file` to hold a pid and returns it, failing the test after ten seconds.
pub async fn pid_in(file: &Path) -> u32 {
  for _ in 0..200 {
    if let Some(pid) = std::fs::read_to_string(file).ok().and_then(|text| text.trim().parse().ok()) {
      return pid;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  panic!("{} never received a pid", file.display());
}
