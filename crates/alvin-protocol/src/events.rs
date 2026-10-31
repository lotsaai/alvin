//! What a live session emits: omp's frames that the protocol exposes, plus the host's own notices.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::omp_wire::{
  AvailableCommandsUpdateEvent, CommandOutputEvent, ConfigUpdateEvent, ExtensionError, ExtensionUiRequest, PromptResultEvent,
  RpcAgentEvent, RpcFrameErrorEvent, SessionInfoUpdateEvent, SessionSettledEvent,
};

/// The payload of one live-session event. The gateway stamps it with an event id and the live session id; omp's frames
/// arrive here without ids.
///
/// Frames omp sends that the protocol does not expose (python, live voice, dictation, btw, subagent, startup `ready`)
/// never become an `EventBody`; the host counts and logs them instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "frame", rename_all = "camelCase")]
pub enum EventBody {
  /// A session event: agent and turn boundaries, message and tool streaming, compaction, retry, model and queue changes.
  Agent(RpcAgentEvent),
  /// Terminal outcome of one accepted prompt.
  PromptResult(PromptResultEvent),
  /// Nothing can wake the session again; the gateway may close the process.
  SessionSettled(SessionSettledEvent),
  /// An extension asked the UI for input or showed it something.
  ExtensionUiRequest(ExtensionUiRequest),
  /// An extension failed.
  ExtensionError(ExtensionError),
  /// The slash-command catalog changed.
  AvailableCommandsUpdate(AvailableCommandsUpdateEvent),
  /// Output of a command omp ran on its own.
  CommandOutput(CommandOutputEvent),
  /// The session's title or metadata changed.
  SessionInfoUpdate(SessionInfoUpdateEvent),
  /// omp's configuration changed.
  ConfigUpdate(Box<ConfigUpdateEvent>),
  /// omp reported that it could not encode one of its own frames.
  RpcFrameError(RpcFrameErrorEvent),
  /// A frame of a known kind whose payload did not decode. Carries the raw frame and the decoder's complaint.
  Undecodable {
    /// The frame as omp sent it.
    raw: Value,
    /// Why it did not decode.
    error: String,
  },
  /// The omp process ended without being closed by the host.
  ProcessExited,
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn an_event_body_has_a_stable_wire_shape() {
    let json = serde_json::to_value(EventBody::ProcessExited).unwrap();
    assert_eq!(json, json!({ "kind": "processExited" }));

    let body = EventBody::Undecodable {
      raw: json!({ "type": "turn_end" }),
      error: "missing field".to_owned(),
    };
    let json = serde_json::to_value(&body).unwrap();
    assert_eq!(
      json,
      json!({ "kind": "undecodable", "frame": { "raw": { "type": "turn_end" }, "error": "missing field" } })
    );
    assert_eq!(serde_json::from_value::<EventBody>(json).unwrap(), body);
  }
}
