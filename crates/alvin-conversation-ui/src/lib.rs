//! gpui-kit views over `alvin-conversation` state. The views render the model and send commands through `alvin-client`;
//! they hold no domain state, create no runtime and never see the gateway directly.

mod actions;
mod composer;
mod model;
mod pickers;
mod queue;
mod session;
mod sidebar;
#[cfg(test)]
mod test_support;
mod tool_card;
mod transcript;
mod workspace;

pub use actions::{Abort, ToggleSidebar, init};
pub use model::{Model, ModelEvent};
pub use workspace::Workspace;
