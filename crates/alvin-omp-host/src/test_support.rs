//! Helpers for tests that start real omp.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;

use alvin_protocol::{AssistantMessageEvent, EventBody, LiveSessionId, PromptStatus, RpcAgentEvent};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
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

/// A host with isolated sessions and agent configuration.
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
    let cwd = TempDir::new();
    let paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").expect("PATH is required")).collect();
    let find = |name: &str| {
      let names = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_owned()]
      } else {
        vec![name.to_owned()]
      };
      paths
        .iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
        .unwrap()
    };
    let omp = find("omp");
    let bun = find("bun");
    let mut dirs = vec![bun.parent().unwrap().to_path_buf()];
    if cfg!(windows) {
      dirs.push(PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot is required")).join("System32"));
    } else {
      dirs.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    }
    let path = std::env::join_paths(dirs).unwrap();
    let mut env = vec![
      (OsString::from("HOME"), cwd.path().as_os_str().to_owned()),
      (OsString::from("PI_CODING_AGENT_DIR"), cwd.path().join("agent").into_os_string()),
      (OsString::from("PATH"), path),
      (
        OsString::from("OPENROUTER_API_KEY"),
        std::env::var_os("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY is required"),
      ),
    ];
    if cfg!(windows) {
      for name in ["SystemRoot", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
          env.push((OsString::from(name), value));
        }
      }
    }
    let config = HostConfig::builder()
      .omp(omp)
      .extra_args(
        ["--no-tools", "--no-skills", "--no-rules", "--no-extensions", "--no-context-files"]
          .map(OsString::from)
          .to_vec(),
      )
      .clear_env(true)
      .env(env)
      .event_capacity(NonZeroUsize::new(capacity).unwrap())
      .build();
    Self {
      host: Host::new(config),
      cwd,
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
  if cfg!(windows) {
    let output = Command::new("tasklist")
      .args(["/FO", "CSV", "/NH", "/FI", &format!("PID eq {pid}")])
      .output()
      .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).contains(&format!("\",\"{pid}\","))
  } else {
    Command::new("kill").args(["-0", &pid.to_string()]).status().unwrap().success()
  }
}

/// Forcibly terminates the process without terminating its descendants directly.
pub fn kill(pid: u32) {
  let status = if cfg!(windows) {
    Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status().unwrap()
  } else {
    Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap()
  };
  assert!(status.success());
}

/// A Bun process reports its pid over TCP and holds the connection until it is terminated.
pub fn sleeper(listener: &TcpListener) -> String {
  let port = listener.local_addr().unwrap().port();
  format!(
    "bun -e 'import net from \"net\"; const s = net.connect({port}, \"127.0.0.1\"); s.on(\"connect\", () => s.write(process.pid + \"\\n\"));'"
  )
}

/// A sleeper that has reported in.
pub struct Child {
  pub pid: u32,
  reader: BufReader<TcpStream>,
}

impl Child {
  /// Waits for the sleeper to connect and report its pid.
  pub async fn report(listener: TcpListener) -> Self {
    let (stream, _) = listener.accept().await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    Self {
      pid: line.trim().parse().unwrap(),
      reader,
    }
  }

  /// Resolves when the sleeper dies and its socket closes.
  pub async fn died(mut self) {
    let mut rest = Vec::new();
    if let Err(error) = self.reader.read_to_end(&mut rest).await {
      assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    }
  }
}
