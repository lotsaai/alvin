//! Wire contract between the alvin gateway and its clients.

pub mod error_codes;
pub mod ids;
pub mod omp_wire;

mod error;

pub use error::{Error, Result};
pub use ids::{EventId, LiveSessionId};

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
