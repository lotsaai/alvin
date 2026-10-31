//! The composer: Enter sends, Shift+Enter inserts a newline.
//!
//! What the user types goes to omp unchanged, except a leading `!`, which runs the rest as a shell command. omp expands `/`
//! commands, skills, templates and `^model` itself; nothing here parses them. The text is edited as markdown.

use std::rc::Rc;

use alvin_protocol::{AvailableSlashCommand, LiveSessionId};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{CompletionProvider, Editor, EditorState, InputEvent, Rope};
use gpui_kit::{App, AppContext as _, Context, Entity, Pixels, Result, Task, Window, rems};
use lsp_types::{CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Position, Range, TextEdit};

use crate::markdown;
use crate::model::Model;

/// How many rows the composer shows at least, and at most before its text scrolls.
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 8;

/// An editor leads its rows at this multiple of its font size.
const LINE_HEIGHT: f32 = 1.5;

/// The composer's editor: soft-wrapped markdown without a gutter, where Enter submits and a leading `/` offers the session's
/// slash commands.
pub(crate) fn state<T: 'static>(
  model: &Entity<Model>,
  live: LiveSessionId,
  window: &mut Window,
  cx: &mut Context<T>,
) -> Entity<EditorState> {
  let commands = Rc::new(SlashCommands {
    model: model.clone(),
    live,
  });
  cx.new(|cx| {
    let mut state = EditorState::new(window, cx)
      .language(markdown::LANGUAGE)
      .line_number(false)
      .folding(false)
      .placeholder("Message omp, or !command")
      .submit_on_enter(true);
    state.set_highlighter_factory(markdown::factory(), cx);
    state.lsp_mut().completion_provider = Some(commands);
    state
  })
}

/// Offers omp's slash commands for the word at the start of the text. omp expands them; this only completes the name.
struct SlashCommands {
  model: Entity<Model>,
  live: LiveSessionId,
}

impl CompletionProvider for SlashCommands {
  fn completions(
    &self,
    text: &Rope,
    offset: usize,
    _: CompletionContext,
    _: &mut Window,
    cx: &mut App,
  ) -> Task<Result<CompletionResponse>> {
    let items = self
      .model
      .read(cx)
      .conversation()
      .live(self.live)
      .map_or_else(Vec::new, |view| command_items(&view.commands, &text.to_string(), offset));
    Task::ready(Ok(CompletionResponse::Array(items)))
  }

  /// Any typed word: the menu refines as the name grows, and `completions` has nothing to offer outside a leading `/word`.
  fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
    !new_text.is_empty() && !new_text.contains(char::is_whitespace)
  }
}

/// The commands whose name starts with the `/word` before `offset`, as edits replacing that word. Nothing when the caret is
/// not at the end of a first word that starts with `/`.
fn command_items(commands: &[AvailableSlashCommand], text: &str, offset: usize) -> Vec<CompletionItem> {
  let Some(typed) = text.get(..offset).and_then(|before| before.strip_prefix('/')) else {
    return Vec::new();
  };
  if typed.contains(char::is_whitespace) {
    return Vec::new();
  }
  let end = Position::new(0, text[..offset].encode_utf16().count() as u32);
  commands
    .iter()
    .filter(|command| command.name.starts_with(typed))
    .map(|command| CompletionItem {
      label: format!("/{}", command.name),
      detail: command.description.clone(),
      text_edit: Some(CompletionTextEdit::Edit(TextEdit {
        range: Range::new(Position::new(0, 0), end),
        new_text: format!("/{} ", command.name),
      })),
      ..Default::default()
    })
    .collect()
}

/// What the user sent: the text, and whether they sent it with the secondary key (Cmd or Ctrl), which queues it as a
/// follow-up instead of steering the running turn.
pub(crate) struct Submission {
  pub(crate) text: String,
  pub(crate) follow_up: bool,
}

/// What the user asked for when the composer reported an event: Enter, with or without the secondary key, on something other
/// than whitespace. Shift+Enter inserts a newline and sends nothing.
pub(crate) fn submitted(event: &InputEvent, state: &Entity<EditorState>, cx: &App) -> Option<Submission> {
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

/// The element for the composer's state. An editor has no auto-grow, so its height follows the wrapped rows the last layout
/// counted, between [`MIN_ROWS`] and [`MAX_ROWS`].
pub(crate) fn view(state: &Entity<EditorState>, window: &Window, cx: &App) -> Editor {
  let rows = state.read(cx).display_map().wrap_row_count().clamp(MIN_ROWS, MAX_ROWS);
  let row: Pixels = cx.theme().mono_font_size * LINE_HEIGHT;
  // The editor's own vertical padding on top of its rows.
  let padding = rems(1.).to_pixels(window.rem_size());
  Editor::new(state).h(row * rows as f32 + padding).aria_label("Message")
}

#[cfg(test)]
mod tests {
  use alvin_protocol::AvailableSlashCommand;
  use lsp_types::CompletionTextEdit;

  use super::command_items;

  fn command(name: &str) -> AvailableSlashCommand {
    serde_json::from_value(serde_json::json!({ "name": name, "source": "builtin", "description": "does it" })).unwrap()
  }

  fn edits(text: &str, offset: usize) -> Vec<(String, String)> {
    let commands = [command("compact"), command("model"), command("move")];
    command_items(&commands, text, offset)
      .into_iter()
      .map(|item| match item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => (item.label, edit.new_text),
        other => panic!("a plain edit, got {other:?}"),
      })
      .collect()
  }

  #[test]
  fn a_leading_slash_word_completes_to_the_commands_it_prefixes() {
    assert_eq!(
      edits("/mo", 3),
      [("/model".into(), "/model ".into()), ("/move".into(), "/move ".into())]
    );
    assert_eq!(edits("/", 1).len(), 3);
  }

  #[test]
  fn nothing_completes_outside_a_leading_slash_word() {
    assert!(edits("hello /mo", 9).is_empty());
    assert!(edits("/model arg", 10).is_empty());
    assert!(edits("!ls", 3).is_empty());
    assert!(edits("/zzz", 4).is_empty());
  }
}
