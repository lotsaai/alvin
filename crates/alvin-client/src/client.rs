//! The client: one type over every transport.

use std::fmt;

use jsonrpsee::core::client::{BatchResponse, ClientT, Error as RpcError, Subscription, SubscriptionClientT};
use jsonrpsee::core::params::BatchRequestBuilder;
use jsonrpsee::core::traits::ToRpcParams;
use serde::de::DeserializeOwned;

use crate::{Endpoint, Result};

/// A connection to a gateway. Call protocol methods through the generated `*ApiClient` traits, which this type
/// implements by delegating to the transport chosen at connect time.
#[derive(Debug)]
pub struct Client {
  inner: jsonrpsee::core::client::Client,
}

impl Client {
  /// Connects to the gateway at `endpoint`. Must be called inside a tokio runtime on native targets.
  pub async fn connect(endpoint: Endpoint) -> Result<Self> {
    let inner = match endpoint {
      #[cfg(not(target_arch = "wasm32"))]
      Endpoint::Channel(pair) => crate::channel::connect(pair),
      Endpoint::WebSocket(url) => crate::ws::connect(&url).await?,
    };
    Ok(Self { inner })
  }
}

impl ClientT for Client {
  async fn notification<Params>(&self, method: &str, params: Params) -> Result<(), RpcError>
  where
    Params: ToRpcParams + Send,
  {
    self.inner.notification(method, params).await
  }

  async fn request<R, Params>(&self, method: &str, params: Params) -> Result<R, RpcError>
  where
    R: DeserializeOwned,
    Params: ToRpcParams + Send,
  {
    self.inner.request(method, params).await
  }

  async fn batch_request<'a, R>(&self, batch: BatchRequestBuilder<'a>) -> Result<BatchResponse<'a, R>, RpcError>
  where
    R: DeserializeOwned + fmt::Debug + 'a,
  {
    self.inner.batch_request(batch).await
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
    self.inner.subscribe(subscribe_method, params, unsubscribe_method).await
  }

  async fn subscribe_to_method<Notif>(&self, method: &str) -> Result<Subscription<Notif>, RpcError>
  where
    Notif: DeserializeOwned,
  {
    self.inner.subscribe_to_method(method).await
  }
}
