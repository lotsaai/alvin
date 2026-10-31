//! The crate's error type and its mapping onto wire errors.

use std::io;

use alvin_protocol::{ErrorCode, ErrorData};
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

  /// The WebSocket listener could not bind.
  #[error("binding the WebSocket listener")]
  Bind(#[source] io::Error),

  /// The WebSocket listener was already stopped.
  #[error("stopping the WebSocket listener")]
  AlreadyStopped(#[from] jsonrpsee::server::AlreadyStoppedError),
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
      Error::Task(_) | Error::Bind(_) | Error::AlreadyStopped(_) => ErrorObject::owned(INTERNAL_ERROR_CODE, message, None::<()>),
    }
  }
}
