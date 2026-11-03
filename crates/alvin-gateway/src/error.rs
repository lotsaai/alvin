//! The crate's error type and its mapping onto wire errors.

use std::io;

use std::path::PathBuf;

use alvin_protocol::{CheckoutId, ErrorCode, ErrorData, LiveSessionId, ProjectId, SessionId, TerminalId};
use jsonrpsee::types::error::INTERNAL_ERROR_CODE;
use jsonrpsee::types::{ErrorObject, ErrorObjectOwned};

/// Errors raised while serving protocol requests.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// Reading omp's session files failed.
  #[error("reading omp sessions")]
  SessionIndex(#[from] alvin_session_index::Error),

  /// A blocking task did not finish.
  #[error("blocking task did not finish")]
  Task(#[from] tokio::task::JoinError),

  /// No known project has this id, or the project has no such checkout.
  #[error("no project {project:?} with checkout {checkout:?}")]
  UnknownProject {
    /// The requested project.
    project: ProjectId,
    /// The requested checkout, when the request named one.
    checkout: Option<CheckoutId>,
  },

  /// No live session has this id.
  #[error("no live session {live:?}")]
  UnknownLiveSession {
    /// The requested live session.
    live: LiveSessionId,
  },

  /// No omp session file has this id.
  #[error("no session {session:?}")]
  UnknownSession {
    /// The requested session.
    session: SessionId,
  },

  /// The working directory a session would run in does not exist.
  #[error("the directory {path} does not exist")]
  MissingDirectory {
    /// The missing directory.
    path: PathBuf,
  },

  /// omp reported no session file for a session it just started.
  #[error("omp reported no session file in {cwd}")]
  NoSessionFile {
    /// The session's working directory.
    cwd: PathBuf,
  },

  /// Resolving a session file's directory failed.
  #[error("resolving the session file {path}")]
  SessionFilePath {
    /// The session file.
    path: PathBuf,
    /// The failure.
    #[source]
    source: io::Error,
  },

  /// An `events/subscribe` cursor is older than the retained log.
  #[error("the cursor is older than the retained events of {live:?}")]
  StaleCursor {
    /// The live session.
    live: LiveSessionId,
  },

  /// A command is still running in the live session, so it could not be closed in time.
  #[error("a command is still running in {live:?}")]
  StillRunning {
    /// The live session.
    live: LiveSessionId,
  },

  /// The omp host failed.
  #[error("omp host")]
  Host(#[from] alvin_omp_host::Error),

  /// A listing asked for pages of no sessions.
  #[error("the page size must be at least 1")]
  EmptyPage,

  /// The gateway has begun shutting down its terminals.
  #[error("the gateway is shutting down terminals")]
  TerminalShutdown,

  /// The terminal has not completed its initial handshake or is closing.
  #[error("terminal {terminal:?} is not ready")]
  TerminalNotReady { terminal: TerminalId },

  /// No terminal has this id.
  #[error("no terminal {terminal:?}")]
  UnknownTerminal { terminal: TerminalId },

  /// The terminal cursor does not belong to its retained event log.
  #[error("unknown event cursor for terminal {terminal:?}")]
  TerminalCursor { terminal: TerminalId },

  /// Allocating or opening a PTY failed.
  #[error("allocating terminal {terminal:?}")]
  TerminalAllocate {
    terminal: TerminalId,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
  },

  /// Starting the shell failed.
  #[error("starting terminal {terminal:?}")]
  TerminalSpawn {
    terminal: TerminalId,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
  },

  /// Reading shell output failed.
  #[error("reading terminal {terminal:?}")]
  TerminalRead {
    terminal: TerminalId,
    #[source]
    source: io::Error,
  },

  /// Writing shell input failed.
  #[error("writing terminal {terminal:?}")]
  TerminalWrite {
    terminal: TerminalId,
    #[source]
    source: io::Error,
  },

  /// Resizing the PTY failed.
  #[error("resizing terminal {terminal:?}")]
  TerminalResize {
    terminal: TerminalId,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
  },

  /// Waiting for the shell failed.
  #[error("waiting for terminal {terminal:?}")]
  TerminalWait {
    terminal: TerminalId,
    #[source]
    source: io::Error,
  },

  /// Terminating the shell failed.
  #[error("closing terminal {terminal:?}")]
  TerminalClose {
    terminal: TerminalId,
    #[source]
    source: io::Error,
  },

  /// A concurrent terminal close failed.
  #[error("closing terminal {terminal:?}")]
  TerminalCloseShared {
    terminal: TerminalId,
    #[source]
    source: std::sync::Arc<Error>,
  },

  /// The terminal close task ended without publishing completion.
  #[error("waiting for terminal {terminal:?} to close")]
  TerminalCompletion {
    terminal: TerminalId,
    #[source]
    source: tokio::sync::watch::error::RecvError,
  },

  /// Two protocol areas declare the same method name.
  #[error("registering protocol methods")]
  RegisterMethods(#[from] jsonrpsee::server::RegisterMethodError),

  /// The WebSocket listener could not bind.
  #[error("binding the WebSocket listener")]
  Bind(#[source] io::Error),

  /// The WebSocket listener was already stopped.
  #[error("stopping the WebSocket listener")]
  AlreadyStopped(#[from] jsonrpsee::server::AlreadyStoppedError),

  /// Setting up telemetry failed.
  #[error("setting up telemetry")]
  Telemetry(#[from] init_tracing_opentelemetry::Error),

  /// The log filter in `ALVIN_LOG` or `RUST_LOG` does not parse.
  #[error("parsing the log filter")]
  LogFilter(#[from] tracing_subscriber::filter::ParseError),
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The error and every cause beneath it, joined, so a client sees why and not only what.
fn chain(error: &dyn std::error::Error) -> String {
  let mut message = error.to_string();
  let mut source = error.source();
  while let Some(cause) = source {
    message.push_str(": ");
    message.push_str(&cause.to_string());
    source = cause.source();
  }
  message
}

impl From<Error> for ErrorObjectOwned {
  fn from(error: Error) -> Self {
    let message = chain(&error);
    match &error {
      Error::SessionIndex(source) => ErrorObject::owned(
        ErrorCode::SessionIndexFailed.code(),
        message,
        Some(ErrorData::SessionIndex {
          path: source.path().to_path_buf(),
        }),
      ),
      Error::UnknownProject { project, checkout } => ErrorObject::owned(
        ErrorCode::UnknownProject.code(),
        message,
        Some(ErrorData::UnknownProject {
          project: project.clone(),
          checkout: checkout.clone(),
        }),
      ),
      Error::UnknownLiveSession { live } => ErrorObject::owned(
        ErrorCode::UnknownLiveSession.code(),
        message,
        Some(ErrorData::UnknownLiveSession { live: *live }),
      ),
      Error::UnknownSession { session } => ErrorObject::owned(
        ErrorCode::UnknownSession.code(),
        message,
        Some(ErrorData::UnknownSession { session: session.clone() }),
      ),
      Error::EmptyPage | Error::MissingDirectory { .. } => ErrorObject::owned(ErrorCode::InvalidRequest.code(), message, None::<()>),
      Error::StaleCursor { .. } => ErrorObject::owned(ErrorCode::StaleCursor.code(), message, None::<()>),
      Error::TerminalShutdown | Error::TerminalNotReady { .. } => {
        ErrorObject::owned(ErrorCode::CommandNotAllowed.code(), message, None::<()>)
      }
      Error::UnknownTerminal { .. } => ErrorObject::owned(ErrorCode::UnknownTerminal.code(), message, None::<()>),
      Error::TerminalCursor { .. } => ErrorObject::owned(ErrorCode::StaleCursor.code(), message, None::<()>),
      Error::TerminalAllocate { .. } => ErrorObject::owned(ErrorCode::TerminalAllocateFailed.code(), message, None::<()>),
      Error::TerminalSpawn { .. } => ErrorObject::owned(ErrorCode::TerminalSpawnFailed.code(), message, None::<()>),
      Error::TerminalRead { .. } => ErrorObject::owned(ErrorCode::TerminalReadFailed.code(), message, None::<()>),
      Error::TerminalWrite { .. } => ErrorObject::owned(ErrorCode::TerminalWriteFailed.code(), message, None::<()>),
      Error::TerminalResize { .. } => ErrorObject::owned(ErrorCode::TerminalResizeFailed.code(), message, None::<()>),
      Error::TerminalWait { .. } => ErrorObject::owned(ErrorCode::TerminalWaitFailed.code(), message, None::<()>),
      Error::TerminalClose { .. } | Error::TerminalCloseShared { .. } | Error::TerminalCompletion { .. } => {
        ErrorObject::owned(ErrorCode::TerminalCloseFailed.code(), message, None::<()>)
      }
      Error::StillRunning { .. } => ErrorObject::owned(ErrorCode::SessionBusy.code(), message, None::<()>),
      Error::Host(alvin_omp_host::Error::ProcessExited { .. }) => {
        ErrorObject::owned(ErrorCode::OmpProcessExited.code(), message, None::<()>)
      }
      Error::Host(alvin_omp_host::Error::Command { code, .. }) => ErrorObject::owned(
        ErrorCode::OmpCommandFailed.code(),
        message,
        Some(ErrorData::OmpCommandFailed { code: code.clone() }),
      ),
      Error::Task(_)
      | Error::Host(_)
      | Error::NoSessionFile { .. }
      | Error::SessionFilePath { .. }
      | Error::Bind(_)
      | Error::AlreadyStopped(_)
      | Error::RegisterMethods(_)
      | Error::Telemetry(_)
      | Error::LogFilter(_) => ErrorObject::owned(INTERNAL_ERROR_CODE, message, None::<()>),
    }
  }
}
