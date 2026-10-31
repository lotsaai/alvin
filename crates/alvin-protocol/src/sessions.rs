//! Sessions of a project, as listed by `sessions/list`.

use std::path::PathBuf;

use jiff::Timestamp;
use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::ids::{CheckoutId, ProjectId, SessionId};

/// How a session's transcript ends, derived from the tail of its file the way omp derives it. A final message
/// larger than the tail window yields [`SessionStatus::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
  /// The agent finished its last turn and yielded to the user.
  Complete,
  /// The last turn was cut off: unanswered tool calls, tool results without a follow-up, or a length stop.
  Interrupted,
  /// The last turn was aborted.
  Aborted,
  /// The last turn ended in an error.
  Error,
  /// The user's last message has no reply.
  Pending,
  /// The tail holds no message to classify.
  Unknown,
}

/// One omp session in a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
  /// The id omp wrote in the session header.
  pub id: SessionId,
  /// The session file.
  pub path: PathBuf,
  /// The display title: the title slot or header title, else the latest compaction summary, else the first user
  /// message. `None` when the session has none of them.
  pub title: Option<String>,
  /// When the session started, from the header.
  pub created: Timestamp,
  /// The file's modification time.
  pub modified: Timestamp,
  /// How the transcript ends.
  pub status: SessionStatus,
  /// The session file this session branched from, when it did.
  pub parent: Option<PathBuf>,
}

/// A position in the newest-first listing: the last session of the previous page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCursor {
  /// The `modified` time of the last session of the previous page.
  pub modified: Timestamp,
  /// The id of the last session of the previous page.
  pub id: SessionId,
}

/// Parameters of `sessions/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessions {
  /// The project to list.
  pub project: ProjectId,
  /// Restrict the listing to one checkout of the project.
  pub checkout: Option<CheckoutId>,
  /// Continue after this position; `None` starts at the newest session.
  pub after: Option<SessionCursor>,
  /// The page size, at least 1.
  pub limit: u32,
}

/// One page of a listing, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
  /// The sessions of this page.
  pub sessions: Vec<SessionSummary>,
  /// The cursor for the next page; `None` on the last page.
  pub next: Option<SessionCursor>,
}

/// Session listings, from omp's session files.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "sessions", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "sessions", namespace_separator = "/"))]
pub trait SessionsApi {
  /// One page of a project's sessions, newest first. Sessions omp itself would hide from its picker (no title,
  /// no message, nothing to classify) are omitted.
  #[method(name = "list")]
  async fn list(&self, request: ListSessions) -> jsonrpsee::core::RpcResult<SessionPage>;
}

#[cfg(test)]
mod tests {
  use super::{SessionCursor, SessionStatus};
  use crate::ids::SessionId;

  #[test]
  fn statuses_and_cursors_have_a_stable_wire_shape() {
    assert_eq!(
      serde_json::to_value(SessionStatus::Interrupted).unwrap(),
      serde_json::json!("interrupted")
    );
    let cursor = SessionCursor {
      modified: "2026-01-02T03:04:05.123456789Z".parse().unwrap(),
      id: SessionId::from("s1".to_owned()),
    };
    let json = serde_json::to_value(&cursor).unwrap();
    assert_eq!(
      json,
      serde_json::json!({ "modified": "2026-01-02T03:04:05.123456789Z", "id": "s1" })
    );
    assert_eq!(serde_json::from_value::<SessionCursor>(json).unwrap(), cursor);
  }
}
