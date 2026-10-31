//! Reconnecting a dropped WebSocket connection.
//!
//! jsonrpsee reports a dropped connection through `on_disconnect` and fails later calls; it never reconnects. A
//! supervisor task watches the current connection and, on the WebSocket transport, replaces it with backoff until the
//! [`Client`](crate::Client) is dropped. An in-process channel does not drop a live connection: a closed channel means
//! the gateway shut down, so it ends in [`ConnectionState::Lost`] without a retry.

use std::sync::{Arc, Weak};

use jiff::SignedDuration;
use tokio::sync::Notify;
use url::Url;

use crate::client::Shared;

/// Where a [`Client`](crate::Client)'s connection stands. The initial connect is `Client::connect` itself, so a client
/// that exists is at least connected once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
  /// Calls and event streams work.
  Connected,
  /// The connection dropped and the client is trying again. `attempt` counts failed reconnects since the drop. Calls fail
  /// at once with [`Error::Disconnected`](crate::Error::Disconnected) and are never retried.
  Reconnecting {
    /// Failed reconnect attempts so far.
    attempt: u32,
  },
  /// The connection is gone for good: the gateway behind an in-process channel shut down.
  Lost,
}

/// How a connection is replaced after it drops.
#[derive(Debug, Clone)]
pub(crate) enum Reconnect {
  /// Never: the transport cannot be rebuilt. There is no in-process channel in the browser.
  #[cfg(not(target_arch = "wasm32"))]
  Never,
  /// Connect to this URL again.
  WebSocket(Url),
}

/// First delay after a drop.
const INITIAL_DELAY: SignedDuration = SignedDuration::from_millis(250);
/// The delay stops growing here.
const MAX_DELAY: SignedDuration = SignedDuration::from_secs(30);
/// The delay doubles per failed attempt up to this many doublings; `250 ms << 7` is already past the cap.
const MAX_DOUBLINGS: u32 = 7;
/// Jitter adds up to this fraction of the delay, so clients that dropped together do not reconnect together.
const JITTER_FRACTION: f64 = 0.25;

/// The delay before reconnect attempt `attempt` (the first is 0): 250 ms doubling to 30 s, plus up to 25 % of it.
/// `jitter` is a number in `[0, 1)`.
pub(crate) fn backoff(attempt: u32, jitter: f64) -> SignedDuration {
  let doublings = attempt.min(MAX_DOUBLINGS);
  let base = INITIAL_DELAY.saturating_mul(1_i32 << doublings).min(MAX_DELAY);
  base + base.mul_f64(JITTER_FRACTION * jitter)
}

/// A number in `[0, 1)` from std's randomly keyed hasher; no random-number dependency for a delay.
fn jitter() -> f64 {
  use std::hash::BuildHasher as _;
  let bits = std::collections::hash_map::RandomState::new().hash_one(0_u8);
  (bits >> 11) as f64 / (1_u64 << 53) as f64
}

#[cfg(not(target_arch = "wasm32"))]
async fn sleep(delay: SignedDuration) {
  tokio::time::sleep(delay.unsigned_abs()).await;
}

#[cfg(target_arch = "wasm32")]
async fn sleep(delay: SignedDuration) {
  // tokio's timer does not run in the browser.
  gloo_timers::future::sleep(delay.unsigned_abs()).await;
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(task: impl Future<Output = ()> + Send + 'static) {
  // Detached: the task ends when the client is dropped.
  drop(tokio::spawn(task));
}

#[cfg(target_arch = "wasm32")]
fn spawn(task: impl Future<Output = ()> + 'static) {
  wasm_bindgen_futures::spawn_local(task);
}

/// Starts the supervisor for `shared`. It holds only a weak reference, plus `closed`, which `Shared` notifies when the
/// client is dropped.
pub(crate) fn supervise(shared: &Arc<Shared>, closed: Arc<Notify>, reconnect: Reconnect) {
  spawn(run(Arc::downgrade(shared), closed, reconnect));
}

async fn run(shared: Weak<Shared>, closed: Arc<Notify>, reconnect: Reconnect) {
  loop {
    let Some(connection) = shared.upgrade().map(|shared| shared.current()) else {
      return;
    };
    tokio::select! {
      reason = connection.on_disconnect() => tracing::debug!(%reason, "the gateway connection ended"),
      () = closed.notified() => return,
    }
    drop(connection);

    let url = match &reconnect {
      #[cfg(not(target_arch = "wasm32"))]
      Reconnect::Never => {
        set_state(&shared, ConnectionState::Lost);
        return;
      }
      Reconnect::WebSocket(url) => url,
    };
    let mut attempt = 0_u32;
    loop {
      if !set_state(&shared, ConnectionState::Reconnecting { attempt }) {
        return;
      }
      tokio::select! {
        () = sleep(backoff(attempt, jitter())) => {}
        () = closed.notified() => return,
      }
      match crate::ws::connect(url).await {
        Ok(connection) => {
          let Some(shared) = shared.upgrade() else {
            return;
          };
          shared.replace(Arc::new(connection));
          shared.set_state(ConnectionState::Connected);
          break;
        }
        Err(error) => {
          tracing::debug!(%error, attempt, "reconnecting to the gateway failed");
          attempt = attempt.saturating_add(1);
        }
      }
    }
  }
}

/// Publishes `state`. False when the client is gone.
fn set_state(shared: &Weak<Shared>, state: ConnectionState) -> bool {
  shared.upgrade().map(|shared| shared.set_state(state)).is_some()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_delay_doubles_from_250_ms_and_stops_at_30_s() {
    let delays: Vec<_> = (0..10).map(|attempt| backoff(attempt, 0.0).as_millis()).collect();
    assert_eq!(delays, [250, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]);
  }

  #[test]
  fn jitter_adds_at_most_a_quarter_of_the_delay() {
    assert_eq!(backoff(0, 0.999_999).as_millis(), 312);
    assert_eq!(backoff(7, 0.5).as_millis(), 33_750);
    assert_eq!(backoff(u32::MAX, 0.0).as_millis(), 30_000);
  }

  #[test]
  fn jitter_is_in_the_unit_interval() {
    assert!((0..1_000).map(|_| jitter()).all(|value| (0.0..1.0).contains(&value)));
  }
}
