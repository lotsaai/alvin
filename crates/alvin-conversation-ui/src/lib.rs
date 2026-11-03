//! gpui-kit views over `alvin-conversation` state. The views render the model and send commands through `alvin-client`;
//! they hold no domain state, create no runtime and never see the gateway directly.

mod actions;
mod ansi;
mod ask;
mod composer;
mod dialogs;
mod error;
mod history_tree;
mod markdown;
mod model;
mod pickers;
mod queue;
mod session;
mod sidebar;
mod status;
mod terminal;
mod terminal_panel;
mod terminal_settings;
#[cfg(test)]
mod test_support;
mod tool_card;
mod transcript;
mod workspace;

pub use actions::{Abort, ToggleSidebar, ToggleTerminal, init};
pub use error::{Error, Result};
pub use markdown::refresh_markdown_theme;
pub use model::{Model, ModelEvent};
pub use terminal_settings::{TerminalCursor, TerminalSettings, parse_terminal_color, parse_terminal_palette_entry};
pub use transcript::UserMessageColors;
pub use workspace::{ProjectPreference, ProjectPreferences, Workspace, WorkspaceEvent};
