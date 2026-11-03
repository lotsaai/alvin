//! Retained terminal rendering and remote byte stream.
//!
//! Derived from gpui-cn e49d909b2ceae040714aa53cd38f97faa4f85cd2;
//! see terminal/NOTICE and accompanying licenses.

mod appearance;
mod block;
mod colors;
mod core;
mod cursor;
mod decoration;
mod element;
mod engine;
mod frame;
mod geometry;
mod ime;
mod input;
mod keys;
mod links;
mod options;
pub(crate) mod panel;
mod selection;
mod settings;
mod source;
mod state;
mod stream;

pub(crate) use element::{KEY_CONTEXT, Terminal};
pub(crate) use engine::{Engine, ExitStatus};
pub(crate) use frame::Viewport;
pub(crate) use options::EngineOptions;
pub use settings::{TerminalCursor, TerminalSettings, parse_terminal_color, parse_terminal_palette_entry};
pub(crate) use state::{TerminalConfig, TerminalEvent, TerminalState};
pub(crate) use stream::{StreamEvent, StreamEvents, StreamSource};

mod actions {
  gpui_kit::actions!(
    terminal,
    [
      Copy,
      Paste,
      SelectAll,
      Clear,
      ScrollToTop,
      ScrollToBottom,
      ScrollPageUp,
      ScrollPageDown,
      ScrollLineUp,
      ScrollLineDown,
      IncreaseFontSize,
      DecreaseFontSize,
      ResetFontSize
    ]
  );
}

pub(crate) fn default_key_bindings() -> Vec<gpui_kit::KeyBinding> {
  use actions::{
    Clear, Copy, DecreaseFontSize, IncreaseFontSize, Paste, ResetFontSize, ScrollLineDown, ScrollLineUp, ScrollPageDown, ScrollPageUp,
    ScrollToBottom, ScrollToTop, SelectAll,
  };
  use gpui_kit::KeyBinding;
  let context = Some(KEY_CONTEXT);
  let [copy, paste, select_all, clear, larger, plus, smaller, reset] = if cfg!(target_os = "macos") {
    ["cmd-c", "cmd-v", "cmd-a", "cmd-k", "cmd-=", "cmd-+", "cmd--", "cmd-0"]
  } else {
    [
      "ctrl-shift-c",
      "ctrl-shift-v",
      "ctrl-shift-a",
      "ctrl-shift-k",
      "ctrl-=",
      "ctrl-+",
      "ctrl--",
      "ctrl-0",
    ]
  };
  vec![
    KeyBinding::new(copy, Copy, context),
    KeyBinding::new(paste, Paste, context),
    KeyBinding::new(select_all, SelectAll, context),
    KeyBinding::new(clear, Clear, context),
    KeyBinding::new("shift-pageup", ScrollPageUp, context),
    KeyBinding::new("shift-pagedown", ScrollPageDown, context),
    KeyBinding::new("shift-home", ScrollToTop, context),
    KeyBinding::new("shift-end", ScrollToBottom, context),
    KeyBinding::new("shift-up", ScrollLineUp, context),
    KeyBinding::new("shift-down", ScrollLineDown, context),
    KeyBinding::new(larger, IncreaseFontSize, context),
    KeyBinding::new(plus, IncreaseFontSize, context),
    KeyBinding::new(smaller, DecreaseFontSize, context),
    KeyBinding::new(reset, ResetFontSize, context),
  ]
}
