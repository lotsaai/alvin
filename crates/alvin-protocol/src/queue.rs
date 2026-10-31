//! Editing the steering and follow-up queue. omp identifies a queued message by its text, not by an id; the current queue
//! arrives as a `queue_update` event and in `live/state`.

use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::ids::LiveSessionId;
use crate::omp_wire::{AbortAndRestoreQueueResult, PromoteQueuedMessageResult, QueuedMessageQueue, RemoveQueuedMessageResult};

/// `queue/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveQueuedRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// The queued message's text.
  pub message: String,
  /// Which queue it is in.
  pub queue: QueuedMessageQueue,
}

/// `queue/promote`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteQueuedRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// The queued follow-up's text.
  pub message: String,
}

/// The steering and follow-up queue.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "queue", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "queue", namespace_separator = "/"))]
pub trait QueueApi {
  /// Takes a message out of a queue. Messages already steered into the running response cannot be removed.
  #[method(name = "remove")]
  async fn remove(&self, request: RemoveQueuedRequest) -> jsonrpsee::core::RpcResult<RemoveQueuedMessageResult>;

  /// Moves a queued follow-up to the steering queue.
  #[method(name = "promote")]
  async fn promote(&self, request: PromoteQueuedRequest) -> jsonrpsee::core::RpcResult<PromoteQueuedMessageResult>;

  /// Aborts the running turn and returns the queued messages that were withdrawn, oldest first, so the composer can offer
  /// them again.
  #[method(name = "restore")]
  async fn restore(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<AbortAndRestoreQueueResult>;
}
