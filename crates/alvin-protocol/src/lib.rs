//! Wire contract between the alvin gateway and its clients.

mod error;
mod error_codes;
mod events;
mod ids;
mod live;
mod omp_wire;
mod projects;
mod queue;
mod sessions;

pub use error::{Error, Result};
pub use error_codes::{ErrorCode, ErrorData};
pub use events::{Event, EventBody};
pub use ids::{CheckoutId, EventId, LiveSessionId, ProjectId, SessionId};
pub use live::{
  BashRequest, EventsApiClient, LiveApiClient, LiveSessionSummary, MessagesRequest, OpenRequest, OpenTarget, PromptRequest,
  SubscribeRequest,
};
#[cfg(feature = "server")]
pub use live::{EventsApiServer, LiveApiServer};
pub use omp_wire::*;
#[cfg(feature = "server")]
pub use projects::ProjectsApiServer;
pub use projects::{CheckoutSummary, ProjectSummary, ProjectsApiClient, Vcs};
#[cfg(feature = "server")]
pub use queue::QueueApiServer;
pub use queue::{PromoteQueuedRequest, QueueApiClient, RemoveQueuedRequest};
#[cfg(feature = "server")]
pub use sessions::SessionsApiServer;
pub use sessions::{ListSessions, SessionCursor, SessionPage, SessionStatus, SessionSummary, SessionsApiClient};

#[cfg(test)]
mod client_tests {
  use jsonrpsee::core::client::Error as ClientError;
  use jsonrpsee::types::ErrorObjectOwned;

  use crate::ErrorCode;

  /// The client half (default features) surfaces a gateway error with its stable code intact.
  #[test]
  fn client_half_keeps_the_wire_error_code() {
    let wire = ErrorObjectOwned::owned(ErrorCode::StaleCursor.code(), "cursor too old", None::<()>);
    let json = serde_json::to_string(&wire).unwrap();
    let received: ErrorObjectOwned = serde_json::from_str(&json).unwrap();
    match ClientError::Call(received) {
      ClientError::Call(error) => assert_eq!(error.code(), ErrorCode::StaleCursor.code()),
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
