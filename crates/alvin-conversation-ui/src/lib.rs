//! gpui-kit views over `alvin-conversation` state. The views render the model and send commands through `alvin-client`;
//! they hold no domain state, create no runtime and never see the gateway directly.

mod actions;
mod ansi;
mod ask;
mod composer;
mod dialogs;
mod history_tree;
mod markdown;
mod model;
mod pickers;
mod queue;
mod session;
mod sidebar;
mod status;
#[cfg(test)]
mod test_support;
mod tool_card;
mod transcript;
mod workspace;

pub use actions::{Abort, ToggleSidebar, init};
pub use markdown::refresh_markdown_theme;
pub use model::{Model, ModelEvent};
pub use transcript::UserMessageColors;
pub use workspace::{ProjectPreference, ProjectPreferences, Workspace, WorkspaceEvent};
