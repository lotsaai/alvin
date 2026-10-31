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

/// What the user sent: the text, and whether it was sent with the secondary key (Cmd or Ctrl), which queues it as a follow-up
/// instead of steering a running turn.
pub(crate) struct Submission {
  pub(crate) text: String,
  pub(crate) follow_up: bool,
}

/// What the user asked for when the composer reported an event: Enter, with or without the secondary key, on something other
/// than whitespace. Shift+Enter inserts a newline and sends nothing.
pub(crate) fn submitted(event: &InputEvent, state: &Entity<TextareaState>, cx: &App) -> Option<Submission> {
  match event {
    InputEvent::PressEnter { shift: false, secondary } => {
      let text = state.read(cx).value();
      let text = text.trim_end();
      (!text.trim().is_empty()).then(|| Submission {
        text: text.to_owned(),
        follow_up: *secondary,
      })
    }
    _ => None,
  }
}

/// The element for the composer's state.
pub(crate) fn view(state: &Entity<TextareaState>) -> Textarea {
  Textarea::new(state).accessibility_id("composer").aria_label("Message")
}
