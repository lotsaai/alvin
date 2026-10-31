//! A live session's event stream.

use alvin_protocol::{Event, EventsApiClient, SubscribeRequest};
use jsonrpsee::core::client::Subscription;

use crate::{Client, Error, Result};

/// A live session's events, in order and without duplicates. The stream ends when the gateway ends the subscription (the
/// subscriber fell behind, or the live session closed) or the connection drops; resubscribe with the last event id.
#[derive(Debug)]
pub struct EventStream {
  inner: Subscription<Event>,
}

impl EventStream {
  /// The next event, or `None` when the stream has ended.
  pub async fn next(&mut self) -> Option<Result<Event>> {
    self.inner.next().await.map(|item| item.map_err(Error::Response))
  }
}

impl Client {
  /// Subscribes to a live session's events. Fails with the gateway's stale-cursor error when `request.after` is older
  /// than the retained log.
  pub async fn events(&self, request: SubscribeRequest) -> Result<EventStream> {
    Ok(EventStream {
      inner: EventsApiClient::events(self, request).await?,
    })
  }
}
