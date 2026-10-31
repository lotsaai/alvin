//! Pure UI state: projects, sessions and live sessions, folded from the protocol's responses and events. No IO, no async.

mod conversation;
mod dialogs;
mod live;
mod queue;
mod streaming;
mod tools;
mod tree;
mod workspace;

pub use conversation::{Connection, Conversation};
pub use dialogs::{Dialogs, request_id};
pub use live::{Compaction, Level, LiveView, Notice, Origin, Retry, Run, Widget};
pub use queue::Queue;
pub use streaming::Streaming;
pub use tools::{ToolExecution, ToolOutcome, Tools};
pub use tree::{Node, Tree};
pub use workspace::{SessionList, SessionsKey, Workspace};
