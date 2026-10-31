//! The event pump: omp's frames in, protocol events out.
//!
//! The transport delivers events on an unbounded channel fed by its reader task. One task per session drains that
//! channel, translates each frame and pushes it into a bounded channel. A slow consumer therefore makes the task wait;
//! nothing is dropped. The transport's channel is unbounded, so a stalled consumer costs memory, not events.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use alvin_protocol::EventBody;
use tokio::sync::mpsc;

use crate::translate::translate;
use crate::transport::Event;

/// A running pump and the async end of its channel.
pub(crate) struct Bridge {
  /// Translated events, in the order omp sent them.
  pub(crate) events: mpsc::Receiver<EventBody>,
  /// How many frames the protocol does not expose have been seen.
  pub(crate) untranslated: Arc<AtomicU64>,
}

impl Bridge {
  /// Starts the pump for the session in `cwd`. `closing` is set before a deliberate teardown so the pump does not report
  /// the process exiting on its own.
  pub(crate) fn start(source: mpsc::UnboundedReceiver<Event>, capacity: NonZeroUsize, closing: Arc<AtomicBool>, cwd: PathBuf) -> Self {
    let (sender, events) = mpsc::channel(capacity.get());
    let untranslated = Arc::new(AtomicU64::new(0));
    // Detached: the task ends when the transport's channel closes or the consumer drops `events`.
    drop(tokio::spawn(pump(source, sender, Arc::clone(&untranslated), closing, cwd)));
    Self { events, untranslated }
  }
}

async fn pump(
  mut source: mpsc::UnboundedReceiver<Event>,
  sender: mpsc::Sender<EventBody>,
  untranslated: Arc<AtomicU64>,
  closing: Arc<AtomicBool>,
  cwd: PathBuf,
) {
  while let Some(event) = source.recv().await {
    match translate(event) {
      Ok(body) => {
        if sender.send(body).await.is_err() {
          // The consumer is gone; nothing is listening for the rest.
          return;
        }
      }
      Err(frame) => {
        let seen = untranslated.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        tracing::debug!(cwd = %cwd.display(), kind = frame.kind(), seen, "omp frame not exposed by protocol");
      }
    }
  }
  // The transport's channel closes when its reader ends: omp closed its stdout, or the host closed the client.
  if !closing.load(Ordering::SeqCst) && sender.send(EventBody::ProcessExited).await.is_err() {
    tracing::debug!(cwd = %cwd.display(), "event consumer gone before the process-exited event");
  }
}
