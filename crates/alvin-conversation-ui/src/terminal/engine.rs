//! The owner thread: one thread per terminal that owns the Ghostty state,
//! parses output, encodes input and publishes frames.
//!
//! The thread sleeps on its mailbox. It wakes for output from the byte
//! source, for input, resizes and visibility changes, and for its timers
//! (scrollback compression, the render hold watchdog). A frame is built
//! only when the UI has granted a credit, so a terminal that is not painted
//! stops producing frames while it keeps parsing.
//!
//! Derived from Herdr (Apache-2.0).

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::base::async_util;

use crate::terminal::core::{Core, Effects};
use crate::terminal::frame::{CellSelection, CursorShape, TerminalColors, TerminalFrame, Viewport};
use crate::terminal::input::TerminalInput;
use crate::terminal::options::EngineOptions;
use crate::terminal::source::{FrameHandle, FrameSink, FrameSource, StartOptions};

const INPUT_BYTES: usize = 1 << 20;
const INPUT_COMMANDS: usize = 256;
const WRITE_CHUNK: usize = 8192;
/// The most input written in one turn, so output keeps flowing during a
/// large paste to a program that reads as fast as it comes.
const WRITE_BURST: usize = 64 * 1024;
/// While a write waits for room, how often the engine tries again even
/// without [`ByteSink::writable`], for a source that never calls it.
const WRITE_RETRY: Duration = Duration::from_millis(50);
/// The output a source may queue ahead of the parser before backpressure.
const READ_QUEUE_BYTES: usize = 1 << 20;
const COMPRESSION_IDLE: Duration = Duration::from_millis(250);
const COMPRESSION_STEP: Duration = Duration::from_millis(16);
/// How long after output the engine asks its source for the foreground
/// process. Output is what a new program or a return to the prompt
/// produces, so the check needs no timer while the terminal is quiet.
const FOREGROUND_CHECK: Duration = Duration::from_millis(50);

/// How a program ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExitStatus {
  pub(crate) code: Option<i32>,
  pub(crate) signal: Option<String>,
}

impl ExitStatus {
  /// An exit with `code`, or by `signal` when a signal ended the program.
  pub fn new(code: Option<i32>, signal: Option<impl Into<String>>) -> Self {
    Self {
      code,
      signal: signal.map(Into::into),
    }
  }
}

/// The working directory reported by a foreground process.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ForegroundProcess {
  pub(crate) cwd: Option<PathBuf>,
}

impl ForegroundProcess {
  /// The working directory, when it could be read.
  pub fn cwd(&self) -> Option<&std::path::Path> {
    self.cwd.as_deref()
  }
}

/// The state of a terminal's program.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub enum TerminalStatus {
  /// The program has not started yet.
  #[default]
  Starting,
  /// The program is running.
  Live,
  /// The program ended.
  Exited(ExitStatus),
  /// The terminal could not start or the engine failed.
  Failed(Arc<io::Error>),
}

impl PartialEq for TerminalStatus {
  fn eq(&self, other: &Self) -> bool {
    match (self, other) {
      (Self::Starting, Self::Starting) | (Self::Live, Self::Live) => true,
      (Self::Exited(left), Self::Exited(right)) => left == right,
      (Self::Failed(left), Self::Failed(right)) => Arc::ptr_eq(left, right),
      _ => false,
    }
  }
}

impl Eq for TerminalStatus {}

/// The latest published terminal state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TerminalSnapshot {
  /// The frame.
  pub frame: Arc<TerminalFrame>,
  /// The program's state.
  pub status: TerminalStatus,
  /// The title the program set.
  pub title: String,
  /// The working directory the program reported.
  pub cwd: Option<PathBuf>,
  /// Bells received so far.
  pub bell_count: u64,
  /// Increments per clipboard write.
  pub clipboard_sequence: u64,
  /// The last clipboard write.
  pub clipboard: Option<Arc<str>>,
  /// The process in the foreground, when the source reports one.
  pub foreground: Option<ForegroundProcess>,
}

/// A source of terminal output that also accepts the program's input.
///
/// The engine calls [`ByteSource::start`] on its owner thread and pumps
/// everything the source writes to the [`ByteSink`] through the terminal.
pub trait ByteSource: Send + 'static {
  /// Start producing output into `sink` at `viewport` and return the
  /// handle for input.
  fn start(self: Box<Self>, sink: ByteSink, viewport: Viewport) -> io::Result<Box<dyn ByteHandle>>;
}

/// The engine's side of a byte source.
pub trait ByteHandle: Send {
  /// Write input for the program without blocking: as much of `bytes` as
  /// it takes now, and return how much that was. When that is less than
  /// all of it, the source calls [`ByteSink::writable`] once more fits,
  /// and the engine writes the rest then, ahead of newer input.
  fn write(&self, bytes: &[u8]) -> io::Result<usize>;
  /// The grid changed.
  fn resize(&self, viewport: Viewport) -> io::Result<()>;
  /// Whether the program ended, and how.
  fn try_wait(&self) -> io::Result<Option<ExitStatus>>;
  /// Stop the program and release the source.
  fn close(&self) -> io::Result<()>;
  /// The process in the foreground, if the source knows it. The engine
  /// asks on its own thread, at most every 50 ms and only after output,
  /// so it may read the process table but must not block.
  fn foreground(&self) -> Option<ForegroundProcess> {
    None
  }
}

/// Where a [`ByteSource`] writes its output.
#[derive(Clone, Debug)]
pub struct ByteSink {
  shared: Arc<Shared>,
}

impl ByteSink {
  /// Enqueue one ordered output event, waiting asynchronously for room.
  /// Successful return commits bytes to the retained engine mailbox.
  pub async fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
    loop {
      {
        let mut mail = self.shared.lock();
        if self.shared.closing.load(Ordering::Acquire) {
          return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal engine is closed"));
        }
        if mail.read_bytes == 0 || bytes.len() <= READ_QUEUE_BYTES.saturating_sub(mail.read_bytes) {
          mail.read_bytes = mail.read_bytes.saturating_add(bytes.len());
          mail.reads.push_back(bytes);
          mail.generation = mail.generation.wrapping_add(1);
          self.shared.wake.notify_one();
          return Ok(());
        }
      }
      self.shared.room_receiver.recv().await.map_err(io::Error::other)?;
      self.shared.lock().room_notified = false;
    }
  }

  /// The program can take input again after a short
  /// [`ByteHandle::write`]; wakes the engine to write the rest.
  pub fn writable(&self) {
    let mut mail = self.shared.lock();
    mail.writable = true;
    self.shared.bump(&mut mail);
  }

  /// The source reached end of file.
  pub fn finished(&self) {
    let mut mail = self.shared.lock();
    mail.reader_exited = true;
    mail.generation = mail.generation.wrapping_add(1);
    self.shared.wake.notify_one();
  }
}

/// Why an input was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputRejected {
  /// The bounded input queue is full; try again after a frame.
  QueueFull,
  /// The terminal is closed.
  Closed,
}

impl std::fmt::Display for InputRejected {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::QueueFull => f.write_str("terminal input queue is full"),
      Self::Closed => f.write_str("terminal is closed"),
    }
  }
}

impl std::error::Error for InputRejected {}

#[derive(Default)]
struct Mailbox {
  commands: VecDeque<TerminalInput>,
  bytes: usize,
  reads: VecDeque<Vec<u8>>,
  /// The bytes in `reads`.
  read_bytes: usize,
  room_notified: bool,
  /// The source said it can take input again.
  writable: bool,
  viewport: Option<Viewport>,
  visible: bool,
  credit: bool,
  /// The UI painted since the owner last looked, which counts as use.
  painted: bool,
  reader_exited: bool,
  reveal: bool,
  copy: Option<(CellSelection, async_util::Sender<io::Result<String>>)>,
  cursor: Option<(CursorShape, bool)>,
  colors: Option<TerminalColors>,
  generation: u64,
}

#[derive(Debug)]
struct Shared {
  mailbox: Mutex<Mailbox>,
  wake: Condvar,
  room_sender: async_util::Sender<()>,
  room_receiver: async_util::Receiver<()>,
  closing: AtomicBool,
}

impl std::fmt::Debug for Mailbox {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Mailbox")
      .field("commands", &self.commands.len())
      .field("reads", &self.reads.len())
      .finish_non_exhaustive()
  }
}

impl Shared {
  fn lock(&self) -> std::sync::MutexGuard<'_, Mailbox> {
    self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
  }

  fn bump(&self, mail: &mut Mailbox) {
    mail.generation = mail.generation.wrapping_add(1);
    self.wake.notify_one();
  }

  /// Marks the terminal closing and frees a source waiting for room.
  fn set_closing(&self) {
    self.closing.store(true, Ordering::Release);
    self.room_sender.close();
  }
}

/// The UI's handle to a running engine.
///
/// Every method is cheap and never blocks on the owner thread.
#[derive(Clone, Debug)]
pub(crate) struct EngineHandle {
  shared: Arc<Shared>,
  thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl EngineHandle {
  /// Queue input in order.
  pub fn input(&self, input: TerminalInput) -> Result<(), InputRejected> {
    let mut mail = self.shared.lock();
    if self.shared.closing.load(Ordering::Acquire) {
      return Err(InputRejected::Closed);
    }
    if mail.reader_exited && !matches!(input, TerminalInput::Scroll(_)) {
      return Err(InputRejected::Closed);
    }
    let budget = input.budget();
    if mail.commands.len() >= INPUT_COMMANDS || (mail.bytes != 0 && budget > INPUT_BYTES.saturating_sub(mail.bytes)) {
      return Err(InputRejected::QueueFull);
    }
    mail.bytes = mail.bytes.saturating_add(budget);
    mail.commands.push_back(input);
    self.shared.bump(&mut mail);
    Ok(())
  }

  /// Change the grid.
  pub fn resize(&self, viewport: Viewport) {
    let mut mail = self.shared.lock();
    mail.viewport = Some(viewport);
    self.shared.bump(&mut mail);
  }

  /// Tell the engine whether the terminal is painted. A hidden terminal
  /// keeps parsing but builds no frames; revealing it builds one at once.
  pub fn set_visible(&self, visible: bool) {
    let mut mail = self.shared.lock();
    if mail.visible != visible {
      mail.visible = visible;
      mail.credit = visible;
      mail.reveal = visible;
      self.shared.bump(&mut mail);
    }
  }

  /// Grant one frame of credit. The UI calls this after it painted the
  /// last frame.
  pub fn request_frame(&self) {
    let mut mail = self.shared.lock();
    if !mail.credit || !mail.painted {
      mail.credit = true;
      mail.painted = true;
      self.shared.bump(&mut mail);
    }
  }

  /// Ask the owner thread for the selected text. The answer arrives on the
  /// returned receiver.
  pub fn copy(&self, selection: CellSelection) -> async_util::Receiver<io::Result<String>> {
    let (tx, rx) = async_util::unbounded();
    let mut mail = self.shared.lock();
    mail.copy = Some((selection, tx));
    self.shared.bump(&mut mail);
    rx
  }

  /// Stop the program and the owner thread. Idempotent.
  pub fn close(&self) {
    let mut mail = self.shared.lock();
    self.shared.set_closing();
    self.shared.bump(&mut mail);
  }
}

/// Everything needed to start an engine.
#[derive(Debug)]
pub(crate) struct EngineConfig {
  /// Engine limits.
  pub options: EngineOptions,
  /// The initial grid.
  pub viewport: Viewport,
  /// The initial colors.
  pub colors: TerminalColors,
  /// Whether the terminal starts visible.
  pub visible: bool,
  /// The default cursor.
  pub cursor: Option<(CursorShape, bool)>,
  /// Bytes written to the program right after it starts.
  pub input: Vec<u8>,
}

impl Default for EngineConfig {
  fn default() -> Self {
    Self {
      options: EngineOptions::default(),
      viewport: Viewport::default(),
      colors: TerminalColors::default(),
      visible: true,
      cursor: None,
      input: Vec::new(),
    }
  }
}

/// Start an owner thread for `source`, publishing to `sink`.
///
/// The first snapshot is published as soon as the source started.
pub(crate) fn spawn(source: Box<dyn ByteSource>, config: EngineConfig, sink: FrameSink) -> io::Result<EngineHandle> {
  let (room_sender, room_receiver) = async_util::unbounded();
  let shared = Arc::new(Shared {
    // The first frame is published as the source starts, so there is no
    // credit until the UI paints it, the same as for every later frame.
    mailbox: Mutex::new(Mailbox {
      visible: config.visible,
      credit: false,
      ..Mailbox::default()
    }),
    wake: Condvar::new(),
    room_sender,
    room_receiver,
    closing: AtomicBool::new(false),
  });
  let owner_shared = shared.clone();
  let byte_sink = ByteSink { shared: shared.clone() };
  let thread = std::thread::Builder::new().name("alvin-terminal".to_owned()).spawn(move || {
    let mut owner = match Owner::start(source, byte_sink, config, owner_shared.clone(), &sink) {
      Ok(owner) => owner,
      Err(error) => {
        owner_shared.set_closing();
        owner_shared.lock().reads.clear();
        if sink
          .publish(Arc::new(TerminalSnapshot {
            status: TerminalStatus::Failed(Arc::new(error)),
            ..TerminalSnapshot::default()
          }))
          .is_err()
        {
          owner_shared.set_closing();
        }
        return;
      }
    };
    owner.run();
  })?;
  Ok(EngineHandle {
    shared,
    thread: Arc::new(Mutex::new(Some(thread))),
  })
}

/// A [`FrameSource`] that runs a [`ByteSource`] through the engine.
#[derive(Debug)]
pub struct Engine<S> {
  source: S,
  options: EngineOptions,
  input: Vec<u8>,
}

impl<S: ByteSource> Engine<S> {
  /// An engine for `source` with `options`.
  #[must_use]
  pub fn new(source: S, options: EngineOptions) -> Self {
    Self {
      source,
      options,
      input: Vec::new(),
    }
  }
}

impl<S: ByteSource> FrameSource for Engine<S> {
  fn start(self: Box<Self>, sink: FrameSink, options: StartOptions) -> io::Result<Box<dyn FrameHandle>> {
    let handle = spawn(
      Box::new(self.source),
      EngineConfig {
        options: self.options,
        viewport: options.viewport,
        colors: options.colors,
        visible: options.visible,
        cursor: options.cursor,
        input: self.input,
      },
      sink,
    )?;
    Ok(Box::new(handle))
  }
}

impl FrameHandle for EngineHandle {
  fn input(&self, input: TerminalInput) -> Result<(), InputRejected> {
    Self::input(self, input)
  }

  fn resize(&self, viewport: Viewport) {
    Self::resize(self, viewport);
  }

  fn set_visible(&self, visible: bool) {
    Self::set_visible(self, visible);
  }

  fn set_appearance(&self, colors: TerminalColors, cursor: Option<(CursorShape, bool)>) {
    let mut mail = self.shared.lock();
    mail.colors = Some(colors);
    mail.cursor = cursor;
    self.shared.bump(&mut mail);
  }

  fn request_frame(&self) {
    Self::request_frame(self);
  }

  fn copy(&self, selection: CellSelection) -> Option<async_util::Receiver<io::Result<String>>> {
    Some(Self::copy(self, selection))
  }

  fn close(&self) {
    Self::close(self);
  }

  fn take_thread(&self) -> Option<std::thread::JoinHandle<()>> {
    self.thread.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
  }
}

struct PendingWrite {
  bytes: Vec<u8>,
  offset: usize,
  budget: usize,
}

struct Owner {
  shared: Arc<Shared>,
  core: Core,
  handle: Box<dyn ByteHandle>,
  sink: FrameSink,
  snapshot: TerminalSnapshot,
  /// Bytes that go ahead of input: replies to the program's queries and
  /// interrupts.
  urgent: Vec<u8>,
  pending: Option<PendingWrite>,
  /// The source took less than it was given and has not said it has
  /// room again.
  write_blocked: bool,
  exit: Option<ExitStatus>,
  compression_deadline: Option<Instant>,
  /// When to ask the source for the foreground process again.
  foreground_due: Option<Instant>,
  dirty: bool,
  force: bool,
  effects: Effects,
}

impl Owner {
  fn start(
    source: Box<dyn ByteSource>,
    byte_sink: ByteSink,
    config: EngineConfig,
    shared: Arc<Shared>,
    sink: &FrameSink,
  ) -> io::Result<Self> {
    let mut core = Core::new(config.viewport, &config.options, config.colors)?;
    if let Some((shape, blinking)) = config.cursor {
      core.set_cursor(shape, blinking)?;
    }
    let frame = core
      .frame(true)?
      .ok_or_else(|| io::Error::other("initial terminal frame missing"))?;
    let handle = source.start(byte_sink, config.viewport)?;
    let snapshot = TerminalSnapshot {
      frame,
      status: TerminalStatus::Live,
      ..TerminalSnapshot::default()
    };
    sink.publish(Arc::new(snapshot.clone()))?;
    Ok(Self {
      shared,
      core,
      handle,
      sink: sink.clone(),
      snapshot,
      urgent: config.input.clone(),
      pending: None,
      write_blocked: false,
      exit: None,
      compression_deadline: None,
      foreground_due: Some(Instant::now() + FOREGROUND_CHECK),
      dirty: true,
      force: false,
      effects: Effects::default(),
    })
  }

  fn run(&mut self) {
    while !self.shared.closing.load(Ordering::Acquire) {
      if let Err(error) = self.turn() {
        self.snapshot.status = TerminalStatus::Failed(Arc::new(error));
        if self.publish().is_err() {
          self.shared.set_closing();
        }
        break;
      }
    }
    self.cleanup();
  }

  fn turn(&mut self) -> io::Result<()> {
    let (reads, viewport, command, visible, credit, eof, reveal, copy, cursor, colors, _painted, generation) = {
      let mut mail = self.shared.lock();
      let command = if self.pending.is_none() { mail.commands.pop_front() } else { None };
      if std::mem::take(&mut mail.writable) {
        self.write_blocked = false;
      }
      let reads = if self.write_blocked && !self.urgent.is_empty() {
        VecDeque::new()
      } else {
        let reads = std::mem::take(&mut mail.reads);
        if !reads.is_empty() {
          mail.read_bytes = 0;
          if !mail.room_notified && self.shared.room_sender.try_send(()).is_ok() {
            mail.room_notified = true;
          }
        }
        reads
      };
      (
        reads,
        mail.viewport.take(),
        command,
        mail.visible,
        mail.credit,
        mail.reader_exited && mail.reads.is_empty(),
        std::mem::take(&mut mail.reveal),
        mail.copy.take(),
        mail.cursor.take(),
        mail.colors.take(),
        std::mem::take(&mut mail.painted),
        mail.generation,
      )
    };
    let had_work =
      !reads.is_empty() || viewport.is_some() || command.is_some() || reveal || copy.is_some() || cursor.is_some() || colors.is_some();
    if reveal {
      self.dirty = true;
      self.force = true;
    }
    if let Some((shape, blinking)) = cursor {
      self.core.set_cursor(shape, blinking)?;
      self.dirty = true;
    }
    if let Some(colors) = colors {
      self.core.set_colors(colors)?;
      self.dirty = true;
      self.force = true;
    }
    if !reads.is_empty() {
      for chunk in reads {
        self.core.write(&chunk, &mut self.effects)?;
      }
      self.apply_effects()?;
      self.compression_deadline = Some(Instant::now() + COMPRESSION_IDLE);
      self.foreground_due.get_or_insert_with(|| Instant::now() + FOREGROUND_CHECK);
      self.dirty = true;
    }
    if let Some(viewport) = viewport {
      self.core.resize(viewport, &mut self.effects)?;
      self.handle.resize(viewport)?;
      self.apply_effects()?;
      self.dirty = true;
      self.force = true;
    }
    if let Some(input) = command {
      let budget = input.budget();
      if self.exit.is_some() && !matches!(input, TerminalInput::Scroll(_)) {
        self.release(budget);
        return Err(io::Error::new(
          io::ErrorKind::BrokenPipe,
          "terminal exited before queued input was delivered",
        ));
      } else {
        match self.core.input(input) {
          Ok(bytes) if !bytes.is_empty() => {
            self.pending = Some(PendingWrite { bytes, offset: 0, budget });
          }
          Ok(_) => self.release(budget),
          Err(error) => {
            self.release(budget);
            return Err(error);
          }
        }
        self.dirty = true;
      }
    }
    if let Some((selection, reply)) = copy {
      reply.try_send(self.core.copy(selection)).map_err(io::Error::other)?;
    }
    self.flush()?;
    if eof
      && self.exit.is_none()
      && let Some(status) = self.handle.try_wait()?
    {
      self.exit = Some(status.clone());
      self.snapshot.status = TerminalStatus::Exited(status);
      self.dirty = true;
      self.force = true;
    }
    if self.foreground_due.is_some_and(|due| Instant::now() >= due) {
      self.foreground_due = None;
      let foreground = if self.exit.is_some() { None } else { self.handle.foreground() };
      if foreground != self.snapshot.foreground {
        self.snapshot.foreground = foreground;
        // Published with the frame already shown, so a hidden or
        // unpainted terminal reports it too.
        self.publish()?;
      }
    }
    let exited = self.exit.is_some();
    let held = self.core.held()?;
    if self.dirty && ((visible && credit && !held) || (exited && self.force)) {
      if let Some(frame) = self.core.frame(self.force)? {
        self.snapshot.frame = frame;
        self.shared.lock().credit = false;
        self.publish()?;
      }
      self.dirty = false;
      self.force = false;
    }
    if self.compression_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
      let complete = self.core.compress();
      self.compression_deadline = (!complete).then(|| Instant::now() + COMPRESSION_STEP);
    }
    let unsent = self.pending.is_some() || !self.urgent.is_empty();
    if !had_work && !(unsent && !self.write_blocked) {
      let delay = if unsent {
        Some(WRITE_RETRY)
      } else if self.dirty && visible && credit {
        self.core.hold_remaining().or(Some(Duration::from_millis(1)))
      } else if let Some(deadline) = self.compression_deadline {
        Some(deadline.saturating_duration_since(Instant::now()))
      } else if eof && self.exit.is_none() {
        Some(Duration::from_millis(20))
      } else {
        None
      };
      let delay = match self.foreground_due {
        Some(due) => {
          let until = due.saturating_duration_since(Instant::now());
          Some(delay.map_or(until, |delay| delay.min(until)))
        }
        None => delay,
      };
      self.wait(generation, delay);
    }
    Ok(())
  }

  /// Sleeps until the mailbox changes, or `delay` passes.
  fn wait(&self, generation: u64, delay: Option<Duration>) {
    let mail = self.shared.lock();
    if mail.generation == generation && !self.shared.closing.load(Ordering::Acquire) {
      match delay {
        None => drop(self.shared.wake.wait(mail).unwrap_or_else(std::sync::PoisonError::into_inner)),
        Some(delay) => drop(
          self
            .shared
            .wake
            .wait_timeout(mail, delay.max(Duration::from_millis(1)))
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        ),
      }
    }
  }

  fn release(&self, budget: usize) {
    let mut mail = self.shared.lock();
    mail.bytes = mail.bytes.saturating_sub(budget);
  }

  /// Writes what the program takes now: the urgent bytes, then up to
  /// [`WRITE_BURST`] of the pending input, resuming where the last short
  /// write stopped.
  fn flush(&mut self) -> io::Result<()> {
    if self.write_blocked {
      return Ok(());
    }
    if !self.urgent.is_empty() {
      let written = self.handle.write(&self.urgent)?;
      self.urgent.drain(..written.min(self.urgent.len()));
      if !self.urgent.is_empty() {
        self.write_blocked = true;
        return Ok(());
      }
    }
    let mut burst = WRITE_BURST;
    while burst > 0 {
      let Some(pending) = &mut self.pending else {
        break;
      };
      let end = pending.offset.saturating_add(WRITE_CHUNK.min(burst)).min(pending.bytes.len());
      let bytes = pending
        .bytes
        .get(pending.offset..end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid terminal input offset"))?;
      let offered = bytes.len();
      let written = self.handle.write(bytes)?;
      if written > offered {
        return Err(io::Error::new(
          io::ErrorKind::InvalidData,
          "terminal input write exceeds offered bytes",
        ));
      }
      pending.offset = pending.offset.saturating_add(written);
      burst = burst.saturating_sub(written);
      if pending.offset >= pending.bytes.len() {
        let budget = pending.budget;
        self.pending = None;
        self.release(budget);
        break;
      }
      if written < offered {
        self.write_blocked = true;
        break;
      }
    }
    Ok(())
  }

  fn apply_effects(&mut self) -> io::Result<()> {
    let effects = std::mem::take(&mut self.effects);
    self.urgent.extend_from_slice(&effects.replies);
    let mut changed = false;
    if effects.title_changed {
      self.snapshot.title = self.core.title().to_owned();
      changed = true;
    }
    if effects.cwd_changed {
      self.snapshot.cwd = self.core.cwd().cloned();
      changed = true;
    }
    if effects.bells > 0 {
      self.snapshot.bell_count = self.snapshot.bell_count.saturating_add(effects.bells);
      changed = true;
    }
    if let Some(text) = effects.clipboard.last() {
      self.snapshot.clipboard_sequence = self
        .snapshot
        .clipboard_sequence
        .saturating_add(u64::try_from(effects.clipboard.len()).unwrap_or(u64::MAX));
      self.snapshot.clipboard = Some(Arc::from(text.as_str()));
      changed = true;
    }
    if changed {
      self.dirty = true;
      self.force = true;
    }
    Ok(())
  }

  fn publish(&self) -> io::Result<()> {
    self.sink.publish(Arc::new(self.snapshot.clone()))
  }

  fn cleanup(&mut self) {
    self.shared.set_closing();
    {
      let mut mail = self.shared.lock();
      mail.commands.clear();
      mail.bytes = 0;
      mail.reads.clear();
      mail.read_bytes = 0;
    }
    self.pending = None;
    if let Err(error) = self.handle.close()
      && !matches!(self.snapshot.status, TerminalStatus::Failed(_))
    {
      self.snapshot.status = TerminalStatus::Failed(Arc::new(error));
    }
    if self.publish().is_err() {
      self.shared.set_closing();
    }
  }
}
