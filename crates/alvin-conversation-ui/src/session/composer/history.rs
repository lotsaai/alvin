//! Browse and restore the current session's submitted user prompts.

use std::cell::Cell;

use alvin_protocol::{AgentMessage, LiveSessionId, MessageContent, UserContent};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
  App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
  Task, Window, div, rems,
};

use crate::model::Model;

pub(super) fn prompts(model: &Model, live: LiveSessionId) -> Vec<String> {
  model
    .conversation()
    .live(live)
    .into_iter()
    .flat_map(|view| &view.transcript)
    .filter_map(|message| {
      let AgentMessage::User(user) = message else { return None };
      let text = match user.content.as_ref()? {
        MessageContent::String(text) => text.clone(),
        MessageContent::UserContentList(parts) => parts
          .iter()
          .filter_map(|part| match part {
            UserContent::Text(text) => text.text.as_deref(),
            UserContent::Image(_) => None,
          })
          .collect::<Vec<_>>()
          .join("\n"),
      };
      (!text.is_empty()).then_some(text)
    })
    .collect()
}

pub(super) struct Browse {
  prompts: Vec<String>,
  position: usize,
  draft: String,
}

impl Browse {
  pub(super) fn new(prompts: Vec<String>, draft: String) -> Option<Self> {
    (!prompts.is_empty()).then_some(Self {
      position: prompts.len(),
      prompts,
      draft,
    })
  }

  pub(super) fn previous(&mut self) -> Option<String> {
    self.position = self.position.saturating_sub(1);
    self.prompts.get(self.position).cloned()
  }

  pub(super) fn next(&mut self) -> Option<String> {
    if self.position >= self.prompts.len() {
      return None;
    }
    self.position += 1;
    Some(self.prompts.get(self.position).cloned().unwrap_or_else(|| self.draft.clone()))
  }
}

struct Prompts {
  prompts: Vec<String>,
  filtered: Vec<usize>,
  selected: Option<usize>,
}

impl ListDelegate for Prompts {
  type Item = ListItem;

  fn items_count(&self, _: usize, _: &App) -> usize {
    self.filtered.len()
  }

  fn perform_search(&mut self, query: &str, _: &mut Window, cx: &mut Context<ListState<Self>>) -> Task<()> {
    let query = query.to_lowercase();
    self.filtered = self
      .prompts
      .iter()
      .enumerate()
      .rev()
      .filter_map(|(ix, text)| text.to_lowercase().contains(&query).then_some(ix))
      .collect();
    self.selected = None;
    cx.notify();
    Task::ready(())
  }

  fn set_selected_index(&mut self, ix: Option<IndexPath>, _: &mut Window, cx: &mut Context<ListState<Self>>) {
    self.selected = ix.and_then(|ix| self.filtered.get(ix.row)).copied();
    cx.notify();
  }

  fn render_item(&mut self, ix: IndexPath, _: &mut Window, _: &mut Context<ListState<Self>>) -> Option<ListItem> {
    let index = *self.filtered.get(ix.row)?;
    let text = self.prompts.get(index)?;
    Some(
      ListItem::new(SharedString::from(format!("prompt-{index}")))
        .accessibility_label(text.clone())
        .child(text.clone()),
    )
  }

  fn render_empty(&mut self, _: &mut Window, cx: &mut Context<ListState<Self>>) -> impl IntoElement {
    div().p_4().text_color(cx.theme().muted_foreground).child("No matching prompts")
  }
}

struct HistoryDialog {
  list: Entity<ListState<Prompts>>,
  editor: Entity<EditorState>,
  _subscription: Subscription,
}

impl HistoryDialog {
  fn new(prompts: Vec<String>, editor: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let filtered = (0..prompts.len()).rev().collect();
    let list = cx.new(|cx| {
      ListState::new(
        Prompts {
          prompts,
          filtered,
          selected: None,
        },
        window,
        cx,
      )
      .searchable(true)
    });
    let subscription = cx.observe(&list, |_, _, cx| cx.notify());
    Self {
      list,
      editor,
      _subscription: subscription,
    }
  }
}

impl Render for HistoryDialog {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let list = self.list.clone();
    let editor = self.editor.clone();
    let enabled = self.list.read(cx).delegate().selected.is_some();
    v_flex()
      .gap_3()
      .child(div().h(rems(20.)).child(List::new(&self.list).search_placeholder("Search prompts")))
      .child(
        h_flex()
          .justify_end()
          .gap_2()
          .child(
            Button::new("history-close")
              .label("Cancel")
              .on_click(|_, window, cx| window.close_dialog(cx)),
          )
          .child(
            Button::new("history-restore")
              .label("Restore prompt")
              .disabled(!enabled)
              .on_click(move |_, window, cx| {
                let list = list.read(cx).delegate();
                if let Some(text) = list.selected.and_then(|ix| list.prompts.get(ix)).cloned() {
                  editor.update(cx, |editor, cx| editor.replace_all(text, window, cx));
                  window.close_dialog(cx);
                }
              }),
          ),
      )
  }
}

pub(super) fn open(prompts: Vec<String>, editor: Entity<EditorState>, window: &mut Window, cx: &mut App) {
  let dialog = cx.new(|cx| HistoryDialog::new(prompts, editor, window, cx));
  let initial_focus = Cell::new(true);
  window.open_dialog(cx, move |surface, window, cx| {
    if initial_focus.replace(false) {
      dialog.read(cx).list.focus_handle(cx).focus(window, cx);
    }
    surface
      .centered()
      .title("Prompt history")
      .w(rems(42.).to_pixels(window.rem_size()))
      .on_ok(|_, _, _| false)
      .child(dialog.clone())
  });
}

#[cfg(test)]
mod tests {
  use super::Browse;

  #[test]
  fn browsing_returns_to_the_saved_draft() {
    let mut history = Browse::new(vec!["first".into(), "second".into()], "draft".into()).unwrap();
    assert_eq!(history.previous().as_deref(), Some("second"));
    assert_eq!(history.previous().as_deref(), Some("first"));
    assert_eq!(history.previous().as_deref(), Some("first"));
    assert_eq!(history.next().as_deref(), Some("second"));
    assert_eq!(history.next().as_deref(), Some("draft"));
    assert_eq!(history.next(), None);
  }
}
