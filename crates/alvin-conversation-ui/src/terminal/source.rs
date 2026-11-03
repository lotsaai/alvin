//! Where a terminal's frames come from.
//!
//! A frame source publishes snapshots and accepts input through its handle.

use std::io;
use std::sync::{Arc, Mutex};

use gpui_kit::base::async_util;

use crate::terminal::engine::{InputRejected, TerminalSnapshot};
use crate::terminal::frame::{CellSelection, CursorShape, TerminalColors, Viewport};
use crate::terminal::input::TerminalInput;

/// Where the engine publishes snapshots: a latest-wins slot plus a wake.
///
/// The UI awaits the receiver, takes the slot and applies it. Several
/// publishes between two wakes collapse into the latest snapshot.
#[derive(Clone)]
pub struct FrameSink {
  latest: Arc<Mutex<Option<Arc<TerminalSnapshot>>>>,
  wake: async_util::Sender<()>,
}

impl std::fmt::Debug for FrameSink {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("FrameSink").finish_non_exhaustive()
  }
}

impl FrameSink {
  /// A sink and the receiver the UI awaits.
  #[must_use]
  pub fn new() -> (Self, async_util::Receiver<()>) {
    let (wake, rx) = async_util::unbounded();
    (
      Self {
        latest: Arc::new(Mutex::new(None)),
        wake,
      },
      rx,
    )
  }

  /// Publish a snapshot and wake the UI.
  pub fn publish(&self, snapshot: Arc<TerminalSnapshot>) -> io::Result<()> {
    *self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot);
    if self.wake.is_empty() {
      self.wake.try_send(()).map_err(io::Error::other)?;
    }
    Ok(())
  }

  /// Take the latest snapshot, if one was published since the last take.
  pub fn take(&self) -> Option<Arc<TerminalSnapshot>> {
    self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
  }
}

/// What a source needs to start.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StartOptions {
  /// The initial grid.
  pub(crate) viewport: Viewport,
  /// The initial colors.
  pub(crate) colors: TerminalColors,
  /// Whether the terminal starts visible.
  pub(crate) visible: bool,
  /// The default cursor shape and blink, when the host overrides them.
  pub(crate) cursor: Option<(CursorShape, bool)>,
}

impl Default for StartOptions {
  fn default() -> Self {
    Self {
      viewport: Viewport::default(),
      colors: TerminalColors::default(),
      visible: true,
      cursor: None,
    }
  }
}

/// A producer of terminal snapshots.
pub trait FrameSource: 'static {
  /// Start publishing into `sink` and return the handle for input.
  fn start(self: Box<Self>, sink: FrameSink, options: StartOptions) -> io::Result<Box<dyn FrameHandle>>;
}

/// The UI's side of a running [`FrameSource`]. Nothing here may block.
pub trait FrameHandle: 'static {
  /// Queue input.
  fn input(&self, input: TerminalInput) -> Result<(), InputRejected>;
  /// The grid changed.
  fn resize(&self, viewport: Viewport);
  /// The terminal became visible or hidden.
  fn set_visible(&self, visible: bool);
  /// Change the terminal's default colors and cursor.
  fn set_appearance(&self, colors: TerminalColors, cursor: Option<(CursorShape, bool)>);
  /// The last frame was painted; the source may publish the next one.
  fn request_frame(&self);
  /// Ask for the selected text, which may reach scrollback the frame does
  /// not carry. `None` means the caller must read the frame instead.
  fn copy(&self, selection: CellSelection) -> Option<async_util::Receiver<io::Result<String>>>;
  /// Stop the source. Idempotent.
  fn close(&self);
  /// Take the owner thread for joining away from the UI thread.
  fn take_thread(&self) -> Option<std::thread::JoinHandle<()>>;
}
