//! Host-owned tools the server calls back into (`set_host_tools`, `host_tool_call`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use omp_rpc::{HostToolDefinition, HostToolResultPayload, HostToolUpdate, RpcInbound, TextContent, ToolLoadMode, UserContent};
use serde_json::{Map, Value};

use super::client::{Error, Guard, Shared};

/// Error a handler returns; its text becomes the tool's error result.
pub type HostToolError = Box<dyn std::error::Error + Send + Sync>;

/// What a handler's future resolves to.
type HandlerFuture = Pin<Box<dyn Future<Output = Result<HostToolOutput, HostToolError>> + Send>>;

type Handler = dyn Fn(Map<String, Value>, HostToolContext) -> HandlerFuture + Send + Sync;

/// A tool definition plus the handler that runs it on the host.
#[derive(Clone)]
pub struct HostTool {
  definition: HostToolDefinition,
  handler: Arc<Handler>,
}

impl HostTool {
  /// `parameters` is the JSON Schema of the arguments object. The handler runs as a task of its own per call and
  /// receives the arguments object.
  pub fn new<F, Fut>(name: impl Into<String>, description: impl Into<String>, parameters: Map<String, Value>, handler: F) -> Self
  where
    F: Fn(Map<String, Value>, HostToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<HostToolOutput, HostToolError>> + Send + 'static,
  {
    Self {
      definition: HostToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters,
        label: None,
        hidden: Some(false),
        load_mode: None,
        reads_skill_uris: Some(false),
      },
      handler: Arc::new(move |arguments, context| Box::pin(handler(arguments, context))),
    }
  }

  /// Sets the label shown for the tool.
  pub fn label(mut self, label: impl Into<String>) -> Self {
    self.definition.label = Some(label.into());
    self
  }

  /// Hides the tool from the model's prompt.
  pub fn hidden(mut self, hidden: bool) -> Self {
    self.definition.hidden = Some(hidden);
    self
  }

  /// Sets how eagerly the tool loads.
  pub fn load_mode(mut self, mode: ToolLoadMode) -> Self {
    self.definition.load_mode = Some(mode);
    self
  }

  /// Declares that the tool reads skill URIs.
  pub fn reads_skill_uris(mut self, reads: bool) -> Self {
    self.definition.reads_skill_uris = Some(reads);
    self
  }

  /// The definition sent with `set_host_tools`.
  pub fn definition(&self) -> &HostToolDefinition {
    &self.definition
  }

  pub(crate) fn run(&self, arguments: Map<String, Value>, context: HostToolContext) -> HandlerFuture {
    (self.handler)(arguments, context)
  }
}

impl fmt::Debug for HostTool {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("HostTool")
      .field("definition", &self.definition)
      .finish_non_exhaustive()
  }
}

/// A handler's result: plain text or a full payload.
#[derive(Debug, Clone, PartialEq)]
pub enum HostToolOutput {
  /// Shortcut for `{content: [{type: "text", text}]}`.
  Text(String),
  /// The full result payload.
  Payload(HostToolResultPayload),
}

impl From<String> for HostToolOutput {
  fn from(text: String) -> Self {
    Self::Text(text)
  }
}

impl From<&str> for HostToolOutput {
  fn from(text: &str) -> Self {
    Self::Text(text.to_owned())
  }
}

impl From<HostToolResultPayload> for HostToolOutput {
  fn from(payload: HostToolResultPayload) -> Self {
    Self::Payload(payload)
  }
}

impl HostToolOutput {
  pub(crate) fn into_payload(self) -> HostToolResultPayload {
    match self {
      Self::Payload(payload) => payload,
      Self::Text(text) => text_payload(text, None),
    }
  }
}

pub(crate) fn text_payload(text: String, details: Option<Value>) -> HostToolResultPayload {
  HostToolResultPayload {
    content: vec![UserContent::Text(TextContent {
      text: Some(text),
      text_signature: None,
      extra: Map::new(),
    })],
    details,
    is_error: None,
    useless: None,
    provider_metadata: None,
  }
}

/// Per-call context handed to a handler.
pub struct HostToolContext {
  pub(crate) tool_call_id: String,
  pub(crate) request_id: String,
  pub(crate) cancelled: Arc<AtomicBool>,
  pub(crate) shared: Arc<Shared>,
}

impl HostToolContext {
  /// The agent's tool call id.
  pub fn tool_call_id(&self) -> &str {
    &self.tool_call_id
  }

  /// Whether the server cancelled the call or the client closed. A handler checks this at its own pace; nothing
  /// interrupts it.
  pub fn is_cancelled(&self) -> bool {
    self.cancelled.load(Ordering::SeqCst)
  }

  /// Streams partial output of the call to the server. A no-op once the call is cancelled, so a handler that re-checks
  /// [`HostToolContext::is_cancelled`] only at its loop head cannot send a stale update.
  pub fn send_update(&self, partial: impl Into<HostToolOutput>) -> Result<(), Error> {
    if self.is_cancelled() {
      return Ok(());
    }
    let update = RpcInbound::HostToolUpdate(HostToolUpdate {
      id: self.request_id.clone(),
      partial_result: partial.into().into_payload(),
    });
    let guard = Guard {
      cancelled: Arc::clone(&self.cancelled),
      release: None,
    };
    self.shared.enqueue(&update, Some(guard))
  }
}
