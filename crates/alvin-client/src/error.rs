//! The client's error type. Gateway errors keep their protocol code and typed data.

use std::sync::Arc;

use alvin_protocol::{ErrorCode, ErrorData};
use jsonrpsee::core::client::Error as RpcError;

/// Errors a client call or connection can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// The gateway answered with an error from its stable code table.
  #[error("gateway error {code:?}: {message}")]
  Gateway {
    /// The protocol error code.
    code: ErrorCode,
    /// The gateway's message.
    message: String,
    /// Typed context, when the gateway sent some.
    data: Option<ErrorData>,
  },

  /// The gateway answered with a code outside the table this client knows.
  #[error("gateway error with unknown code {code}: {message}")]
  UnknownCode {
    /// The code on the wire.
    code: i32,
    /// The gateway's message.
    message: String,
  },

  /// A gateway error carried `data` that does not decode as [`ErrorData`].
  #[error("gateway error {code:?} carried malformed data")]
  MalformedErrorData {
    /// The protocol error code.
    code: ErrorCode,
    /// The decoding error.
    #[source]
    source: serde_json::Error,
  },

  /// The connection to the gateway is gone.
  #[error("disconnected from the gateway")]
  Disconnected(#[source] Arc<RpcError>),

  /// The transport failed.
  #[error("transport failure")]
  Transport(#[source] Box<dyn std::error::Error + Send + Sync>),

  /// The request timed out.
  #[error("request timed out")]
  RequestTimeout,

  /// A response did not decode as the expected type.
  #[error("decoding a response")]
  Response(#[source] serde_json::Error),

  /// The JSON-RPC client rejected the call itself, for example a malformed subscription id.
  #[error("JSON-RPC client error")]
  Rpc(#[source] RpcError),

  /// The endpoint is not a URL.
  #[error("invalid endpoint URL")]
  InvalidUrl(#[from] url::ParseError),

  /// The endpoint's scheme is not served by any transport this client has.
  #[error("unsupported endpoint scheme {scheme:?}")]
  UnsupportedScheme {
    /// The rejected scheme.
    scheme: String,
  },
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<RpcError> for Error {
  fn from(error: RpcError) -> Self {
    match error {
      RpcError::Call(object) => {
        let (code, message) = (object.code(), object.message().to_owned());
        let Some(known) = ErrorCode::from_code(code) else {
          return Self::UnknownCode { code, message };
        };
        match object.data().map(|raw| serde_json::from_str(raw.get())).transpose() {
          Ok(data) => Self::Gateway {
            code: known,
            message,
            data,
          },
          Err(source) => Self::MalformedErrorData { code: known, source },
        }
      }
      RpcError::RestartNeeded(cause) => Self::Disconnected(cause),
      RpcError::Transport(source) => Self::Transport(source),
      RpcError::RequestTimeout => Self::RequestTimeout,
      RpcError::ParseError(source) => Self::Response(source),
      other => Self::Rpc(other),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use alvin_protocol::{ErrorCode, ErrorData};
  use jsonrpsee::core::client::Error as RpcError;
  use jsonrpsee::types::ErrorObjectOwned;

  use super::Error;

  #[test]
  fn a_gateway_error_keeps_its_code_and_typed_data() {
    let data = ErrorData::SessionIndex { path: "/sessions".into() };
    let wire = ErrorObjectOwned::owned(ErrorCode::SessionIndexFailed.code(), "reading omp sessions", Some(&data));

    let error = Error::from(RpcError::Call(wire));

    assert!(
      matches!(&error, Error::Gateway { code: ErrorCode::SessionIndexFailed, data: Some(received), .. } if *received == data),
      "{error:?}"
    );
  }

  #[test]
  fn a_code_outside_the_table_is_unknown_not_guessed() {
    let wire = ErrorObjectOwned::owned(-32099, "from a newer gateway", None::<()>);

    let error = Error::from(RpcError::Call(wire));

    assert!(matches!(error, Error::UnknownCode { code: -32099, .. }), "{error:?}");
  }

  #[test]
  fn data_that_is_not_error_data_is_reported_not_dropped() {
    let wire = ErrorObjectOwned::owned(ErrorCode::StaleCursor.code(), "stale", Some("not error data"));

    let error = Error::from(RpcError::Call(wire));

    assert!(
      matches!(
        error,
        Error::MalformedErrorData {
          code: ErrorCode::StaleCursor,
          ..
        }
      ),
      "{error:?}"
    );
  }

  #[test]
  fn a_terminated_background_task_is_a_disconnect() {
    let error = Error::from(RpcError::RestartNeeded(Arc::new(RpcError::RequestTimeout)));

    assert!(matches!(error, Error::Disconnected(_)), "{error:?}");
  }
}
