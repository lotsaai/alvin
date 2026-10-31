//! Starting omp processes.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use alvin_protocol::EventBody;
use omp_rpc::{Client, ClientOptions, Command, SetAskDialogCommand, SwitchSessionCommand};
use tokio::sync::mpsc;
use typed_builder::TypedBuilder;

use crate::bridge::Bridge;
use crate::session::Session;
use crate::{Error, Result};

/// Events buffered between omp and the consumer before the pump waits.
const DEFAULT_EVENT_CAPACITY: NonZeroUsize = NonZeroUsize::new(1024).unwrap();

/// How the host launches omp. All values come from the gateway's configuration; the host reads no environment and
/// chooses no defaults of its own beyond the event buffer size.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HostConfig {
  /// The omp executable.
  omp: PathBuf,
  /// Extra command-line arguments appended after the host's own.
  #[builder(default)]
  extra_args: Vec<OsString>,
  /// Environment variables added to the inherited environment.
  #[builder(default)]
  env: Vec<(OsString, OsString)>,
  /// Events buffered between omp and the consumer.
  #[builder(default = DEFAULT_EVENT_CAPACITY)]
  event_capacity: NonZeroUsize,
}

/// What to start.
#[derive(Debug, Clone, TypedBuilder)]
pub struct StartRequest {
  /// The working directory the session runs in and records.
  cwd: PathBuf,
  /// A model, as `provider/id`.
  #[builder(default)]
  model: Option<String>,
  /// Where omp writes its session file.
  #[builder(default)]
  session_dir: Option<PathBuf>,
  /// An existing session file to resume. omp starts on a new session and switches to it, restoring its saved model.
  #[builder(default)]
  resume: Option<PathBuf>,
}

/// Starts omp processes, one per live session.
#[derive(Debug, Clone)]
pub struct Host {
  config: Arc<HostConfig>,
}

impl Host {
  /// Creates a host from its configuration.
  pub fn new(config: HostConfig) -> Self {
    Self { config: Arc::new(config) }
  }

  /// Spawns `omp --mode rpc-ui` in the request's working directory and waits for its handshake. Returns the session handle
  /// and its one event stream; the stream is the caller's to drain. `rpc-ui` is the same
  /// transport as `rpc` with extension dialogs enabled; omp-rpc speaks it unchanged.
  pub async fn start(&self, request: StartRequest) -> Result<(Session, mpsc::Receiver<EventBody>)> {
    let config = Arc::clone(&self.config);
    tokio::task::spawn_blocking(move || launch(&config, request)).await?
  }
}

fn launch(config: &HostConfig, request: StartRequest) -> Result<(Session, mpsc::Receiver<EventBody>)> {
  let StartRequest {
    cwd,
    model,
    session_dir,
    resume,
  } = request;
  let mut process = Process::new(&config.omp);
  // `--cwd` sets the directory omp records; `current_dir` makes a missing directory fail at spawn with a clear error.
  process.args(["--mode", "rpc-ui", "--cwd"]).arg(&cwd).current_dir(&cwd);
  if let Some(model) = &model {
    process.arg("--model").arg(model);
  }
  if let Some(dir) = &session_dir {
    process.arg("--session-dir").arg(dir);
  }
  process
    .args(&config.extra_args)
    .envs(config.env.iter().map(|(name, value)| (name, value)));

  let (client, source) = Client::spawn(process, ClientOptions::default()).map_err(|source| match source {
    omp_rpc::Error::Timeout { ref command } if command == "ready" => Error::ReadyTimeout {
      cwd: cwd.clone(),
      source: Box::new(source),
    },
    source => Error::Spawn {
      cwd: cwd.clone(),
      source: Box::new(source),
    },
  })?;

  // Without it each `ask` call arrives as one `select` per choice. omp is pinned to one release that supports it, so a
  // failure is a startup error, not a fallback.
  let enabled = client
    .call(&SetAskDialogCommand { enabled: true })
    .map_err(|source| Error::from_rpc(&cwd, SetAskDialogCommand::NAME, source))?;
  if !enabled {
    return Err(Error::from_rpc(
      &cwd,
      SetAskDialogCommand::NAME,
      omp_rpc::Error::Protocol("omp did not enable ask dialogs".to_owned()),
    ));
  }

  if let Some(path) = &resume {
    let session_path = path.to_str().ok_or_else(|| Error::PathNotUtf8 { path: path.clone() })?.to_owned();
    let switched = client
      .call(&SwitchSessionCommand {
        session_path,
        provider: None,
        model_id: None,
      })
      .map_err(|source| Error::from_rpc(&cwd, SwitchSessionCommand::NAME, source))?;
    if switched.cancelled {
      return Err(Error::ResumeCancelled { cwd, path: path.clone() });
    }
  }
  let pid = client.pid().ok_or_else(|| Error::from_rpc(&cwd, "spawn", omp_rpc::Error::Closed))?;
  let closing = Arc::new(AtomicBool::new(false));
  let bridge = Bridge::start(source, config.event_capacity, Arc::clone(&closing), &cwd)?;
  Ok((Session::new(cwd, pid, client, closing, bridge.untranslated), bridge.events))
}
