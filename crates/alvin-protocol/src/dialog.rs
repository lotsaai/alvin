//! Answering the dialogs extensions and tools open in a live session.

use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::ids::LiveSessionId;
use crate::omp_wire::ExtensionUiResponse;

/// `dialog/respond`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RespondRequest {
  /// The live session the dialog belongs to.
  pub live: LiveSessionId,
  /// The answer, or the dismissal. Its `id` names the request it answers.
  pub response: ExtensionUiResponse,
}

/// Dialogs of a live session: `select`, `confirm`, `input`, `editor` and `ask` requests arrive as events.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "dialog", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "dialog", namespace_separator = "/"))]
pub trait DialogApi {
  /// Answers or dismisses the dialog the request names. An answer to a dialog omp already closed is ignored by omp.
  #[method(name = "respond")]
  async fn respond(&self, request: RespondRequest) -> jsonrpsee::core::RpcResult<()>;
}
