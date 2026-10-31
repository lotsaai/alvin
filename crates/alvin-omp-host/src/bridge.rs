//! The blocking-to-async event pump.
//!
//! `omp-rpc` delivers events on a `std::sync::mpsc` channel fed by its reader thread. One thread per session drains that
//! channel, translates each frame and pushes it into a bounded `tokio::sync::mpsc` channel with `blocking_send`. A slow
//! consumer therefore makes this thread wait; nothing is dropped. The `omp-rpc` channel is unbounded, so a stalled
//! consumer costs memory, not events.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread::Builder;

use alvin_protocol::EventBody;
use tokio::sync::mpsc;

use crate::translate::{Translation, translate};
use crate::{Error, Result};

/// A running pump and the async end of its channel.
pub(crate) struct Bridge {
  /// Translated events, in the order omp sent them.
  pub(crate) events: mpsc::Receiver<EventBody>,
  /// How many frames the protocol does not expose have been seen.
  pub(crate) untranslated: Arc<AtomicU64>,
}

impl Bridge {
  /// Starts the pump for the session in `cwd`. `closing` is set before a deliberate teardown so the pump does not
  /// report it as the process exiting on its own.
  pub(crate) fn start(
    source: std_mpsc::Receiver<omp_rpc::Event>,
    capacity: NonZeroUsize,
    closing: Arc<AtomicBool>,
    cwd: &Path,
  ) -> Result<Self> {
    let (sender, events) = mpsc::channel(capacity.get());
    let untranslated = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&untranslated);
    let owner = cwd.to_path_buf();
    // Detached: the thread ends when omp-rpc's channel disconnects or the consumer drops `events`. Joining it could wait
    // forever on a consumer that holds a full channel without draining it.
    Builder::new()
      .name("alvin-omp-events".to_owned())
      .spawn(move || pump(&source, &sender, &counter, &closing, &owner))
      .map_err(|source| Error::Bridge {
        cwd: cwd.to_path_buf(),
        source,
      })?;
    Ok(Self { events, untranslated })
  }
}

fn pump(
  source: &std_mpsc::Receiver<omp_rpc::Event>,
  sender: &mpsc::Sender<EventBody>,
  untranslated: &AtomicU64,
  closing: &AtomicBool,
  cwd: &Path,
) {
  for event in source {
    match translate(event) {
      Translation::Event(body) => {
        if sender.blocking_send(body).is_err() {
          // The consumer is gone, so there is nobody to deliver to. omp-rpc keeps serving calls without a reader.
          return;
        }
      }
      Translation::Untranslated(frame) => {
        let seen = untranslated.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        tracing::debug!(cwd = %cwd.display(), kind = frame.kind(), seen, "omp frame not exposed by the protocol");
      }
    }
  }
  // omp-rpc's channel disconnects when its reader ends: omp closed its stdout, or the host closed the client.
  if !closing.load(Ordering::SeqCst) && sender.blocking_send(EventBody::ProcessExited).is_err() {
    tracing::debug!(cwd = %cwd.display(), "event consumer gone before the process-exited event");
  }
}
