//! Gateway-owned interactive shells and their resumable output logs.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use alvin_protocol::{ErrorCode, EventId, TerminalEvent, TerminalEventBody, TerminalId, TerminalInput, TerminalResize, TerminalSize};
use jsonrpsee::types::ErrorObjectOwned;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{Notify, broadcast, watch};

use crate::terminal_process::ProcessTree;
use crate::{Error, Result};

const OUTPUT_CHUNK: usize = 8192;
const BROADCAST_CAPACITY: usize = 128;
/// Maximum output chunks awaiting acknowledgment by the retained terminal engine.
const RETAINED_EVENTS: usize = 128;

type TerminalSubscription = (Vec<Arc<TerminalEvent>>, broadcast::Receiver<Arc<TerminalEvent>>);

#[derive(Debug, Default)]
struct EventLog {
  events: VecDeque<Arc<TerminalEvent>>,
  floor: Option<EventId>,
  accepted: bool,
  closing: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Terminals {
  entries: Mutex<HashMap<TerminalId, Arc<Terminal>>>,
  stopped: AtomicBool,
  pub(crate) open_lock: tokio::sync::Mutex<()>,
}

impl Terminals {
  pub(crate) fn stop(&self) {
    self.stopped.store(true, Ordering::Release);
  }

  pub(crate) fn ensure_running(&self) -> Result<()> {
    if self.stopped.load(Ordering::Acquire) {
      Err(Error::TerminalShutdown)
    } else {
      Ok(())
    }
  }

  fn entries(&self) -> MutexGuard<'_, HashMap<TerminalId, Arc<Terminal>>> {
    self.entries.lock().unwrap_or_else(PoisonError::into_inner)
  }

  pub(crate) fn get(&self, id: TerminalId) -> Result<Arc<Terminal>> {
    self.entries().get(&id).cloned().ok_or(Error::UnknownTerminal { terminal: id })
  }

  pub(crate) fn insert(&self, terminal: Arc<Terminal>) {
    self.entries().insert(terminal.id, terminal);
  }

  pub(crate) fn remove(&self, id: TerminalId) -> Result<Arc<Terminal>> {
    self.entries().remove(&id).ok_or(Error::UnknownTerminal { terminal: id })
  }

  pub(crate) fn drain(&self) -> Vec<Arc<Terminal>> {
    self.entries().drain().map(|(_, terminal)| terminal).collect()
  }
}

/// Cleans up an open whose subscription was cancelled before acceptance.
pub(crate) struct PendingOpen {
  terminals: Arc<Terminals>,
  id: Option<TerminalId>,
}

impl PendingOpen {
  pub(crate) fn new(terminals: Arc<Terminals>, id: TerminalId) -> Self {
    Self { terminals, id: Some(id) }
  }
}

impl Drop for PendingOpen {
  fn drop(&mut self) {
    if let Some(id) = self.id.take() {
      let terminals = Arc::clone(&self.terminals);
      tokio::spawn(async move {
        match terminals.get(id) {
          Ok(terminal) => match terminal.close_unaccepted().await {
            Ok(true) => {
              if let Err(error) = terminals.remove(id)
                && !matches!(error, Error::UnknownTerminal { .. })
              {
                tracing::error!(terminal = ?id, %error, "removing cancelled terminal");
              }
            }
            Ok(false) => {}
            Err(error) => tracing::error!(terminal = ?id, %error, "closing cancelled terminal"),
          },
          Err(Error::UnknownTerminal { .. }) => {}
          Err(error) => tracing::error!(terminal = ?id, %error, "removing cancelled terminal"),
        }
      });
    }
  }
}

struct Control {
  master: Option<Box<dyn MasterPty + Send>>,
  writer: Box<dyn Write + Send>,
}

impl std::fmt::Debug for Control {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Control").finish_non_exhaustive()
  }
}

#[derive(Debug)]
pub(crate) struct Terminal {
  pub(crate) id: TerminalId,
  control: Mutex<Control>,
  tree: Mutex<ProcessTree>,
  events: Mutex<EventLog>,
  space: Condvar,
  acceptance: Notify,
  sender: broadcast::Sender<Arc<TerminalEvent>>,
  worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
  completion: watch::Sender<Option<std::result::Result<(), Arc<Error>>>>,
}

impl Terminal {
  pub(crate) fn start(cwd: &Path, size: TerminalSize) -> Result<Arc<Self>> {
    let mut command = CommandBuilder::new_default_prog();
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    Self::start_command(command, size)
  }

  fn start_command(command: CommandBuilder, size: TerminalSize) -> Result<Arc<Self>> {
    let id = TerminalId::new();
    let (mut reader, writer, master, mut child, tree) = {
      let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(|source| Error::TerminalAllocate {
          terminal: id,
          source: source.into_boxed_dyn_error(),
        })?;
      let reader = pair.master.try_clone_reader().map_err(|source| Error::TerminalAllocate {
        terminal: id,
        source: source.into_boxed_dyn_error(),
      })?;
      let writer = pair.master.take_writer().map_err(|source| Error::TerminalAllocate {
        terminal: id,
        source: source.into_boxed_dyn_error(),
      })?;
      let mut child = pair.slave.spawn_command(command).map_err(|source| Error::TerminalSpawn {
        terminal: id,
        source: source.into_boxed_dyn_error(),
      })?;
      drop(pair.slave);
      let tree = match ProcessTree::new(child.as_ref()) {
        Ok(tree) => tree,
        Err(source) => {
          child.kill().map_err(|source| Error::TerminalClose { terminal: id, source })?;
          child.wait().map_err(|source| Error::TerminalWait { terminal: id, source })?;
          return Err(Error::TerminalSpawn {
            terminal: id,
            source: Box::new(source),
          });
        }
      };
      (reader, writer, pair.master, child, tree)
    };
    let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
    let terminal = Arc::new(Self {
      id,
      control: Mutex::new(Control {
        master: Some(master),
        writer,
      }),
      tree: Mutex::new(tree),
      events: Mutex::new(EventLog::default()),
      space: Condvar::new(),
      acceptance: Notify::new(),
      sender,
      worker: Mutex::new(None),
      completion: watch::channel(None).0,
    });
    terminal.publish(TerminalEventBody::Started);
    let running = Arc::clone(&terminal);
    let worker = tokio::task::spawn_blocking(move || {
      #[cfg(windows)]
      let child_waiter = {
        let terminal = Arc::clone(&running);
        let (sender, receiver) = std::sync::mpsc::channel();
        match std::thread::Builder::new().spawn(move || {
          let status = child.wait();
          let master = terminal.control.lock().unwrap_or_else(PoisonError::into_inner).master.take();
          drop(master);
          if let Err(error) = sender.send(status) {
            tracing::debug!(%error, "terminal wait receiver closed");
          }
        }) {
          Ok(_) => receiver,
          Err(source) => {
            running.failed(Error::TerminalWait { terminal: id, source }, ErrorCode::TerminalWaitFailed);
            if let Err(source) = running.tree.lock().unwrap_or_else(PoisonError::into_inner).terminate() {
              running.failed(Error::TerminalClose { terminal: id, source }, ErrorCode::TerminalCloseFailed);
            }
            return;
          }
        }
      };
      let mut buffer = [0_u8; OUTPUT_CHUNK];
      let mut draining = false;
      loop {
        match reader.read(&mut buffer) {
          Ok(0) => break,
          Ok(count) => {
            if !draining
              && let Some(bytes) = buffer.get(..count)
              && !running.publish(TerminalEventBody::Output { bytes: bytes.to_vec() })
            {
              // Closing ConPTY can wait for its output pipe to be drained.
              if cfg!(windows) {
                draining = true;
              } else {
                break;
              }
            }
          }
          Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
          Err(source) => {
            running.failed(Error::TerminalRead { terminal: id, source }, ErrorCode::TerminalReadFailed);
            if let Err(source) = running.tree.lock().unwrap_or_else(PoisonError::into_inner).terminate() {
              running.failed(Error::TerminalClose { terminal: id, source }, ErrorCode::TerminalCloseFailed);
            }
            break;
          }
        }
      }
      #[cfg(windows)]
      let status = match child_waiter.recv() {
        Ok(status) => status,
        Err(source) => {
          running.failed(Error::TerminalWaitWorker { terminal: id, source }, ErrorCode::TerminalWaitFailed);
          return;
        }
      };
      #[cfg(not(windows))]
      let status = child.wait();
      match status {
        Ok(status) => {
          running.publish(TerminalEventBody::Exited {
            code: status.signal().is_none().then_some(status.exit_code()),
            signal: status.signal().map(str::to_owned),
          });
        }
        Err(source) => running.failed(Error::TerminalWait { terminal: id, source }, ErrorCode::TerminalWaitFailed),
      }
    });
    *terminal.worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(worker);
    Ok(terminal)
  }

  fn publish(&self, body: TerminalEventBody) -> bool {
    let mut log = self.events.lock().unwrap_or_else(PoisonError::into_inner);
    while log.events.len() >= RETAINED_EVENTS && !log.closing {
      log = self.space.wait(log).unwrap_or_else(PoisonError::into_inner);
    }
    if log.closing {
      return false;
    }
    let event = Arc::new(TerminalEvent {
      id: EventId::new(),
      terminal: self.id,
      body,
    });
    log.events.push_back(Arc::clone(&event));
    if let Err(broadcast::error::SendError(_unobserved)) = self.sender.send(event) {
      tracing::trace!(terminal = ?self.id, "terminal event retained without subscribers");
    }
    true
  }

  fn failed(&self, error: Error, code: ErrorCode) {
    let object = ErrorObjectOwned::from(error);
    self.publish(TerminalEventBody::Failed {
      code: code.code(),
      message: object.message().to_owned(),
    });
  }

  pub(crate) fn subscribe(&self, after: Option<EventId>) -> Result<TerminalSubscription> {
    let log = self.events.lock().unwrap_or_else(PoisonError::into_inner);
    let start = match after {
      None if log.floor.is_none() => 0,
      Some(id) if log.floor == Some(id) => 0,
      Some(id) => log
        .events
        .iter()
        .position(|event| event.id == id)
        .map(|index| index.saturating_add(1))
        .ok_or(Error::TerminalCursor { terminal: self.id })?,
      None => return Err(Error::TerminalCursor { terminal: self.id }),
    };
    let receiver = self.sender.subscribe();
    Ok((log.events.iter().skip(start).cloned().collect(), receiver))
  }

  pub(crate) fn accepted(&self, through: EventId) -> Result<()> {
    let mut log = self.events.lock().unwrap_or_else(PoisonError::into_inner);
    if log.closing {
      return Err(Error::TerminalNotReady { terminal: self.id });
    }
    if log.floor == Some(through) {
      return Ok(());
    }
    let index = log
      .events
      .iter()
      .position(|event| event.id == through)
      .ok_or(Error::TerminalCursor { terminal: self.id })?;
    drop(log.events.drain(..=index));
    log.accepted = true;
    self.acceptance.notify_waiters();
    log.floor = Some(through);
    self.space.notify_all();
    Ok(())
  }

  pub(crate) async fn wait_for_acceptance(&self) {
    loop {
      let notified = self.acceptance.notified();
      tokio::pin!(notified);
      notified.as_mut().enable();
      let ready = {
        let log = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        log.accepted || log.closing
      };
      if ready {
        return;
      }
      notified.await;
    }
  }

  pub(crate) fn input(&self, request: TerminalInput) -> Result<()> {
    {
      let log = self.events.lock().unwrap_or_else(PoisonError::into_inner);
      if !log.accepted || log.closing {
        return Err(Error::TerminalNotReady { terminal: self.id });
      }
    }
    let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
    control
      .writer
      .write_all(&request.bytes)
      .map_err(|source| Error::TerminalWrite { terminal: self.id, source })
  }

  pub(crate) fn resize(&self, request: TerminalResize) -> Result<()> {
    let control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
    control
      .master
      .as_ref()
      .ok_or(Error::TerminalNotReady { terminal: self.id })?
      .resize(pty_size(request.size))
      .map_err(|source| Error::TerminalResize {
        terminal: self.id,
        source: source.into_boxed_dyn_error(),
      })
  }

  pub(crate) async fn close(self: Arc<Self>) -> Result<()> {
    self.close_inner(false).await.map(drop)
  }

  async fn close_unaccepted(self: Arc<Self>) -> Result<bool> {
    self.close_inner(true).await
  }

  async fn close_inner(self: Arc<Self>, only_unaccepted: bool) -> Result<bool> {
    let mut completion = self.completion.subscribe();
    let start = {
      let mut log = self.events.lock().unwrap_or_else(PoisonError::into_inner);
      if only_unaccepted && log.accepted {
        return Ok(false);
      }
      if log.closing {
        false
      } else {
        self.acceptance.notify_waiters();
        log.closing = true;
        self.space.notify_all();
        true
      }
    };
    if start {
      let terminal = Arc::clone(&self);
      tokio::spawn(async move {
        let result = terminal.finish_close().await.map_err(Arc::new);
        terminal.completion.send_replace(Some(result));
      });
    }
    loop {
      if let Some(result) = completion.borrow().clone() {
        return result
          .map(|()| true)
          .map_err(|source| Error::TerminalCloseShared { terminal: self.id, source });
      }
      completion
        .changed()
        .await
        .map_err(|source| Error::TerminalCompletion { terminal: self.id, source })?;
    }
  }

  async fn finish_close(&self) -> Result<()> {
    let task = self.worker.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(task) = task {
      if !task.is_finished() {
        self
          .tree
          .lock()
          .unwrap_or_else(PoisonError::into_inner)
          .terminate()
          .map_err(|source| Error::TerminalClose { terminal: self.id, source })?;
      }
      task.await?;
    }
    Ok(())
  }
}

fn pty_size(size: TerminalSize) -> PtySize {
  PtySize {
    rows: size.rows.get(),
    cols: size.columns.get(),
    pixel_width: size.pixel_width,
    pixel_height: size.pixel_height,
  }
}

#[cfg(test)]
mod tests {
  use std::num::NonZeroU16;
  use std::path::PathBuf;

  use alvin_client::{ChannelPair, Client, Endpoint, Error as ClientError};
  use alvin_protocol::{
    ErrorCode, EventId, OpenTarget, SessionId, TerminalApiClient, TerminalEventBody, TerminalInput, TerminalSize, TerminalSubscription,
  };
  use test_context::{AsyncTestContext, test_context};

  use crate::{Gateway, GatewayConfig, HostConfig};

  struct ShellFixture {
    base: PathBuf,
    gateway: Gateway,
  }

  impl AsyncTestContext for ShellFixture {
    async fn setup() -> Self {
      let base = std::env::temp_dir().join(format!("alvin-terminal-{}", EventId::new().as_uuid()));
      let project = base.join("project");
      let sessions = base.join("sessions");
      std::fs::create_dir_all(&project).unwrap();
      std::fs::create_dir_all(sessions.join("-project")).unwrap();
      let project = project.canonicalize().unwrap();
      let records = include_str!("../fixtures/session-records.jsonl")
        .replace("\"/tmp/alvin-probe.hnBA/proj\"", &serde_json::to_string(&project).unwrap());
      std::fs::write(sessions.join("-project/terminal.jsonl"), records).unwrap();
      let omp = HostConfig::builder().omp(PathBuf::from("omp")).build();
      let gateway = Gateway::new(GatewayConfig::builder().sessions_dir(sessions).omp(omp).build());
      Self { base, gateway }
    }

    async fn teardown(self) {
      self.gateway.shutdown().await.unwrap();
      std::fs::remove_dir_all(self.base).unwrap();
    }
  }

  #[test_context(ShellFixture)]
  #[tokio::test]
  async fn unacknowledged_open_is_reaped_when_subscription_is_cancelled(fixture: &mut ShellFixture) {
    let channel = Client::connect(Endpoint::Channel(ChannelPair::from(fixture.gateway.connect_channel(16).unwrap())))
      .await
      .unwrap();
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
      .await
      .unwrap();
    let server = fixture.gateway.serve_ws(listener).await.unwrap();
    let endpoint = format!("ws://{}", server.local_addr()).parse::<Endpoint>().unwrap();
    let websocket = Client::connect(endpoint).await.unwrap();
    let clients = [channel, websocket];
    let target = OpenTarget::Session {
      session: SessionId::from("01a18ff1-ab74-73b8-a3a2-79905565c49b".to_owned()),
    };
    let size = TerminalSize {
      columns: NonZeroU16::new(80).unwrap(),
      rows: NonZeroU16::new(24).unwrap(),
      pixel_width: 800,
      pixel_height: 480,
    };
    for client in &clients {
      let mut stream = TerminalApiClient::terminal_events(
        client,
        TerminalSubscription::Open {
          target: target.clone(),
          size,
        },
      )
      .await
      .unwrap();
      let started = stream.next().await.unwrap().unwrap();
      assert!(matches!(started.body, TerminalEventBody::Started));
      let terminal = fixture.gateway.terminals.get(started.terminal).unwrap();
      let input = TerminalApiClient::input(
        client,
        TerminalInput {
          terminal: started.terminal,
          bytes: b"not delivered".to_vec(),
        },
      )
      .await
      .unwrap_err();
      assert!(matches!(
        ClientError::from(input),
        ClientError::Gateway {
          code: ErrorCode::CommandNotAllowed,
          ..
        }
      ));
      let mut completion = terminal.completion.subscribe();
      drop(stream);
      loop {
        if let Some(result) = completion.borrow().clone() {
          assert!(result.is_ok(), "{result:?}");
          break;
        }
        completion.changed().await.unwrap();
      }
    }
    drop(clients);
    server.shutdown().await.unwrap();
  }
  #[test_context(ShellFixture)]
  #[tokio::test]
  async fn closing_terminal_terminates_background_process_over_both_transports(fixture: &mut ShellFixture) {
    use alvin_protocol::TerminalAccepted;
    use portable_pty::CommandBuilder;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;

    let channel = Client::connect(Endpoint::Channel(ChannelPair::from(fixture.gateway.connect_channel(16).unwrap())))
      .await
      .unwrap();
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
      .await
      .unwrap();
    let server = fixture.gateway.serve_ws(listener).await.unwrap();
    let websocket = Client::connect(format!("ws://{}", server.local_addr()).parse::<Endpoint>().unwrap())
      .await
      .unwrap();
    let size = TerminalSize {
      columns: NonZeroU16::new(80).unwrap(),
      rows: NonZeroU16::new(24).unwrap(),
      pixel_width: 800,
      pixel_height: 480,
    };
    let bun = std::process::Command::new("mise").args(["which", "bun"]).output().unwrap();
    assert!(bun.status.success(), "{}", String::from_utf8_lossy(&bun.stderr));
    let bun = String::from_utf8(bun.stdout).unwrap();
    let clients = [channel, websocket];
    for client in &clients {
      let child_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
      let port = child_listener.local_addr().unwrap().port();
      let child_script =
        format!("const s = require('net').connect({port}, '127.0.0.1'); s.on('connect', () => s.write(process.pid + '\\n'));");
      let parent_script = format!(
        "require('child_process').spawn(process.execPath, ['-e', {}], {{stdio:'ignore'}}); require('net').createServer().listen(0, '127.0.0.1');",
        serde_json::to_string(&child_script).unwrap()
      );
      let mut command = CommandBuilder::new(bun.trim());
      command.arg("-e");
      command.arg(parent_script);
      let terminal = super::Terminal::start_command(command, size).unwrap();
      fixture.gateway.terminals.insert(terminal.clone());
      let mut stream = TerminalApiClient::terminal_events(
        client,
        TerminalSubscription::Resume {
          terminal: terminal.id,
          after: None,
        },
      )
      .await
      .unwrap();
      let event = stream.next().await.unwrap().unwrap();
      assert!(matches!(event.body, TerminalEventBody::Started));
      TerminalApiClient::accepted(
        client,
        TerminalAccepted {
          terminal: terminal.id,
          through: event.id,
        },
      )
      .await
      .unwrap();
      let mut reader = tokio::task::spawn_blocking(move || {
        let (socket, _) = child_listener.accept().unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.trim().parse::<u32>().unwrap() > 0);
        reader
      })
      .await
      .unwrap();
      TerminalApiClient::close(client, terminal.id).await.unwrap();
      tokio::task::spawn_blocking(move || {
        let mut rest = Vec::new();
        if let Err(error) = reader.read_to_end(&mut rest) {
          assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        }
      })
      .await
      .unwrap();
    }
    drop(clients);
    server.shutdown().await.unwrap();
  }
}
