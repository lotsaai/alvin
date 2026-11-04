//! Actions and the key bindings this crate registers.

use gpui_kit::{App, KeyBinding, actions};

actions!(
  alvin,
  [
    /// Aborts the running turn, or the running `!` command.
    Abort,
    /// Shows or hides the project sidebar.
    ToggleSidebar,
    /// Shows or hides the terminal.
    ToggleTerminal,
  ]
);

/// The key context of a session view. Bindings below apply while focus is inside it.
pub(crate) const SESSION_CONTEXT: &str = "AlvinSession";

/// The key context of the workspace.
pub(crate) const WORKSPACE_CONTEXT: &str = "AlvinWorkspace";

/// The platform's command key plus `b`.
#[cfg(target_os = "macos")]
const TOGGLE_SIDEBAR_KEY: &str = "cmd-b";
#[cfg(not(target_os = "macos"))]
const TOGGLE_SIDEBAR_KEY: &str = "ctrl-b";

#[cfg(target_os = "macos")]
const TOGGLE_TERMINAL_KEY: &str = "cmd-j";
#[cfg(not(target_os = "macos"))]
const TOGGLE_TERMINAL_KEY: &str = "ctrl-j";

/// Registers this crate's key bindings. Call after `gpui_kit::init`.
pub fn init(cx: &mut App) {
  crate::session::refresh_markdown_theme(cx);
  crate::session::init_timeline(cx);
  crate::session::init_shortcuts(cx);
  cx.bind_keys(crate::terminal::default_key_bindings());
  cx.bind_keys([
    KeyBinding::new("escape", Abort, Some(SESSION_CONTEXT)),
    KeyBinding::new(TOGGLE_SIDEBAR_KEY, ToggleSidebar, Some(WORKSPACE_CONTEXT)),
    KeyBinding::new(TOGGLE_TERMINAL_KEY, ToggleTerminal, Some(WORKSPACE_CONTEXT)),
  ]);
}
