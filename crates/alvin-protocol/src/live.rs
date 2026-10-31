//! Live sessions: one omp process each, opened, driven and closed through `live/*`, observed through `events/subscribe`.

use std::path::PathBuf;

use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::events::Event;
use crate::history::{EntriesRequest, HistoryEntries};
use crate::ids::{CheckoutId, EventId, LiveSessionId, ProjectId, SessionId};
use crate::omp_wire::{BashResult, MessagesPage, SessionState};

/// What `live/open` starts omp on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpenTarget {
  /// A new session in a checkout of a listed project.
  #[serde(rename_all = "camelCase")]
  New {
    /// The project the checkout belongs to.
    project: ProjectId,
    /// The checkout omp runs in.
    checkout: CheckoutId,
  },
  /// An existing session, as listed by `sessions/list`. Opening a session that is already live returns the live session
  /// it is in.
  #[serde(rename_all = "camelCase")]
  Session {
    /// The session to resume.
    session: SessionId,
  },
}

/// `live/open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
  /// What to open.
  pub target: OpenTarget,
  /// A model as `provider/id`. Absent: the gateway's default, else omp's own choice.
  pub model: Option<String>,
}

/// A live session as `live/open` and `live/list` report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSessionSummary {
  /// The live session.
  pub live: LiveSessionId,
  /// The working directory omp runs in.
  pub cwd: PathBuf,
  /// The session file omp writes.
  pub file: PathBuf,
  /// Whether the omp process has exited. The entry stays until `live/close` so a UI can show what happened.
  pub exited: bool,
}

/// `live/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagesRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// The cursor of the previous page, absent for the first page.
  pub cursor: Option<String>,
  /// The page size, at least 1. Absent: omp's default.
  pub limit: Option<u32>,
}

/// `live/prompt`. omp expands `/` commands, skills, templates and `^model` itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// What to send.
  pub message: String,
}

/// `live/bash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashRequest {
  /// The live session.
  pub live: LiveSessionId,
  /// The shell command, as the user typed it after `!`.
  pub command: String,
}

/// `events/subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
  /// The live session to observe.
  pub live: LiveSessionId,
  /// Resume after this event: the subscription first delivers the retained events newer than it, then live ones, with
  /// no gap and no duplicate. Absent: start with the retained log.
  pub after: Option<EventId>,
}

/// Live sessions.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "live", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "live", namespace_separator = "/"))]
pub trait LiveApi {
  /// Starts omp on a new or existing session, or returns the live session an existing one is already in.
  #[method(name = "open")]
  async fn open(&self, request: OpenRequest) -> jsonrpsee::core::RpcResult<LiveSessionSummary>;

  /// Closes the live session and its omp process. Removes it from the registry.
  #[method(name = "close")]
  async fn close(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<()>;

  /// Every live session, including ones whose omp has exited.
  #[method(name = "list")]
  async fn list(&self) -> jsonrpsee::core::RpcResult<Vec<LiveSessionSummary>>;

  /// omp's state of the session: model, thinking level, streaming and compacting flags, queue modes.
  #[method(name = "state")]
  async fn state(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<SessionState>;

  /// One page of the session's messages, read from omp.
  #[method(name = "messages")]
  async fn messages(&self, request: MessagesRequest) -> jsonrpsee::core::RpcResult<MessagesPage>;

  /// Submits a prompt. Its outcome arrives as a `PromptResult` event.
  #[method(name = "prompt")]
  async fn prompt(&self, request: PromptRequest) -> jsonrpsee::core::RpcResult<()>;

  /// Aborts the running turn.
  #[method(name = "abort")]
  async fn abort(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<()>;

  /// Queues a message that interrupts the running turn.
  #[method(name = "steer")]
  async fn steer(&self, request: PromptRequest) -> jsonrpsee::core::RpcResult<()>;

  /// Queues a message that is sent when the running turn finishes.
  #[method(name = "followUp")]
  async fn follow_up(&self, request: PromptRequest) -> jsonrpsee::core::RpcResult<()>;

  /// Aborts the running turn and sends a message in its place.
  #[method(name = "abortAndPrompt")]
  async fn abort_and_prompt(&self, request: PromptRequest) -> jsonrpsee::core::RpcResult<()>;

  /// Runs a shell command in the session and returns its output.
  #[method(name = "bash")]
  async fn bash(&self, request: BashRequest) -> jsonrpsee::core::RpcResult<BashResult>;

  /// Aborts the running `bash` command.
  #[method(name = "abortBash")]
  async fn abort_bash(&self, live: LiveSessionId) -> jsonrpsee::core::RpcResult<()>;

  /// The entries omp appended after one it names, oldest first, or all of them. The branches follow from `parent_id`. omp
  /// sends no event when history grows: read this after a turn.
  #[method(name = "entries")]
  async fn entries(&self, request: EntriesRequest) -> jsonrpsee::core::RpcResult<HistoryEntries>;
}

/// Live-session events.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "events", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "events", namespace_separator = "/"))]
pub trait EventsApi {
  /// Streams a live session's events. Fails with the stale-cursor code when `after` is older than the retained log. The
  /// stream ends when the subscriber falls behind or the live session closes; resubscribe with the last event id.
  #[subscription(name = "subscribe" => "event", unsubscribe = "unsubscribe", item = Event)]
  async fn events(&self, request: SubscribeRequest) -> jsonrpsee::core::SubscriptionResult;
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn an_open_request_has_a_stable_wire_shape() {
    let request = OpenRequest {
      target: OpenTarget::New {
        project: ProjectId::from("p".to_owned()),
        checkout: CheckoutId::from("c".to_owned()),
      },
      model: None,
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(
      json,
      json!({ "target": { "kind": "new", "project": "p", "checkout": "c" }, "model": null })
    );
    assert_eq!(serde_json::from_value::<OpenRequest>(json).unwrap(), request);

    let resume = OpenRequest {
      target: OpenTarget::Session {
        session: SessionId::from("s".to_owned()),
      },
      model: Some("zai/glm-5.3-flash".to_owned()),
    };
    let json = serde_json::to_value(&resume).unwrap();
    assert_eq!(
      json,
      json!({ "target": { "kind": "session", "session": "s" }, "model": "zai/glm-5.3-flash" })
    );
  }
}
