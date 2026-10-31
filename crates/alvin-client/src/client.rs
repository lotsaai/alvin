//! The client: one type over every transport, reconnecting where the transport can.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use jsonrpsee::core::client::{BatchResponse, ClientT, Error as RpcError, Subscription, SubscriptionClientT};
use jsonrpsee::core::params::BatchRequestBuilder;
use jsonrpsee::core::traits::ToRpcParams;
use serde::de::DeserializeOwned;
use tokio::sync::{Notify, watch};

use crate::reconnect::{ConnectionState, Reconnect, supervise};
use crate::{Endpoint, Result};

/// A connection to the gateway. Call protocol methods through the generated `*ApiClient` traits, which this type
/// implements by delegating to the connection it currently holds. Cheap to clone: clones share one connection.
///
/// A dropped WebSocket connection is replaced in the background (see [`ConnectionState`]). Calls are never retried: a call
/// in flight when the connection drops fails with [`Error::Disconnected`](crate::Error::Disconnected), because a retry
/// could run `live/prompt` or `live/bash` twice.
#[derive(Clone)]
pub struct Client {
  shared: Arc<Shared>,
}

/// What every clone of a [`Client`] and the supervisor share.
pub(crate) struct Shared {
  connection: RwLock<Arc<jsonrpsee::core::client::Client>>,
  state: watch::Sender<ConnectionState>,
  /// Notified when the last clone is dropped, which ends the supervisor.
  closed: Arc<Notify>,
}

impl Shared {
  /// The connection calls go to now.
  pub(crate) fn current(&self) -> Arc<jsonrpsee::core::client::Client> {
    Arc::clone(&self.connection.read().unwrap_or_else(PoisonError::into_inner))
  }

  /// Swaps in a reconnected connection.
  pub(crate) fn replace(&self, connection: Arc<jsonrpsee::core::client::Client>) {
    *self.connection.write().unwrap_or_else(PoisonError::into_inner) = connection;
  }

  pub(crate) fn set_state(&self, state: ConnectionState) {
    self.state.send_replace(state);
  }
}

impl Drop for Shared {
  fn drop(&mut self) {
    self.closed.notify_one();
  }
}

impl fmt::Debug for Client {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Client")
      .field("state", &self.connection_state())
      .finish_non_exhaustive()
  }
}

impl Client {
  /// Connects to the gateway at `endpoint`. Call inside a tokio runtime on native targets.
  pub async fn connect(endpoint: Endpoint) -> Result<Self> {
    let (connection, reconnect) = match endpoint {
      #[cfg(not(target_arch = "wasm32"))]
      Endpoint::Channel(pair) => (crate::channel::connect(pair), Reconnect::Never),
      Endpoint::WebSocket(url) => (crate::ws::connect(&url).await?, Reconnect::WebSocket(url)),
    };
    let closed = Arc::new(Notify::new());
    let shared = Arc::new(Shared {
      connection: RwLock::new(Arc::new(connection)),
      state: watch::channel(ConnectionState::Connected).0,
      closed: Arc::clone(&closed),
    });
    supervise(&shared, closed, reconnect);
    Ok(Self { shared })
  }

  /// Where the connection stands now.
  pub fn connection_state(&self) -> ConnectionState {
    *self.shared.state.borrow()
  }

  /// A receiver that observes every change of [`ConnectionState`].
  pub fn watch_connection(&self) -> watch::Receiver<ConnectionState> {
    self.shared.state.subscribe()
  }

  pub(crate) fn connection(&self) -> Arc<jsonrpsee::core::client::Client> {
    self.shared.current()
  }
}

impl ClientT for Client {
  async fn notification<Params>(&self, method: &str, params: Params) -> Result<(), RpcError>
  where
    Params: ToRpcParams + Send,
  {
    self.connection().notification(method, params).await
  }

  async fn request<R, Params>(&self, method: &str, params: Params) -> Result<R, RpcError>
  where
    R: DeserializeOwned,
    Params: ToRpcParams + Send,
  {
    self.connection().request(method, params).await
  }

  async fn batch_request<'a, R>(&self, batch: BatchRequestBuilder<'a>) -> Result<BatchResponse<'a, R>, RpcError>
  where
    R: DeserializeOwned + fmt::Debug + 'a,
  {
    self.connection().batch_request(batch).await
  }
}

impl SubscriptionClientT for Client {
  async fn subscribe<'a, Notif, Params>(
    &self,
    subscribe_method: &'a str,
    params: Params,
    unsubscribe_method: &'a str,
  ) -> Result<Subscription<Notif>, RpcError>
  where
    Params: ToRpcParams + Send,
    Notif: DeserializeOwned,
  {
    self.connection().subscribe(subscribe_method, params, unsubscribe_method).await
  }

  async fn subscribe_to_method<Notif>(&self, method: &str) -> Result<Subscription<Notif>, RpcError>
  where
    Notif: DeserializeOwned,
  {
    self.connection().subscribe_to_method(method).await
  }
}
