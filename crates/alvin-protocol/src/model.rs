//! The model and thinking level of a live session.

use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::ids::LiveSessionId;
use crate::omp_wire::{ModelCycleResult, ModelInfo, ThinkingLevel, ThinkingLevelCycleResult};

/// `model/set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// The model's provider, as `model/list` reports it.
  pub provider: String,
  /// The model id, as `model/list` reports it.
  pub model_id: String,
}

/// `thinking/set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetThinkingRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// The level to use.
  pub level: ThinkingLevel,
}

/// The model and thinking level of a live session. The new values also arrive as events and in `live/state`.
#[cfg_attr(feature = "server", rpc(client, server))]
#[cfg_attr(not(feature = "server"), rpc(client))]
pub trait ModelApi {
  /// The models omp can use.
  #[method(name = "model/list")]
  async fn models(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<Vec<ModelInfo>>;

  /// Switches the session to a model and returns it.
  #[method(name = "model/set")]
  async fn set_model(&self, request: SetModelRequest) -> jsonrpsee::core::RpcResult<ModelInfo>;

  /// Switches to the next model in omp's cycle. `None` when there is no other model to cycle to.
  #[method(name = "model/cycle")]
  async fn cycle_model(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<Option<ModelCycleResult>>;

  /// The thinking levels the session's model accepts.
  #[method(name = "thinking/levels")]
  async fn thinking_levels(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<Vec<ThinkingLevel>>;

  /// Sets the thinking level.
  #[method(name = "thinking/set")]
  async fn set_thinking(&self, request: SetThinkingRequest) -> jsonrpsee::core::RpcResult<()>;

  /// Switches to the next thinking level. `None` when the model has none to cycle through.
  #[method(name = "thinking/cycle")]
  async fn cycle_thinking(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<Option<ThinkingLevelCycleResult>>;
}
