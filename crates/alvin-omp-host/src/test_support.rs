//! Helpers for tests that start real omp.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;

use alvin_protocol::{AssistantMessageEvent, EventBody, LiveSessionId, PromptStatus, RpcAgentEvent};
use tokio::sync::mpsc::Receiver;

use crate::{Host, HostConfig, Session, StartRequest};

/// The model every test runs against.
pub const MODEL: &str = "openrouter/inclusionai/ling-3.0-flash";

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
    if let Err(error) = std::fs::remove_dir_all(&self.0) {
      if std::thread::panicking() {
        eprintln!("could not remove {}: {error}", self.0.display());
      } else {
        panic!("could not remove {}: {error}", self.0.display());
      }
    }
  }
}

/// A host with isolated sessions and the machine's normal omp configuration.
pub struct Fixture {
  pub host: Host,
  pub cwd: TempDir,
  sessions: TempDir,
}

impl Fixture {
  pub fn new() -> Self {
    Self::with_capacity(1024)
  }

  pub fn with_capacity(capacity: usize) -> Self {
    let config = HostConfig::builder()
      .omp(PathBuf::from("omp"))
      .extra_args(
        ["--no-tools", "--no-skills", "--no-rules", "--no-extensions", "--no-context-files"]
          .map(OsString::from)
          .to_vec(),
      )
      .event_capacity(NonZeroUsize::new(capacity).unwrap())
      .build();
    Self {
      host: Host::new(config),
      cwd: TempDir::new(),
      sessions: TempDir::new(),
    }
  }

  pub fn request(&self) -> StartRequest {
    StartRequest::builder()
      .cwd(self.cwd.path().to_path_buf())
      .model(Some(MODEL.to_owned()))
      .session_dir(Some(self.sessions.path().to_path_buf()))
      .build()
  }

  pub async fn start(&self) -> (Session, Receiver<EventBody>) {
    self.host.start(self.request()).await.unwrap()
  }
}

/// Receives the next event, failing the test if the stream ended.
pub async fn next(events: &mut Receiver<EventBody>) -> EventBody {
  events.recv().await.expect("the event stream ended")
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

/// A shell command that reports its own pid through the fifo at `fifo`, then becomes `/bin/sleep 600` still holding
/// the fifo's write end. omp runs commands in its own shell where `sleep` is a builtin without a process and `$!` is
/// empty, so the child reports its pid itself; the sandbox denies listing processes.
pub fn sleeper(fifo: &Path) -> String {
  format!("/bin/sh -c 'exec 3> {}; echo $$ >&3; exec /bin/sleep 600'", fifo.display())
}

/// Creates the fifo a [`sleeper`] reports through.
pub fn make_fifo(dir: &Path) -> PathBuf {
  let fifo = dir.join("child.fifo");
  let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
  assert!(status.success());
  fifo
}

/// A [`sleeper`] that has reported in.
pub struct Child {
  /// The sleeper's pid.
  pub pid: u32,
  reader: BufReader<File>,
}

impl Child {
  /// Waits for the sleeper to report its pid through `fifo`. The fifo is read with blocking calls on the blocking
  /// pool: kqueue does not report end-of-file on a fifo, so tokio's pipe types cannot tell when the writer died.
  pub async fn report(fifo: &Path) -> Self {
    let fifo = fifo.to_owned();
    tokio::task::spawn_blocking(move || {
      // Opening the read end blocks until the sleeper opens the write end.
      let mut reader = BufReader::new(File::open(fifo).unwrap());
      let mut line = String::new();
      reader.read_line(&mut line).unwrap();
      Self {
        pid: line.trim().parse().unwrap(),
        reader,
      }
    })
    .await
    .unwrap()
  }

  /// Resolves once the sleeper has died: its write end of the fifo closes with it.
  pub async fn died(mut self) {
    tokio::task::spawn_blocking(move || {
      let mut rest = Vec::new();
      self.reader.read_to_end(&mut rest).unwrap();
    })
    .await
    .unwrap();
  }
}
