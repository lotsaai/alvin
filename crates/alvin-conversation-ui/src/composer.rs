//! The composer: Enter sends, Shift+Enter inserts a newline.
//!
//! What the user types goes to omp unchanged, except a leading `!`, which runs the rest as a shell command. omp expands `/`
//! commands, skills, templates and `^model` itself; nothing here parses them.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::{App, AppContext as _, Context, Entity, Window};

/// The composer's text area.
pub(crate) fn state<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Entity<TextareaState> {
  cx.new(|cx| {
    TextareaState::new(window, cx)
      .auto_grow(2, 8)
      .placeholder("Message omp, or !command")
      .submit_on_enter(true)
  })
}

/// What the user asked for when the composer reported an event: the text to send, if Enter without Shift was pressed on
/// something other than whitespace.
pub(crate) fn submitted(event: &InputEvent, state: &Entity<TextareaState>, cx: &App) -> Option<String> {
  match event {
    InputEvent::PressEnter { shift: false, .. } => {
      let text = state.read(cx).value();
      let text = text.trim_end();
      (!text.trim().is_empty()).then(|| text.to_owned())
    }
    _ => None,
  }
}

/// The element for the composer's state.
pub(crate) fn view(state: &Entity<TextareaState>) -> Textarea {
  Textarea::new(state).accessibility_id("composer").aria_label("Message")
}
