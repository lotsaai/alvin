//! The assistant message being built.
//!
//! omp sends the whole message so far with every `message_update`, so assembling it is replacing the previous snapshot; the
//! deltas that come with it are for views that animate. A snapshot belongs to one message, named by omp's `messageId`.

use alvin_protocol::AgentMessage;

/// A message between its `message_start` and `message_end`.
#[derive(Debug, Clone, PartialEq)]
pub struct Streaming {
  /// omp's id for the message, shared by its start, updates and end. Unique per process.
  pub message_id: Option<String>,
  /// The message as of the latest event.
  pub message: AgentMessage,
}
