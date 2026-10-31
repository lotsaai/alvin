//! Wire contract between the alvin gateway and its clients.

pub mod error_codes;
pub mod ids;
pub mod omp_wire;

mod error;

pub use error::{Error, Result};
pub use ids::{EventId, LiveSessionId};

#[cfg(test)]
mod client_tests {
  use jsonrpsee::core::client::Error as ClientError;
  use jsonrpsee::types::ErrorObjectOwned;

  use crate::error_codes::STALE_CURSOR;

  /// The client half (default features) surfaces a gateway error with its stable code intact.
  #[test]
  fn client_half_keeps_the_wire_error_code() {
    let wire = ErrorObjectOwned::owned(STALE_CURSOR, "cursor too old", None::<()>);
    let json = serde_json::to_string(&wire).unwrap();
    let received: ErrorObjectOwned = serde_json::from_str(&json).unwrap();
    match ClientError::Call(received) {
      ClientError::Call(error) => assert_eq!(error.code(), STALE_CURSOR),
      other => panic!("unexpected client error: {other}"),
    }
  }
}

#[cfg(all(test, feature = "server"))]
mod server_tests {
  use jsonrpsee::RpcModule;

  /// `RpcModule` is the server half of jsonrpsee; it is only linked when `server` enables `server-core`.
  #[test]
  fn server_feature_enables_the_jsonrpsee_server_half() {
    let mut module = RpcModule::new(());
    module.register_method("live/ping", |_, _, _| "pong").unwrap();
    assert_eq!(module.method_names().collect::<Vec<_>>(), ["live/ping"]);
  }
}
