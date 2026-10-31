//! Live sessions: the registry of omp processes, each session's event log, and the pump that fills it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use alvin_omp_host::Session;
use alvin_protocol::{Event, EventBody, EventId, LiveSessionId, LiveSessionSummary};
use jiff::Timestamp;
use tokio::sync::{broadcast, mpsc};

use crate::telemetry::Metrics;
use crate::{Error, Result};

/// Events a subscriber may fall behind by before its subscription ends.
const BROADCAST_CAPACITY: usize = 1024;

/// How long closing waits for a command in flight to return after aborting it.
const IDLE_GRACE: Duration = Duration::from_secs(5);

/// One live session: an omp process, where it runs, the file it writes, and its event log.
#[derive(Debug)]
pub(crate) struct Live {
  pub(crate) id: LiveSessionId,
  pub(crate) cwd: PathBuf,
  /// The session file omp writes. A fork or branch moves the session onto another file, so it is behind a lock.
  file: Mutex<PathBuf>,
  pub(crate) session: Session,
  pub(crate) log: Arc<EventLog>,
}

impl Live {
  pub(crate) fn new(id: LiveSessionId, cwd: PathBuf, file: PathBuf, session: Session, log: Arc<EventLog>) -> Self {
    Self {
      id,
      cwd,
      file: Mutex::new(file),
      session,
      log,
    }
  }

  /// The session file omp writes now.
  pub(crate) fn file(&self) -> PathBuf {
    self.file.lock().unwrap_or_else(PoisonError::into_inner).clone()
  }

  pub(crate) fn summary(&self) -> LiveSessionSummary {
    LiveSessionSummary {
      live: self.id,
      cwd: self.cwd.clone(),
      file: self.file(),
      exited: self.log.exited.load(Ordering::SeqCst),
    }
  }
}

/// The live sessions and the two ways to find one: by id, and by the session file it is on.
#[derive(Debug, Default)]
pub(crate) struct Registry {
  by_id: HashMap<LiveSessionId, Arc<Live>>,
  by_file: HashMap<PathBuf, LiveSessionId>,
}

/// The registry behind a lock, plus the lock that serializes `live/open`: starting omp takes seconds, and two opens of
/// one session file must not both start a process.
#[derive(Debug, Default)]
pub(crate) struct LiveSessions {
  registry: Mutex<Registry>,
  pub(crate) open_lock: tokio::sync::Mutex<()>,
}

impl LiveSessions {
  fn registry(&self) -> MutexGuard<'_, Registry> {
    self.registry.lock().unwrap_or_else(PoisonError::into_inner)
  }

  pub(crate) fn get(&self, id: LiveSessionId) -> Result<Arc<Live>> {
    self
      .registry()
      .by_id
      .get(&id)
      .cloned()
      .ok_or(Error::UnknownLiveSession { live: id })
  }

  pub(crate) fn by_file(&self, file: &Path) -> Option<Arc<Live>> {
    let registry = self.registry();
    registry.by_file.get(file).and_then(|id| registry.by_id.get(id)).cloned()
  }

  pub(crate) fn list(&self) -> Vec<LiveSessionSummary> {
    let mut summaries: Vec<_> = self.registry().by_id.values().map(|live| live.summary()).collect();
    summaries.sort_by_key(|summary| summary.live);
    summaries
  }

  pub(crate) fn insert(&self, live: Arc<Live>) {
    let mut registry = self.registry();
    registry.by_file.insert(live.file(), live.id);
    registry.by_id.insert(live.id, live);
  }

  pub(crate) fn remove(&self, id: LiveSessionId) -> Result<Arc<Live>> {
    let mut registry = self.registry();
    let live = registry.by_id.remove(&id).ok_or(Error::UnknownLiveSession { live: id })?;
    registry.by_file.remove(&live.file());
    Ok(live)
  }

  /// Moves a live session to another session file, as a fork or branch does. Opening the file it left starts a new process.
  pub(crate) fn rekey(&self, live: &Live, file: PathBuf) {
    let mut registry = self.registry();
    let mut current = live.file.lock().unwrap_or_else(PoisonError::into_inner);
    registry.by_file.remove(&*current);
    registry.by_file.insert(file.clone(), live.id);
    *current = file;
  }

  pub(crate) fn drain(&self) -> Vec<Arc<Live>> {
    let mut registry = self.registry();
    registry.by_file.clear();
    registry.by_id.drain().map(|(_, live)| live).collect()
  }
}

/// A live session's events since it last settled, and the channel that carries new ones to subscribers.
#[derive(Debug)]
pub(crate) struct EventLog {
  live: LiveSessionId,
  state: Mutex<LogState>,
  sender: broadcast::Sender<Event>,
  exited: AtomicBool,
  metrics: Metrics,
}

#[derive(Debug, Default)]
struct LogState {
  events: Vec<Event>,
  /// The event that last cleared the log. A cursor older than it points at events that are gone.
  floor: Option<EventId>,
}

impl EventLog {
  pub(crate) fn new(live: LiveSessionId, metrics: Metrics) -> Arc<Self> {
    Arc::new(Self {
      live,
      state: Mutex::new(LogState::default()),
      sender: broadcast::channel(BROADCAST_CAPACITY).0,
      exited: AtomicBool::new(false),
      metrics,
    })
  }

  fn state(&self) -> MutexGuard<'_, LogState> {
    self.state.lock().unwrap_or_else(PoisonError::into_inner)
  }

  /// Stamps `body` with a fresh id, appends it, publishes it, and clears the log when the session settled. Appending and
  /// publishing happen under the lock so a subscriber's replay and its live feed meet without a gap or a duplicate.
  fn publish(&self, body: EventBody) {
    // omp's own docs: a prompt result with `sessionSettled` and a `session_settled` event both mean nothing can wake
    // the session again.
    let settled = match &body {
      EventBody::SessionSettled(_) => true,
      EventBody::PromptResult(result) => result.session_settled,
      _ => false,
    };
    let exited = matches!(body, EventBody::ProcessExited);
    let event = Event {
      id: EventId::new(),
      live: self.live,
      at: Timestamp::now(),
      body,
    };
    let id = event.id;
    let mut state = self.state();
    state.events.push(event.clone());
    if self.sender.send(event).is_err() {
      tracing::trace!("no subscriber for a live-session event");
    }
    if settled {
      // Everything before the settle is in omp's transcript, which `live/messages` reads.
      state.events.clear();
      state.floor = Some(id);
    }
    if exited {
      self.exited.store(true, Ordering::SeqCst);
    }
    drop(state);
    self.metrics.event_published();
  }

  /// The retained events newer than `after` and a receiver for every later one.
  pub(crate) fn subscribe(&self, after: Option<EventId>) -> Result<(Vec<Event>, broadcast::Receiver<Event>)> {
    let state = self.state();
    if let (Some(after), Some(floor)) = (after, state.floor)
      && after < floor
    {
      self.metrics.subscription_stale();
      return Err(Error::StaleCursor { live: self.live });
    }
    let start = after.map_or(0, |after| state.events.partition_point(|event| event.id <= after));
    let replay = state.events.get(start..).map(<[Event]>::to_vec).unwrap_or_default();
    // Registered before the lock is released: anything published later reaches the receiver.
    Ok((replay, self.sender.subscribe()))
  }

  pub(crate) fn metrics(&self) -> &Metrics {
    &self.metrics
  }
}

/// Moves the host's events into the log until the stream ends.
pub(crate) async fn pump(log: Arc<EventLog>, mut events: mpsc::Receiver<EventBody>) {
  while let Some(body) = events.recv().await {
    log.publish(body);
  }
}

/// Closes a live session once no request still holds it. A command still in flight is aborted so it returns.
pub(crate) async fn close(live: Arc<Live>) -> Result<()> {
  let live = match Arc::try_unwrap(live) {
    Ok(live) => return live.session.close().await.map_err(Error::from),
    Err(shared) => shared,
  };
  if let Err(error) = live.session.abort_bash().await {
    tracing::debug!(%error, "aborting the command in flight before close");
  }
  let id = live.id;
  let deadline = Instant::now() + IDLE_GRACE;
  let mut shared = live;
  loop {
    shared = match Arc::try_unwrap(shared) {
      Ok(live) => return live.session.close().await.map_err(Error::from),
      Err(shared) => shared,
    };
    if Instant::now() >= deadline {
      return Err(Error::StillRunning { live: id });
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
}

/// A session file's path with its directory resolved, so two spellings of one file (`/var` and `/private/var`) key the
/// same registry entry. The file itself may not exist yet; a directory that does not exist is left as written, since no
/// listed session can live there.
pub(crate) fn canonical_file(path: &Path) -> Result<PathBuf> {
  let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
    return Ok(path.to_path_buf());
  };
  match std::fs::canonicalize(parent) {
    Ok(parent) => Ok(parent.join(name)),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
    Err(source) => Err(Error::SessionFilePath {
      path: path.to_path_buf(),
      source,
    }),
  }
}
