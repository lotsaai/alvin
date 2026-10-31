//! A live session's event stream, which survives reconnects.

use alvin_protocol::{ErrorCode, Event, EventId, EventsApiClient, LiveSessionId, SubscribeRequest};
use jsonrpsee::core::client::Subscription;
use tokio::sync::watch;

use crate::{Client, ConnectionState, Error, Result};

/// Why a stream stopped and asks its owner to start over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
  /// The gateway no longer retains the events after the stream's last one, or the stream kept ending without making
  /// progress. Reload the transcript with `live/messages` and subscribe again without a cursor.
  StaleCursor,
  /// The gateway does not know the live session: it was closed, or the gateway restarted and lost its live sessions.
  UnknownLiveSession,
}

/// A live session's events, in order, without gaps or duplicates.
///
/// The stream records the id of the last event it delivered. When the connection drops it waits for the client to
/// reconnect and subscribes again from that id; when the gateway ends the subscription (the subscriber fell behind, or
/// the live session closed) it subscribes again once. A gateway that cannot continue from the id ends the stream with one
/// [`Error::Resync`]. The stream ends with `None` when the client is [`ConnectionState::Lost`].
#[derive(Debug)]
pub struct EventStream {
  client: Client,
  live: LiveSessionId,
  last: Option<EventId>,
  subscription: Option<Subscription<Event>>,
  state: watch::Receiver<ConnectionState>,
  /// Whether the current subscription has delivered an event.
  delivered: bool,
  /// Subscriptions in a row that ended while connected without delivering anything.
  idle_endings: u8,
  finished: bool,
}

/// Idle endings in a row before the stream gives up and asks for a resync.
const MAX_IDLE_ENDINGS: u8 = 2;

enum Resubscribed {
  Subscribed(Subscription<Event>),
  Resync(ResyncReason),
  Lost,
  Failed(Error),
}

impl EventStream {
  /// The next event, or `None` when the stream has ended. A stream that cannot continue without a gap ends with one
  /// `Err(`[`Error::Resync`]`)`.
  pub async fn next(&mut self) -> Option<Result<Event>> {
    if self.finished {
      return None;
    }
    loop {
      if let Some(subscription) = &mut self.subscription {
        match subscription.next().await {
          Some(Ok(event)) => {
            self.last = Some(event.id);
            self.delivered = true;
            self.idle_endings = 0;
            return Some(Ok(event));
          }
          Some(Err(error)) => return Some(Err(Error::Response(error))),
          None => {
            self.subscription = None;
            if !self.delivered && *self.state.borrow() == ConnectionState::Connected {
              self.idle_endings = self.idle_endings.saturating_add(1);
            }
          }
        }
      }
      if self.idle_endings >= MAX_IDLE_ENDINGS {
        return self.finish(Some(Err(Error::Resync(ResyncReason::StaleCursor))));
      }
      match self.resubscribe().await {
        Resubscribed::Subscribed(subscription) => {
          self.subscription = Some(subscription);
          self.delivered = false;
        }
        Resubscribed::Resync(reason) => return self.finish(Some(Err(Error::Resync(reason)))),
        Resubscribed::Lost => return self.finish(None),
        Resubscribed::Failed(error) => return self.finish(Some(Err(error))),
      }
    }
  }

  fn finish(&mut self, last: Option<Result<Event>>) -> Option<Result<Event>> {
    self.finished = true;
    last
  }

  /// Waits for a connection, then subscribes from the last delivered event.
  async fn resubscribe(&mut self) -> Resubscribed {
    loop {
      if !self.connected().await {
        return Resubscribed::Lost;
      }
      let request = SubscribeRequest {
        live: self.live,
        after: self.last,
      };
      match subscribe(&self.client, request).await {
        Ok(subscription) => return Resubscribed::Subscribed(subscription),
        Err(Error::Gateway {
          code: ErrorCode::StaleCursor,
          ..
        }) => return Resubscribed::Resync(ResyncReason::StaleCursor),
        Err(Error::Gateway {
          code: ErrorCode::UnknownLiveSession,
          ..
        }) => return Resubscribed::Resync(ResyncReason::UnknownLiveSession),
        // The connection dropped under the attempt. The supervisor notices, publishes a new state, and the next pass
        // waits for the reconnect.
        Err(Error::Disconnected(_)) => {
          if self.state.changed().await.is_err() {
            return Resubscribed::Lost;
          }
        }
        Err(error) => return Resubscribed::Failed(error),
      }
    }
  }

  /// Waits until the client is connected. False when it is lost for good.
  async fn connected(&mut self) -> bool {
    loop {
      match *self.state.borrow_and_update() {
        ConnectionState::Connected => return true,
        ConnectionState::Lost => return false,
        ConnectionState::Reconnecting { .. } => {}
      }
      if self.state.changed().await.is_err() {
        return false;
      }
    }
  }
}

async fn subscribe(client: &Client, request: SubscribeRequest) -> Result<Subscription<Event>> {
  Ok(EventsApiClient::events(client, request).await?)
}

impl Client {
  /// Subscribes to a live session's events. Fails with the gateway's stale-cursor error when `request.after` is older
  /// than the retained log. The returned stream continues across reconnects; see [`EventStream`].
  pub async fn events(&self, request: SubscribeRequest) -> Result<EventStream> {
    let live = request.live;
    let after = request.after;
    let mut state = self.watch_connection();
    state.mark_unchanged();
    Ok(EventStream {
      subscription: Some(subscribe(self, request).await?),
      client: self.clone(),
      live,
      last: after,
      state,
      delivered: false,
      idle_endings: 0,
      finished: false,
    })
  }
}
