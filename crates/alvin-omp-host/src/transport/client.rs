//! Async transport for `omp --mode rpc` over any reader/writer pair.
//!
//! A reader task decodes stdout lines (reassembling protocol v2 `rpc_chunk` sequences), routes responses to waiting
//! calls by id, feeds prompt collectors, runs host tools and host URIs, and delivers everything else as [`Event`]s. A
//! writer task owns stdin: callers queue encoded frames and await their response on a oneshot channel, so any number
//! of requests are in flight on the one pipe and omp alone decides what runs concurrently.
//!
//! Once the client is closed (a transport violation, the peer's EOF, a failed write, [`Client::close`], or drop) every
//! pending and later call fails with the closing error, no further server frame is dispatched, queued frames are
//! discarded, and the writer task drops stdin so the server sees EOF.

use std::collections::{HashMap, HashSet};
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use omp_rpc::{
  AgentMessage, Command, GetMessagesCommand, GetMessagesPageCommand, HostToolResult, HostUriOperation, HostUriResult,
  NegotiateProtocolCommand, PromptCommand, PromptResultEvent, ReadyEvent, RpcAgentEvent, RpcInbound, RpcNotification, RpcResponse,
  RpcServerFrame, SetHostToolsCommand, SetHostUriSchemesCommand,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command as Process};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
#[cfg(unix)]
use tokio::time::sleep;
use tokio::time::{Instant, timeout, timeout_at};

use super::frame::{FrameDecoder, MAX_FRAME_BYTES, MAX_REASSEMBLED_BYTES};
use super::host_tool::{HostTool, HostToolContext, text_payload};
use super::host_uri::{HostUri, HostUriContext, uri_scheme};
use super::turn::PromptTurn;

const PAGE_LIMIT: i64 = 256;
const PAGE_BUSY_ERROR: &str = "Cannot page messages while the session is changing";
const PAGE_STALE_ERROR: &str = "RPC message cursor is stale";
/// How long teardown waits for the process group leader after each signal.
const LEADER_EXIT_TIMEOUT: Duration = Duration::from_secs(1);
/// After SIGKILL, killed members linger until reaped by their (new) parent.
#[cfg(unix)]
const KILL_SETTLE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long teardown waits for the reader task once the process is gone.
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(1);
/// How often teardown re-checks an emptying process group: the kernel sends no notification when one empties.
#[cfg(unix)]
const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Client failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// Spawning or talking to the process failed, or teardown could not confirm that the process group is gone.
  #[error("io: {0}")]
  Io(#[from] io::Error),
  /// Encoding a frame or decoding a response failed.
  #[error("json: {0}")]
  Json(#[from] serde_json::Error),
  /// The server answered `success: false`.
  #[error("{command} failed: {error}")]
  Command {
    /// The command that failed.
    command: String,
    /// omp's message.
    error: String,
    /// omp's machine-readable reason, when it sent one.
    code: Option<String>,
  },
  /// A bounded wait ended without an answer.
  #[error("{command} timed out")]
  Timeout {
    /// What was being waited for.
    command: String,
  },
  /// The server's stdout closed, or the client was closed.
  #[error("server closed")]
  Closed,
  /// The transport broke its contract (invalid JSON, bad chunk sequence, failed negotiation).
  #[error("protocol error: {0}")]
  Protocol(String),
  /// A definition was rejected before reaching the server (an empty URI scheme, for example).
  #[error("invalid argument: {0}")]
  InvalidArgument(String),
  /// The Windows job object that holds the server's process tree could not be set up.
  #[cfg(windows)]
  #[error("job object: {0}")]
  Job(#[from] win32job::JobError),
}

/// A server frame that is not a correlated response, a host tool frame, or a host URI frame (the client serves those
/// itself).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
  /// Unsolicited frame; unrecognized types arrive as [`RpcNotification::Unknown`].
  Notification(Box<RpcNotification>),
  /// A response whose id matched no pending call.
  UnmatchedResponse(RpcResponse),
  /// A known frame type whose payload failed to decode.
  Undecodable {
    /// The frame as omp sent it.
    raw: Value,
    /// Why it did not decode.
    error: String,
  },
}

/// Construction options.
#[derive(Debug, Clone)]
pub struct ClientOptions {
  /// Host tools registered right after startup (see [`Client::set_custom_tools`]).
  pub tools: Vec<HostTool>,
  /// Host URI schemes registered after the tools (see [`Client::set_host_uris`]).
  pub uris: Vec<HostUri>,
  /// Deadline for the `ready` frame.
  pub ready_timeout: Duration,
}

impl Default for ClientOptions {
  fn default() -> Self {
    Self {
      tools: Vec::new(),
      uris: Vec::new(),
      ready_timeout: Duration::from_secs(60),
    }
  }
}

/// Why the client closed.
#[derive(Clone)]
enum Closure {
  /// The server closed its stdout.
  Eof,
  /// [`Client::close`] or drop.
  Closed,
  Protocol(String),
  /// Writing to the server's stdin failed.
  Write(io::ErrorKind, String),
}

impl Closure {
  fn error(&self) -> Error {
    match self {
      Self::Eof | Self::Closed => Error::Closed,
      Self::Protocol(message) => Error::Protocol(message.clone()),
      Self::Write(kind, message) => Error::Io(io::Error::new(*kind, message.clone())),
    }
  }
}

/// What a prompt collector receives.
enum PromptSignal {
  Event(Box<RpcAgentEvent>),
  Result(Box<PromptResultEvent>),
  Failed(Error),
}

/// Which host request registry a reply belongs to.
#[derive(Clone, Copy)]
pub(crate) enum HostRequestKind {
  Tool,
  Uri,
}

/// Ties a queued frame to a host request: the writer skips the frame when the request was cancelled, checking right
/// before it writes.
pub(crate) struct Guard {
  pub(crate) cancelled: Arc<AtomicBool>,
  /// Set on the final reply: the request's cancellation entry is released once the reply is written or skipped, so a
  /// cancel arriving while it waits in the queue still suppresses it.
  pub(crate) release: Option<(HostRequestKind, String)>,
}

/// An encoded frame waiting for the writer task.
struct Outgoing {
  line: Vec<u8>,
  guard: Option<Guard>,
}

type ResponseSender = oneshot::Sender<Result<RpcResponse, Error>>;

/// Delivering to a receiver that is gone is expected: a cancelled call, a dropped event stream.
fn gone<E>(delivered: Result<(), E>) {
  if delivered.is_err() {
    tracing::trace!("the receiver is gone");
  }
}

#[derive(Default)]
struct State {
  /// Queue of the writer task; `None` once closed.
  outgoing: Option<mpsc::UnboundedSender<Outgoing>>,
  /// Request id → (command, waiter).
  pending: HashMap<String, (String, ResponseSender)>,
  /// Prompt request id → collector.
  prompts: HashMap<String, mpsc::UnboundedSender<PromptSignal>>,
  /// Host tool request id → cancellation flag.
  host_calls: HashMap<String, Arc<AtomicBool>>,
  /// Host URI request id → cancellation flag.
  uri_requests: HashMap<String, Arc<AtomicBool>>,
  /// Agent tool call id → host tool name, for renaming `tool_execution_*` events.
  dispatch_names: HashMap<String, String>,
  ready: Option<oneshot::Sender<ReadyEvent>>,
  closed: Option<Closure>,
}

impl State {
  fn requests(&mut self, kind: HostRequestKind) -> &mut HashMap<String, Arc<AtomicBool>> {
    match kind {
      HostRequestKind::Tool => &mut self.host_calls,
      HostRequestKind::Uri => &mut self.uri_requests,
    }
  }
}

/// State shared by the client, its reader and writer tasks, and host handler tasks.
pub(crate) struct Shared {
  state: Mutex<State>,
  /// `rpc_chunk` frames are accepted (set right before `negotiate_protocol`).
  v2: AtomicBool,
  tools: Mutex<Vec<HostTool>>,
  uris: Mutex<Vec<HostUri>>,
}

impl Shared {
  fn state(&self) -> MutexGuard<'_, State> {
    self.state.lock().unwrap_or_else(PoisonError::into_inner)
  }

  fn is_closed(&self) -> bool {
    self.state().closed.is_some()
  }

  /// Queues `frame` for the writer task; fails once the client is closed.
  pub(crate) fn enqueue<T: Serialize>(&self, frame: &T, guard: Option<Guard>) -> Result<(), Error> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    let state = self.state();
    if let Some(closure) = &state.closed {
      return Err(closure.error());
    }
    let outgoing = state.outgoing.as_ref().ok_or(Error::Closed)?;
    outgoing.send(Outgoing { line, guard }).map_err(|_unsent| Error::Closed)
  }

  /// Registers a host request's cancellation flag; `None` once closed, so no handler starts after teardown.
  fn register(&self, kind: HostRequestKind, id: &str) -> Option<Arc<AtomicBool>> {
    let mut state = self.state();
    if state.closed.is_some() {
      return None;
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    state.requests(kind).insert(id.to_owned(), Arc::clone(&cancelled));
    Some(cancelled)
  }

  fn cancel(&self, kind: HostRequestKind, id: &str) {
    if let Some(cancelled) = self.state().requests(kind).get(id) {
      cancelled.store(true, Ordering::SeqCst);
    }
  }

  fn release(&self, kind: HostRequestKind, id: &str, cancelled: &Arc<AtomicBool>) {
    let mut state = self.state();
    let requests = state.requests(kind);
    if requests.get(id).is_some_and(|entry| Arc::ptr_eq(entry, cancelled)) {
      requests.remove(id);
    }
  }

  /// Closes the client; the first closure wins.
  fn close(&self, closure: Closure) {
    let mut state = self.state();
    if state.closed.is_some() {
      return;
    }
    // Dropping the only sender ends the writer task, which drops stdin.
    state.outgoing = None;
    for (_, (_, waiter)) in state.pending.drain() {
      gone(waiter.send(Err(closure.error())));
    }
    for collector in state.prompts.values() {
      gone(collector.send(PromptSignal::Failed(closure.error())));
    }
    let state = &mut *state;
    for (_, cancelled) in state.host_calls.drain().chain(state.uri_requests.drain()) {
      cancelled.store(true, Ordering::SeqCst);
    }
    state.dispatch_names.clear();
    state.ready = None;
    state.closed = Some(closure);
  }

  async fn write_loop<W: AsyncWrite + Unpin>(shared: Weak<Self>, mut writer: W, mut queue: mpsc::UnboundedReceiver<Outgoing>) {
    while let Some(frame) = queue.recv().await {
      let Some(shared) = shared.upgrade() else {
        return;
      };
      let cancelled = frame.guard.as_ref().is_some_and(|guard| guard.cancelled.load(Ordering::SeqCst));
      let written = if cancelled || shared.is_closed() {
        Ok(())
      } else {
        match writer.write_all(&frame.line).await {
          Ok(()) => writer.flush().await,
          Err(error) => Err(error),
        }
      };
      if let Some(Guard {
        cancelled,
        release: Some((kind, id)),
      }) = &frame.guard
      {
        shared.release(*kind, id, cancelled);
      }
      if let Err(error) = written {
        let message = format!("Failed to write RPC input: {error}");
        shared.close(Closure::Write(error.kind(), message));
        return;
      }
    }
  }

  async fn read_loop<R: AsyncBufReadExt + Unpin>(self: &Arc<Self>, mut reader: R, events: &mpsc::UnboundedSender<Event>) -> Closure {
    let mut decoder = FrameDecoder::default();
    let mut line = Vec::new();
    loop {
      line.clear();
      match reader.read_until(b'\n', &mut line).await {
        Ok(0) => return Closure::Eof,
        Ok(_) => {}
        Err(error) => return Closure::Protocol(format!("Failed to read RPC output: {error}")),
      }
      if self.is_closed() {
        return Closure::Closed;
      }
      let text = line.trim_ascii();
      if text.is_empty() {
        continue;
      }
      let raw: Value = match serde_json::from_slice(text) {
        Ok(raw) => raw,
        Err(error) => {
          return Closure::Protocol(format!("Failed to decode RPC output: {error}"));
        }
      };
      if raw.get("type").and_then(Value::as_str) == Some("rpc_chunk") && !self.v2.load(Ordering::SeqCst) {
        return Closure::Protocol("RPC chunk received before protocol negotiation".to_owned());
      }
      match decoder.push(raw) {
        Ok(Some(frame)) => self.dispatch(frame, events),
        Ok(None) => {}
        Err(message) => return Closure::Protocol(message),
      }
    }
  }

  fn dispatch(self: &Arc<Self>, mut frame: Value, events: &mpsc::UnboundedSender<Event>) {
    match frame.get("type").and_then(Value::as_str) {
      Some("host_tool_call") => return self.host_tool_call(frame, events),
      Some("host_uri_request") => return self.host_uri_request(frame),
      Some(kind @ ("host_tool_cancel" | "host_uri_cancel")) => {
        if let Some(target) = frame.get("targetId").and_then(Value::as_str) {
          let kind = if kind == "host_tool_cancel" {
            HostRequestKind::Tool
          } else {
            HostRequestKind::Uri
          };
          self.cancel(kind, target);
          return;
        }
      }
      Some(kind @ ("tool_execution_update" | "tool_execution_end")) => {
        let end = kind == "tool_execution_end";
        self.rename_tool_event(&mut frame, end);
      }
      _ => {}
    }
    let decoded = match RpcServerFrame::deserialize(&frame) {
      Ok(decoded) => decoded,
      Err(error) => {
        let error = error.to_string();
        if frame.get("type").and_then(Value::as_str) == Some("prompt_result")
          && let Some(id) = frame.get("id").and_then(Value::as_str)
          && let Some(collector) = self.state().prompts.get(id)
        {
          let message = format!("Failed to parse prompt_result: {error}");
          gone(collector.send(PromptSignal::Failed(Error::Protocol(message))));
        }
        gone(events.send(Event::Undecodable { raw: frame, error }));
        return;
      }
    };
    let event = match decoded {
      RpcServerFrame::Response(response) => match self.response(response) {
        Some(response) => Event::UnmatchedResponse(response),
        None => return,
      },
      // Host tool and URI frames were consumed above; only malformed ones get here.
      RpcServerFrame::RpcHostRequest(_) => return,
      RpcServerFrame::RpcNotification(notification) => {
        self.notification(&notification);
        Event::Notification(Box::new(notification))
      }
      RpcServerFrame::Unknown(raw) => Event::Notification(Box::new(RpcNotification::Unknown(raw))),
    };
    // The host may drop the receiver; keep serving calls.
    gone(events.send(event));
  }

  /// Delivers a response to its waiter; returns it when nobody waits for it.
  fn response(&self, response: RpcResponse) -> Option<RpcResponse> {
    let mut state = self.state();
    if let Some(id) = &response.id {
      if let Some((_, waiter)) = state.pending.remove(id) {
        gone(waiter.send(Ok(response)));
        return None;
      }
      if !response.success {
        // A prompt that was acknowledged and then failed.
        if let Some(collector) = state.prompts.get(id) {
          gone(collector.send(PromptSignal::Failed(command_error(response))));
          return None;
        }
      }
    }
    if !response.success {
      // An uncorrelated failure belongs to the only pending call of that command.
      let mut matching = state.pending.iter().filter(|(_, (command, _))| *command == response.command);
      if let (Some((id, _)), None) = (matching.next(), matching.next()) {
        let id = id.clone();
        if let Some((_, waiter)) = state.pending.remove(&id) {
          gone(waiter.send(Ok(response)));
          return None;
        }
      }
    }
    Some(response)
  }

  fn notification(&self, notification: &RpcNotification) {
    let mut state = self.state();
    match notification {
      RpcNotification::Ready(ready) => {
        if let Some(sender) = state.ready.take() {
          gone(sender.send(ready.clone()));
        }
      }
      RpcNotification::PromptResult(result) => {
        if let Some(collector) = result.id.as_ref().and_then(|id| state.prompts.get(id)) {
          gone(collector.send(PromptSignal::Result(Box::new(result.clone()))));
        }
      }
      RpcNotification::RpcAgentEvent(event) => {
        for collector in state.prompts.values() {
          gone(collector.send(PromptSignal::Event(Box::new(event.clone()))));
        }
      }
      _ => {}
    }
  }

  /// With `tools.xdev`, host tools run through `write` to an `xd://` device, so update/end events name the transport
  /// tool; report the host tool instead. `tool_execution_start` precedes `host_tool_call` and keeps the transport name.
  fn rename_tool_event(&self, frame: &mut Value, end: bool) {
    let Value::Object(map) = frame else { return };
    let Some(call_id) = map.get("toolCallId").and_then(Value::as_str) else {
      return;
    };
    let mut state = self.state();
    let name = if end {
      state.dispatch_names.remove(call_id)
    } else {
      state.dispatch_names.get(call_id).cloned()
    };
    if let Some(name) = name {
      map.insert("toolName".to_owned(), Value::String(name));
    }
  }

  fn host_tool_call(self: &Arc<Self>, frame: Value, events: &mpsc::UnboundedSender<Event>) {
    let Value::Object(mut map) = frame else {
      return;
    };
    let (Some(Value::String(id)), Some(Value::String(tool_name)), Some(Value::String(tool_call_id))) =
      (map.remove("id"), map.remove("toolName"), map.remove("toolCallId"))
    else {
      gone(events.send(Event::Undecodable {
        raw: Value::Object(map),
        error: "host_tool_call needs string id, toolName, and toolCallId".to_owned(),
      }));
      return;
    };
    self.state().dispatch_names.insert(tool_call_id.clone(), tool_name.clone());
    let Some(Value::Object(arguments)) = map.remove("arguments") else {
      return self.host_tool_error(id, "Host tool arguments must be an object".to_owned(), None);
    };
    let tool = self
      .tools
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .iter()
      .find(|tool| tool.definition().name == tool_name)
      .cloned();
    let Some(tool) = tool else {
      return self.host_tool_error(id, format!("Host tool \"{tool_name}\" is not registered"), None);
    };
    let Some(cancelled) = self.register(HostRequestKind::Tool, &id) else {
      return;
    };
    let context = HostToolContext {
      tool_call_id,
      request_id: id,
      cancelled,
      shared: Arc::clone(self),
    };
    tokio::spawn(Arc::clone(self).run_host_tool(tool, arguments, context));
  }

  /// Runs one host tool on a task of its own and sends its result. A handler that panics ends its own task only.
  async fn run_host_tool(self: Arc<Self>, tool: HostTool, arguments: Map<String, Value>, context: HostToolContext) {
    let id = context.request_id.clone();
    let guard = Guard {
      cancelled: Arc::clone(&context.cancelled),
      release: Some((HostRequestKind::Tool, id.clone())),
    };
    let name = tool.definition().name.clone();
    match tokio::spawn(tool.run(arguments, context)).await {
      Ok(Ok(output)) => {
        let result = HostToolResult {
          id,
          result: output.into_payload(),
          is_error: None,
        };
        // Fails only once closed, which already cancelled this call.
        if let Err(error) = self.enqueue(&RpcInbound::HostToolResult(result), Some(guard)) {
          tracing::debug!(%error, "dropping the result of a host tool call");
        }
      }
      Ok(Err(error)) => self.host_tool_error(id, error.to_string(), Some(guard)),
      Err(error) => {
        tracing::warn!(%error, tool = %name, "a host tool handler did not finish");
        self.host_tool_error(id, format!("Host tool \"{name}\" panicked"), Some(guard));
      }
    }
  }

  fn host_tool_error(&self, id: String, text: String, guard: Option<Guard>) {
    let result = HostToolResult {
      id,
      result: text_payload(text, Some(Value::Object(Map::new()))),
      is_error: Some(true),
    };
    if let Err(error) = self.enqueue(&RpcInbound::HostToolResult(result), guard) {
      tracing::debug!(%error, "dropping the error result of a host tool call");
    }
  }

  fn host_uri_request(self: &Arc<Self>, frame: Value) {
    let Value::Object(mut map) = frame else {
      return;
    };
    let (Some(Value::String(id)), Some(Value::String(operation)), Some(Value::String(url))) =
      (map.remove("id"), map.remove("operation"), map.remove("url"))
    else {
      return;
    };
    let operation = match operation.as_str() {
      "read" => HostUriOperation::Read,
      "write" => HostUriOperation::Write,
      other => {
        return self.host_uri_error(id, format!("Unsupported host URI operation: {other}"), None);
      }
    };
    let scheme = uri_scheme(&url);
    let uri = self
      .uris
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .iter()
      .find(|uri| uri.scheme() == scheme)
      .cloned();
    let Some(uri) = uri else {
      return self.host_uri_error(id, format!("No host URI scheme \"{scheme}://\" is registered"), None);
    };
    if operation == HostUriOperation::Write && !uri.writable() {
      return self.host_uri_error(id, format!("Host URI scheme \"{scheme}://\" is read-only"), None);
    }
    let content = match map.remove("content") {
      Some(Value::String(content)) => Some(content),
      Some(other) => {
        return self.host_uri_error(id, format!("Host URI write content must be a string, got {other}"), None);
      }
      None => None,
    };
    let Some(cancelled) = self.register(HostRequestKind::Uri, &id) else {
      return;
    };
    let context = HostUriContext { url, operation, cancelled };
    tokio::spawn(Arc::clone(self).run_host_uri(uri, id, content, context));
  }

  /// Runs one host URI handler on a task of its own and sends its result.
  async fn run_host_uri(self: Arc<Self>, uri: HostUri, id: String, content: Option<String>, context: HostUriContext) {
    let guard = Guard {
      cancelled: Arc::clone(&context.cancelled),
      release: Some((HostRequestKind::Uri, id.clone())),
    };
    let scheme = uri.scheme().to_owned();
    let operation = context.operation;
    let handler = match operation {
      HostUriOperation::Read => tokio::spawn(async move { uri.run_read(context).await.map(Some) }),
      HostUriOperation::Write => {
        let content = content.unwrap_or_default();
        tokio::spawn(async move {
          match uri.run_write(context, content).await {
            Some(outcome) => outcome.map(|()| None),
            None => Ok(None),
          }
        })
      }
    };
    let result = match handler.await {
      Ok(Ok(Some(read))) => HostUriResult {
        id,
        content: Some(read.content),
        content_type: read.content_type,
        notes: read.notes,
        immutable: read.immutable,
        is_error: None,
        error: None,
      },
      // A write succeeded.
      Ok(Ok(None)) => HostUriResult {
        id,
        content: None,
        content_type: None,
        notes: None,
        immutable: None,
        is_error: None,
        error: None,
      },
      Ok(Err(error)) => return self.host_uri_error(id, error.to_string(), Some(guard)),
      Err(error) => {
        tracing::warn!(%error, %scheme, "a host URI handler did not finish");
        return self.host_uri_error(id, format!("Host URI handler for \"{scheme}://\" panicked"), Some(guard));
      }
    };
    // Fails only once closed, which already cancelled this request.
    if let Err(error) = self.enqueue(&RpcInbound::HostUriResult(result), Some(guard)) {
      tracing::debug!(%error, "dropping the result of a host URI request");
    }
  }

  fn host_uri_error(&self, id: String, error: String, guard: Option<Guard>) {
    let result = HostUriResult {
      id,
      content: None,
      content_type: None,
      notes: None,
      immutable: None,
      is_error: Some(true),
      error: Some(error),
    };
    if let Err(error) = self.enqueue(&RpcInbound::HostUriResult(result), guard) {
      tracing::debug!(%error, "dropping the error result of a host URI request");
    }
  }
}

fn command_error(response: RpcResponse) -> Error {
  Error::Command {
    command: response.command,
    error: response.error.unwrap_or_default(),
    code: response.code,
  }
}

/// Removes a prompt collector when the wait ends, however it ends.
struct Collector<'a> {
  shared: &'a Shared,
  id: &'a str,
}

impl Drop for Collector<'_> {
  fn drop(&mut self) {
    self.shared.state().prompts.remove(self.id);
  }
}

/// Removes a pending call when its future ends, however it ends, so a cancelled call leaves no entry behind.
struct Pending<'a> {
  shared: &'a Shared,
  id: &'a str,
}

impl Drop for Pending<'_> {
  fn drop(&mut self) {
    self.shared.state().pending.remove(self.id);
  }
}

/// The spawned server and what it takes to bring its process tree down.
struct Spawned {
  child: Child,
  pid: u32,
  /// The server leads its own process group, whose id is the leader's pid.
  #[cfg(unix)]
  group: rustix::process::Pid,
  /// Closing the job kills every process the server started.
  #[cfg(windows)]
  job: win32job::Job,
}

impl Spawned {
  /// Takes the process tree down and confirms it is gone.
  async fn terminate(mut self) -> Result<(), Error> {
    #[cfg(unix)]
    return terminate(&mut self.child, self.group).await;
    #[cfg(windows)]
    return terminate(&mut self.child, self.job).await;
  }

  /// Ends the process tree without waiting; used when no runtime is left to await on.
  fn kill_now(self) {
    #[cfg(unix)]
    signal_group(self.group, rustix::process::Signal::KILL);
    // Dropping the job closes it, which kills the process tree.
    #[cfg(windows)]
    drop(self.job);
  }
}

/// What a client has to tear down: the server (when it spawned one) and the reader task.
struct Running {
  process: Option<Spawned>,
  reader: JoinHandle<()>,
}

impl Running {
  async fn finish(mut self) -> Result<(), Error> {
    let outcome = match self.process.take() {
      Some(process) => process.terminate().await,
      None => Ok(()),
    };
    // The server is gone, so its stdout reaches EOF; a surviving process that inherited stdout may keep it open, so do
    // not wait forever.
    match timeout(READER_JOIN_TIMEOUT, &mut self.reader).await {
      Ok(Ok(())) => {}
      Ok(Err(error)) => tracing::debug!(%error, "the omp reader task ended abnormally"),
      Err(_elapsed) => self.reader.abort(),
    }
    outcome
  }

  fn kill_now(self) {
    if let Some(process) = self.process {
      process.kill_now();
    }
    self.reader.abort();
  }
}

/// A connected `omp --mode rpc` server.
pub struct Client {
  shared: Arc<Shared>,
  /// Taken by [`Client::close`] and drop.
  running: Option<Running>,
  next_id: AtomicU64,
  ready: ReadyEvent,
  protocol_version: u32,
  /// Serializes each host tool / URI replacement registration round trip, so the server and the dispatch registry agree.
  registration: tokio::sync::Mutex<()>,
}

/// Builds the command frame `{"id", "type", ...params}`.
pub fn encode_command<C: Command>(id: &str, command: &C) -> Result<Value, serde_json::Error> {
  let mut frame = serde_json::to_value(command)?;
  let Value::Object(map) = &mut frame else {
    return Err(serde::ser::Error::custom("command parameters must serialize to an object"));
  };
  map.insert("id".to_owned(), Value::String(id.to_owned()));
  map.insert("type".to_owned(), Value::String(C::NAME.to_owned()));
  Ok(frame)
}

impl Client {
  /// Spawns `process` with piped stdin/stdout and connects to it (see [`Client::from_io`]).
  ///
  /// The server leads its own process group on Unix and runs in a job object on Windows, so teardown ([`Client::close`]
  /// or drop) also reaches the processes it spawned.
  pub async fn spawn(mut process: Process, options: ClientOptions) -> Result<(Self, mpsc::UnboundedReceiver<Event>), Error> {
    #[cfg(unix)]
    process.process_group(0);
    process.stdin(Stdio::piped()).stdout(Stdio::piped()).kill_on_drop(true);
    let mut child = process.spawn()?;
    let pid = child.id().ok_or(Error::Closed)?;
    #[cfg(unix)]
    let group = i32::try_from(pid)
      .ok()
      .and_then(rustix::process::Pid::from_raw)
      .ok_or_else(|| io::Error::other("the server's pid is not a valid process group id"))?;
    #[cfg(windows)]
    let job = contain(&child)?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let spawned = Spawned {
      child,
      pid,
      #[cfg(unix)]
      group,
      #[cfg(windows)]
      job,
    };
    let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
      spawned.terminate().await?;
      return Err(Error::Closed);
    };
    Self::connect(stdout, stdin, options, Some(spawned)).await
  }

  /// Connects over the server's stdout (`reader`) and stdin (`writer`).
  ///
  /// Waits for `ready`, negotiates protocol v2 when the server advertises it with the standard limits, then registers
  /// `options.tools` and `options.uris`. Returns the client and the receiver of every frame that is not a correlated
  /// response or a host tool/URI frame (`ready` included).
  ///
  /// A generic reader cannot be interrupted: after the client closes, its reader task stops dispatching and ends once
  /// the peer closes its output.
  pub async fn from_io<R, W>(reader: R, writer: W, options: ClientOptions) -> Result<(Self, mpsc::UnboundedReceiver<Event>), Error>
  where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
  {
    Self::connect(reader, writer, options, None).await
  }

  async fn connect<R, W>(
    reader: R,
    writer: W,
    options: ClientOptions,
    process: Option<Spawned>,
  ) -> Result<(Self, mpsc::UnboundedReceiver<Event>), Error>
  where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
  {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (outgoing, queue) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
      state: Mutex::new(State {
        outgoing: Some(outgoing),
        ready: Some(ready_tx),
        ..State::default()
      }),
      v2: AtomicBool::new(false),
      tools: Mutex::new(Vec::new()),
      uris: Mutex::new(Vec::new()),
    });
    // Detached: a write blocked on a peer that never reads must not hold up teardown; the task ends when the write
    // fails or the queue closes.
    tokio::spawn(Shared::write_loop(Arc::downgrade(&shared), writer, queue));
    let (events, receiver) = mpsc::unbounded_channel();
    let reader_shared = Arc::clone(&shared);
    let reader = tokio::spawn(async move {
      let closure = reader_shared.read_loop(BufReader::with_capacity(1 << 16, reader), &events).await;
      reader_shared.close(closure);
    });
    let mut client = Self {
      ready: ReadyEvent {
        protocol_version: None,
        supported_protocol_versions: None,
        max_frame_bytes: None,
        max_reassembled_frame_bytes: None,
        supports_bash_exclude_from_context: None,
      },
      shared,
      running: Some(Running { process, reader }),
      next_id: AtomicU64::new(1),
      protocol_version: 1,
      registration: tokio::sync::Mutex::new(()),
    };
    // From here on an early return drops `client`, which tears everything down.
    client.ready = match timeout(options.ready_timeout, ready_rx).await {
      Ok(Ok(ready)) => ready,
      Ok(Err(_closed)) => {
        return Err(client.shared.state().closed.as_ref().map_or(Error::Closed, Closure::error));
      }
      Err(_elapsed) => {
        return Err(Error::Timeout {
          command: "ready".to_owned(),
        });
      }
    };
    let ready = &client.ready;
    if ready
      .supported_protocol_versions
      .as_ref()
      .is_some_and(|versions| versions.contains(&2))
      && ready.max_frame_bytes == Some(MAX_FRAME_BYTES)
      && ready.max_reassembled_frame_bytes == Some(MAX_REASSEMBLED_BYTES)
    {
      // Chunks may follow the negotiation response immediately.
      client.shared.v2.store(true, Ordering::SeqCst);
      let negotiated = client.call(&NegotiateProtocolCommand { protocol_version: 2 }).await?;
      if negotiated.protocol_version != 2 {
        return Err(Error::Protocol("RPC protocol v2 negotiation failed".to_owned()));
      }
      client.protocol_version = 2;
    }
    if !options.tools.is_empty() {
      client.set_custom_tools(options.tools).await?;
    }
    if !options.uris.is_empty() {
      client.set_host_uris(options.uris).await?;
    }
    Ok((client, receiver))
  }

  /// Process id of the spawned server (on Unix also its process group id); `None` for [`Client::from_io`].
  pub fn pid(&self) -> Option<u32> {
    self
      .running
      .as_ref()
      .and_then(|running| running.process.as_ref())
      .map(|process| process.pid)
  }

  /// The server's `ready` frame.
  pub fn ready(&self) -> &ReadyEvent {
    &self.ready
  }

  /// Negotiated protocol version: 2 when the server supports lossless chunking, else 1.
  pub fn protocol_version(&self) -> u32 {
    self.protocol_version
  }

  fn next_id(&self) -> String {
    format!("req_{}", self.next_id.fetch_add(1, Ordering::Relaxed))
  }

  /// Sends `command` and awaits its response, matched by id. There is no deadline of the client's own: a command runs
  /// as long as omp runs it, and a dead server fails every call in flight.
  pub async fn call<C: Command>(&self, command: &C) -> Result<C::Output, Error> {
    let id = self.next_id();
    let response = self.request(&id, C::NAME, &encode_command(&id, command)?).await?;
    Ok(C::decode(response.data)?)
  }

  /// Queues `frame` and awaits its successful response. Dropping the future withdraws the request's entry.
  async fn request(&self, id: &str, command: &str, frame: &Value) -> Result<RpcResponse, Error> {
    let (tx, rx) = oneshot::channel();
    self.shared.state().pending.insert(id.to_owned(), (command.to_owned(), tx));
    let _pending = Pending { shared: &self.shared, id };
    self.shared.enqueue(frame, None)?;
    let response = match rx.await {
      Ok(response) => response?,
      Err(_closed) => return Err(Error::Closed),
    };
    if !response.success {
      return Err(command_error(response));
    }
    Ok(response)
  }

  /// Queues a host frame (an extension UI response); returns once queued, without waiting for the write. Fails once the
  /// client is closed.
  pub fn send(&self, frame: &RpcInbound) -> Result<(), Error> {
    self.shared.enqueue(frame, None)
  }

  /// Replaces the host tools (`set_host_tools`); returns the names the server registered.
  pub async fn set_custom_tools(&self, tools: Vec<HostTool>) -> Result<Vec<String>, Error> {
    let _registration = self.registration.lock().await;
    let definitions = tools.iter().map(|tool| tool.definition().clone()).collect();
    *self.shared.tools.lock().unwrap_or_else(PoisonError::into_inner) = tools;
    self.call(&SetHostToolsCommand { tools: definitions }).await
  }

  /// Replaces the host URI schemes (`set_host_uri_schemes`); returns the schemes the server registered.
  pub async fn set_host_uris(&self, uris: Vec<HostUri>) -> Result<Vec<String>, Error> {
    let _registration = self.registration.lock().await;
    let schemes = uris.iter().map(HostUri::definition).collect();
    *self.shared.uris.lock().unwrap_or_else(PoisonError::into_inner) = uris;
    self.call(&SetHostUriSchemesCommand { schemes }).await
  }

  /// Submits `prompt` and waits (up to `timeout`) for its own `prompt_result`.
  ///
  /// Waits are independent: each collects the session events that arrive while its prompt runs and ends only on the
  /// `prompt_result` carrying its request id, so concurrent waits are allowed. Events still reach the event receiver.
  pub async fn prompt_and_wait(&self, prompt: &PromptCommand, timeout: Duration) -> Result<PromptTurn, Error> {
    let deadline = Instant::now() + timeout;
    let id = self.next_id();
    let frame = encode_command(&id, prompt)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // Registered before writing: the result may arrive before the acknowledgement.
    self.shared.state().prompts.insert(id.clone(), tx);
    let _collector = Collector {
      shared: &self.shared,
      id: &id,
    };
    let expired = || Error::Timeout {
      command: PromptCommand::NAME.to_owned(),
    };
    let response = timeout_at(deadline, self.request(&id, PromptCommand::NAME, &frame))
      .await
      .map_err(|_elapsed| expired())??;
    let ack = PromptCommand::decode(response.data)?;
    let mut events = Vec::new();
    if ack.agent_invoked == Some(false) {
      while let Ok(signal) = rx.try_recv() {
        if let PromptSignal::Event(event) = signal {
          events.push(*event);
        }
      }
      return PromptTurn::build(events, None);
    }
    loop {
      match timeout_at(deadline, rx.recv()).await {
        Ok(Some(PromptSignal::Event(event))) => events.push(*event),
        Ok(Some(PromptSignal::Result(result))) => return PromptTurn::build(events, Some(*result)),
        Ok(Some(PromptSignal::Failed(error))) => return Err(error),
        Ok(None) => return Err(Error::Closed),
        Err(_elapsed) => return Err(expired()),
      }
    }
  }

  /// Every message of the session. With protocol v2 it drains `get_messages_page`, falling back to `get_messages` when
  /// the session changes mid-way; with v1 it sends `get_messages`.
  pub async fn get_messages(&self) -> Result<Vec<AgentMessage>, Error> {
    if self.protocol_version == 2 {
      match self.get_message_pages().await {
        Err(Error::Command { command, error, code })
          if command == GetMessagesPageCommand::NAME
            && (matches!(code.as_deref(), Some("session_busy" | "stale_cursor"))
              || error == PAGE_BUSY_ERROR
              || error == PAGE_STALE_ERROR) => {}
        result => return result,
      }
    }
    self.call(&GetMessagesCommand {}).await
  }

  async fn get_message_pages(&self) -> Result<Vec<AgentMessage>, Error> {
    let mut messages = Vec::new();
    let mut seen = HashSet::new();
    let mut total = None;
    let mut cursor = None;
    loop {
      let page = self
        .call(&GetMessagesPageCommand {
          cursor: cursor.take(),
          limit: Some(PAGE_LIMIT),
        })
        .await?;
      if total.is_some_and(|total| total != page.total_messages) {
        return Err(Error::Protocol("RPC message pagination returned an inconsistent total".to_owned()));
      }
      total = Some(page.total_messages);
      messages.extend(page.messages);
      let Some(next) = page.next_cursor else { break };
      if !seen.insert(next.clone()) {
        return Err(Error::Protocol("RPC message pagination repeated a cursor".to_owned()));
      }
      cursor = Some(next);
    }
    if i64::try_from(messages.len()).ok() != total {
      return Err(Error::Protocol(
        "RPC message pagination ended before the advertised total".to_owned(),
      ));
    }
    Ok(messages)
  }

  /// Closes the client and tears the server down: cancels host work, closes stdin, then signals the server's process
  /// group SIGTERM and, if the leader is still alive after a second, SIGKILL. Returns an error when teardown cannot
  /// confirm the leader is reaped and the group empty. Dropping the client runs the same teardown and ignores the
  /// outcome.
  pub async fn close(mut self) -> Result<(), Error> {
    self.shared.close(Closure::Closed);
    match self.running.take() {
      Some(running) => running.finish().await,
      None => Ok(()),
    }
  }
}

impl Drop for Client {
  fn drop(&mut self) {
    self.shared.close(Closure::Closed);
    let Some(running) = self.running.take() else {
      return;
    };
    match tokio::runtime::Handle::try_current() {
      Ok(runtime) => drop(runtime.spawn(async move {
        if let Err(error) = running.finish().await {
          tracing::warn!(%error, "tearing down a dropped omp client");
        }
      })),
      Err(_no_runtime) => running.kill_now(),
    }
  }
}

/// Waits for the leader to exit, up to `grace`; true once it has.
async fn leader_exited(child: &mut Child, grace: Duration) -> Result<bool, Error> {
  if grace.is_zero() {
    return Ok(child.try_wait()?.is_some());
  }
  match timeout(grace, child.wait()).await {
    Ok(status) => status.map(|_status| true).map_err(Error::from),
    Err(_elapsed) => Ok(false),
  }
}

/// Sends `signal` to every process in `group`. A group that is already empty is the expected outcome.
#[cfg(unix)]
fn signal_group(group: rustix::process::Pid, signal: rustix::process::Signal) {
  match rustix::process::kill_process_group(group, signal) {
    Ok(()) => {}
    Err(errno) if errno == rustix::io::Errno::SRCH => {}
    Err(errno) => tracing::debug!(%errno, "signalling the omp process group failed"),
  }
}

/// Whether no process is left in `group`.
#[cfg(unix)]
fn group_gone(group: rustix::process::Pid) -> bool {
  matches!(rustix::process::test_kill_process_group(group), Err(errno) if errno == rustix::io::Errno::SRCH)
}

/// Terminates the server and every process in its group, escalating SIGTERM to SIGKILL; mirrors the Python client's
/// `_terminate_process_group`. The group id is the leader's pid and stays valid while any member lives, even after the
/// leader is reaped.
#[cfg(unix)]
async fn terminate(child: &mut Child, group: rustix::process::Pid) -> Result<(), Error> {
  use rustix::process::Signal;

  if leader_exited(child, Duration::ZERO).await? && group_gone(group) {
    return Ok(());
  }
  signal_group(group, Signal::TERM);
  if leader_exited(child, LEADER_EXIT_TIMEOUT).await? && group_gone(group) {
    return Ok(());
  }
  signal_group(group, Signal::KILL);
  if !leader_exited(child, LEADER_EXIT_TIMEOUT).await? {
    return Err(io::Error::other("RPC server survived SIGKILL").into());
  }
  // Killed members linger until their new parent reaps them, and the kernel announces no change: poll, bounded.
  let settled = timeout(KILL_SETTLE_TIMEOUT, async {
    while !group_gone(group) {
      sleep(GROUP_POLL_INTERVAL).await;
    }
  })
  .await;
  match settled {
    Ok(()) => Ok(()),
    Err(_elapsed) => Err(io::Error::other("RPC server process group survived SIGKILL").into()),
  }
}

/// Puts the server in a job object that kills every process in it when closed.
#[cfg(windows)]
fn contain(child: &Child) -> Result<win32job::Job, Error> {
  let mut limits = win32job::ExtendedLimitInfo::new();
  limits.limit_kill_on_job_close();
  let job = win32job::Job::create_with_limit_info(&limits)?;
  let handle = child.raw_handle().ok_or(Error::Closed)?;
  job.assign_process(handle as isize)?;
  Ok(job)
}

/// Terminates the server, then closes the job, which kills every process it started.
#[cfg(windows)]
async fn terminate(child: &mut Child, job: win32job::Job) -> Result<(), Error> {
  if let Err(error) = child.start_kill() {
    tracing::debug!(%error, "the omp server was already gone");
  }
  let exited = leader_exited(child, LEADER_EXIT_TIMEOUT).await?;
  drop(job);
  if exited {
    Ok(())
  } else {
    Err(io::Error::other("RPC server survived kill").into())
  }
}
