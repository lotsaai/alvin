//! The crate's error type and its mapping onto wire errors.

use std::io;

use alvin_protocol::{CheckoutId, ErrorCode, ErrorData, ProjectId};
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

  /// A listing asked for pages of no sessions.
  #[error("the page size must be at least 1")]
  EmptyPage,

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

impl From<Error> for ErrorObjectOwned {
  fn from(error: Error) -> Self {
    let message = error.to_string();
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
      Error::EmptyPage => ErrorObject::owned(ErrorCode::InvalidRequest.code(), message, None::<()>),
      Error::Task(_)
      | Error::Bind(_)
      | Error::AlreadyStopped(_)
      | Error::RegisterMethods(_)
      | Error::Telemetry(_)
      | Error::LogFilter(_) => ErrorObject::owned(INTERNAL_ERROR_CODE, message, None::<()>),
    }
  }
}
