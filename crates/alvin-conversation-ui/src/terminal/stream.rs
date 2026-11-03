//! Ordered transport between the gateway stream and the retained VT engine.
//!
//! [`StreamPeer::output`] applies backpressure before accepting an event;
//! [`StreamPeer::take_events`] gives the transport its sole input receiver.

use std::io;
use std::sync::{Arc, Mutex};

use gpui_kit::base::async_util;

use crate::terminal::engine::{ByteHandle, ByteSink, ByteSource, ExitStatus};
use crate::terminal::frame::Viewport;

/// What the terminal sends to the application's side of a
/// [`StreamSource`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamEvent {
  /// Bytes for the program: keys, pastes and replies to its queries,
  /// already encoded for the terminal modes the program set.
  Input(Vec<u8>),
  /// The grid changed. The first event is the grid the terminal
  /// started with.
  Resize(Viewport),
  /// The terminal closed. Stop the program.
  Closed,
}

/// A [`ByteSource`] fed by its [`StreamPeer`].
#[derive(Debug)]
pub struct StreamSource {
  shared: Arc<Shared>,
  ready: async_util::Sender<ByteSink>,
}

/// The application's side of a [`StreamSource`].
#[derive(Debug)]
pub struct StreamPeer {
  shared: Arc<Shared>,
  events: Option<async_util::Receiver<StreamEvent>>,
  ready: async_util::Receiver<ByteSink>,
  sink: Option<ByteSink>,
}
/// Receiver for serialized input and resize events.
#[derive(Debug)]
pub struct StreamEvents {
  shared: Arc<Shared>,
  receiver: async_util::Receiver<StreamEvent>,
}

impl StreamEvents {
  /// Receive the next event and permit the engine to send another.
  pub async fn recv(&self) -> io::Result<StreamEvent> {
    let event = self.receiver.recv().await.map_err(io::Error::other)?;
    let mut state = self.shared.lock();
    state.outstanding = false;
    if let Some(viewport) = state.pending_resize.take() {
      self
        .shared
        .events
        .try_send(StreamEvent::Resize(viewport))
        .map_err(io::Error::other)?;
      state.outstanding = true;
    } else if let Some(sink) = &state.sink {
      sink.writable();
    }
    Ok(event)
  }
}

#[derive(Debug)]
struct Shared {
  state: Mutex<State>,
  events: async_util::Sender<StreamEvent>,
}

#[derive(Debug, Default)]
struct State {
  sink: Option<ByteSink>,
  exit: Option<ExitStatus>,
  outstanding: bool,
  pending_resize: Option<Viewport>,
}

impl Shared {
  fn lock(&self) -> std::sync::MutexGuard<'_, State> {
    self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
  }
}

impl StreamSource {
  /// A source and the peer that feeds it.
  #[must_use]
  pub fn new() -> (Self, StreamPeer) {
    let (events, receiver) = async_util::unbounded();
    let (ready, ready_receiver) = async_util::unbounded();
    let shared = Arc::new(Shared {
      state: Mutex::new(State::default()),
      events,
    });
    (
      Self {
        shared: shared.clone(),
        ready,
      },
      StreamPeer {
        shared,
        events: Some(receiver),
        ready: ready_receiver,
        sink: None,
      },
    )
  }
}

impl StreamPeer {
  /// Enqueue one ordered output event into the retained engine.
  /// Await each call before acknowledging the event to the gateway.
  pub async fn output(&mut self, bytes: Vec<u8>) -> io::Result<()> {
    if self.sink.is_none() {
      self.sink = Some(self.ready.recv().await.map_err(io::Error::other)?);
    }
    let sink = self
      .sink
      .as_ref()
      .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal engine did not start"))?;
    sink.write(bytes).await
  }

  /// Reports that the program ended; the terminal shows its status.
  pub fn exit(&self, status: ExitStatus) {
    let mut state = self.shared.lock();
    state.exit = Some(status);
    if let Some(sink) = &state.sink {
      sink.finished();
    }
  }

  /// Take the single receiver for ordered terminal events.
  pub fn take_events(&mut self) -> io::Result<StreamEvents> {
    let receiver = self
      .events
      .take()
      .ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "terminal event receiver already taken"))?;
    Ok(StreamEvents {
      shared: self.shared.clone(),
      receiver,
    })
  }
}

/// The engine's side of a started [`StreamSource`].
struct Handle {
  shared: Arc<Shared>,
}

impl ByteSource for StreamSource {
  fn start(self: Box<Self>, sink: ByteSink, _viewport: Viewport) -> io::Result<Box<dyn ByteHandle>> {
    {
      let mut state = self.shared.lock();
      if state.exit.is_some() {
        sink.finished();
      }
      state.sink = Some(sink.clone());
    }
    self.ready.try_send(sink).map_err(io::Error::other)?;
    Ok(Box::new(Handle { shared: self.shared }))
  }
}

impl ByteHandle for Handle {
  fn write(&self, bytes: &[u8]) -> io::Result<usize> {
    let mut state = self.shared.lock();
    if state.outstanding {
      return Ok(0);
    }
    self
      .shared
      .events
      .try_send(StreamEvent::Input(bytes.to_vec()))
      .map_err(io::Error::other)?;
    state.outstanding = true;
    Ok(bytes.len())
  }

  fn resize(&self, viewport: Viewport) -> io::Result<()> {
    let mut state = self.shared.lock();
    if state.outstanding {
      state.pending_resize = Some(viewport);
      return Ok(());
    }
    self
      .shared
      .events
      .try_send(StreamEvent::Resize(viewport))
      .map_err(io::Error::other)?;
    state.outstanding = true;
    Ok(())
  }

  fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
    Ok(self.shared.lock().exit.clone())
  }

  fn close(&self) -> io::Result<()> {
    let result = self.shared.events.try_send(StreamEvent::Closed).map_err(io::Error::other);
    self.shared.lock().sink = None;
    result
  }
}

#[cfg(test)]
mod tests {
  use crate::terminal::engine::{Engine, InputRejected, TerminalStatus};
  use crate::terminal::frame::Viewport;
  use crate::terminal::input::TerminalInput;
  use crate::terminal::options::EngineOptions;
  use crate::terminal::source::{FrameSink, FrameSource, StartOptions};
  use crate::terminal::stream::{StreamEvent, StreamSource};

  #[tokio::test]
  async fn stream_renders_unicode_and_encodes_bracketed_paste() {
    let (source, mut peer) = StreamSource::new();
    let events = peer.take_events().expect("terminal event receiver");
    let (sink, wake) = FrameSink::new();
    let handle = Box::new(Engine::new(source, EngineOptions::default()))
      .start(sink.clone(), StartOptions::default())
      .expect("start terminal engine");
    let viewport = Viewport::new(20, 4, 8, 16).expect("valid viewport");
    handle.resize(viewport);
    assert_eq!(events.recv().await.expect("measured resize"), StreamEvent::Resize(viewport));
    wake.recv().await.expect("initial terminal frame");
    sink.take().expect("initial snapshot");
    handle.request_frame();
    peer.output("\x1b[?2004h你好".as_bytes().to_vec()).await.expect("enqueue output");
    loop {
      wake.recv().await.expect("terminal frame");
      if let Some(snapshot) = sink.take() {
        if snapshot.frame.modes.bracketed_paste && snapshot.frame.rows.iter().any(|row| row.text.contains("你好")) {
          break;
        }
        handle.request_frame();
      }
    }
    handle.input(TerminalInput::Paste("one\ntwo".to_owned())).expect("queue paste");
    assert_eq!(
      events.recv().await.expect("terminal input"),
      StreamEvent::Input(b"\x1b[200~one\ntwo\x1b[201~".to_vec())
    );
    handle.close();
    assert_eq!(handle.input(TerminalInput::Text("late".into())), Err(InputRejected::Closed));
    let thread = handle.take_thread().expect("engine thread");
    tokio::task::spawn_blocking(move || thread.join())
      .await
      .expect("join task")
      .expect("engine thread");
  }

  #[tokio::test]
  async fn losing_event_consumer_reports_failure() {
    let (source, mut peer) = StreamSource::new();
    let events = peer.take_events().expect("terminal event receiver");
    let (sink, wake) = FrameSink::new();
    let handle = Box::new(Engine::new(source, EngineOptions::default()))
      .start(sink.clone(), StartOptions::default())
      .expect("start terminal engine");
    drop(events);
    handle.resize(Viewport::new(20, 4, 8, 16).expect("valid viewport"));
    loop {
      wake.recv().await.expect("terminal state");
      if let Some(snapshot) = sink.take()
        && matches!(snapshot.status, TerminalStatus::Failed(_))
      {
        break;
      }
    }
    handle.close();
    let thread = handle.take_thread().expect("engine thread");
    tokio::task::spawn_blocking(move || thread.join())
      .await
      .expect("join task")
      .expect("engine thread");
  }
}
