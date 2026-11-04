//! gpui-kit views over `alvin-conversation` state. The views render the model and send commands through `alvin-client`;
//! they hold no domain state, create no runtime and never see the gateway directly.

mod actions;
mod conversation_preferences;
mod error;
mod model;
mod session;
mod terminal;
#[cfg(test)]
mod test_support;
mod workspace;

pub use actions::{Abort, ToggleSidebar, ToggleTerminal, init};
pub use conversation_preferences::ConversationPreferences;
pub use error::{Error, Result};
pub use model::{Model, ModelEvent};
pub use session::{UserMessageColors, refresh_markdown_theme};
pub use terminal::{TerminalCursor, TerminalSettings, parse_terminal_color, parse_terminal_palette_entry};
pub use workspace::{ProjectPreference, ProjectPreferences, Workspace, WorkspaceEvent};
