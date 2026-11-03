//! Interactive terminal requests and ordered output events.

use std::num::NonZeroU16;

use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::{EventId, OpenTarget, TerminalId};

/// Terminal dimensions in cells and pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSize {
  /// Number of columns.
  pub columns: NonZeroU16,
  /// Number of rows.
  pub rows: NonZeroU16,
  /// Viewport width in pixels.
  pub pixel_width: u16,
  /// Viewport height in pixels.
  pub pixel_height: u16,
}

/// Opens a shell or resumes its event stream into the same retained emulator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TerminalSubscription {
  /// Open a shell in a gateway-resolved target.
  Open {
    /// Target checkout or existing session.
    target: OpenTarget,
    /// Initial terminal size.
    size: TerminalSize,
  },
  /// Continue after the last event accepted by the retained emulator.
  Resume {
    /// The terminal to resume.
    terminal: TerminalId,
    /// Last event accepted by the emulator, or none if it accepted no events.
    after: Option<EventId>,
  },
}

/// One ordered terminal event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalEvent {
  /// Gateway-minted event identifier.
  pub id: EventId,
  /// Terminal that emitted the event.
  pub terminal: TerminalId,
  /// Event payload.
  pub body: TerminalEventBody,
}

/// Output and lifecycle changes from a gateway-owned shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TerminalEventBody {
  /// The shell started; output follows this event.
  Started,
  /// Raw terminal output, in stream order.
  Output {
    /// Bytes to feed to the terminal emulator.
    bytes: Vec<u8>,
  },
  /// The shell exited after all output was delivered.
  Exited {
    /// Exit code, when available.
    code: Option<u32>,
    /// Signal name, when available.
    signal: Option<String>,
  },
  /// The gateway could not continue the terminal stream.
  Failed {
    /// Stable JSON-RPC error code.
    code: i32,
    /// Error and its source chain.
    message: String,
  },
}

/// Terminal input request. A lost response leaves delivery indeterminate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInput {
  /// Destination terminal.
  pub terminal: TerminalId,
  /// Encoded input bytes.
  pub bytes: Vec<u8>,
}

/// Terminal resize request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResize {
  /// Destination terminal.
  pub terminal: TerminalId,
  /// New dimensions.
  pub size: TerminalSize,
}

/// Output accepted by the retained emulator's ordered queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAccepted {
  /// The terminal whose events were accepted.
  pub terminal: TerminalId,
  /// Last event accepted in order.
  pub through: EventId,
}

/// Gateway-owned interactive terminals.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "terminal", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "terminal", namespace_separator = "/"))]
pub trait TerminalApi {
  /// Opens a shell or resumes a retained shell's ordered output.
  #[subscription(name = "subscribe" => "event", unsubscribe = "unsubscribe", item = TerminalEvent)]
  async fn terminal_events(&self, request: TerminalSubscription) -> jsonrpsee::core::SubscriptionResult;

  /// Writes input once. A transport error does not establish whether it was written.
  #[method(name = "input")]
  async fn input(&self, request: TerminalInput) -> jsonrpsee::core::RpcResult<()>;

  /// Applies new terminal dimensions.
  #[method(name = "resize")]
  async fn resize(&self, request: TerminalResize) -> jsonrpsee::core::RpcResult<()>;

  /// Acknowledges events accepted into the retained emulator's ordered queue.
  #[method(name = "accepted")]
  async fn accepted(&self, request: TerminalAccepted) -> jsonrpsee::core::RpcResult<()>;

  /// Terminates the shell and releases its resources.
  #[method(name = "close")]
  async fn close(&self, terminal: TerminalId) -> jsonrpsee::core::RpcResult<()>;
}
