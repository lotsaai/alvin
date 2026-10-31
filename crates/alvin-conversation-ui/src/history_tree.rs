//! The history tree of a session: every entry omp wrote, rows in a virtual list.
//!
//! A chain of single children stays at one indent, so a long linear session is a flat list; only a branch point indents the
//! entries that start its branches. gpui-kit's `Tree` nests one level per child, which for omp's chains would mean one level
//! per message. The active branch, from the root to the leaf, is highlighted and labels show inline.

use std::collections::BTreeSet;
use std::rc::Rc;

use alvin_conversation::Tree;
use alvin_protocol::{Entry, EntryId, LiveSessionId};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::{
  AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
  SharedString, Size, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, size,
};
use serde_json::Value;

use crate::model::Model;

/// How tall a row is, and how far each level of a branch point indents.
const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 14.0;

/// One entry, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
  pub(crate) id: EntryId,
  pub(crate) depth: usize,
  /// Whether the entry is on the path from the root to the leaf.
  pub(crate) active: bool,
  pub(crate) label: Option<String>,
  pub(crate) text: String,
  /// Whether the entry is a message: only those can be forked at.
  pub(crate) message: bool,
}

/// What an entry says, in a line: the message of a `message`, else its kind.
fn summary(entry: &Entry) -> String {
  let kind = entry.kind.as_str();
  if kind != "message" {
    return kind.replace('_', " ");
  }
  let Some(message) = entry.data.get("message") else {
    return kind.to_owned();
  };
  let role = message.get("role").and_then(Value::as_str).unwrap_or("message");
  let text = match message.get("content") {
    Some(Value::String(text)) => Some(text.as_str()),
    Some(Value::Array(blocks)) => blocks.iter().find_map(|block| {
      block
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| block.get("name").and_then(Value::as_str))
    }),
    _ => None,
  };
  let line = text.unwrap_or_default().lines().next().unwrap_or_default();
  let shown: String = line.chars().take(80).collect();
  format!("{role}: {shown}")
}

/// The rows of a tree in order: a parent before its children, siblings oldest first. A node with more than one child is a
/// branch point and indents them; a node with one child keeps it at the same depth.
pub(crate) fn rows(tree: &Tree) -> Vec<Row> {
  let active: BTreeSet<EntryId> = tree.leaf().map(|leaf| tree.path_to(leaf)).unwrap_or_default().into_iter().collect();
  let mut rows = Vec::with_capacity(tree.len());
  let mut pending: Vec<(EntryId, usize)> = tree.roots().iter().rev().map(|root| (root.clone(), 0)).collect();
  while let Some((id, depth)) = pending.pop() {
    let Some(node) = tree.node(&id) else { continue };
    rows.push(Row {
      active: active.contains(&id),
      label: tree.label(&id).map(str::to_owned),
      text: summary(&node.entry),
      message: node.entry.kind.as_str() == "message",
      depth,
      id,
    });
    let below = if node.children.len() > 1 { depth + 1 } else { depth };
    pending.extend(node.children.iter().rev().map(|child| (child.clone(), below)));
  }
  rows
}

/// What the rows were last built from: the newest entry, the leaf and how many entries there are. Labels arrive as entries,
/// so a change to any of them changes the newest.
type Fingerprint = (Option<EntryId>, Option<EntryId>, usize);

/// The panel: a virtual list of the session's rows.
pub(crate) struct HistoryTreeView {
  live: LiveSessionId,
  model: Entity<Model>,
  rows: Vec<Row>,
  sizes: Rc<Vec<Size<Pixels>>>,
  built_from: Option<Fingerprint>,
  scroll: VirtualListScrollHandle,
  /// The row the actions apply to.
  selected: Option<EntryId>,
  /// What the user types for a label, and for the instructions of a branch summary.
  label: Entity<TextareaState>,
  instructions: Entity<TextareaState>,
  _subscription: Subscription,
}

impl HistoryTreeView {
  pub(crate) fn new(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let label = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 2).placeholder("Label"));
    let instructions = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 3).placeholder("Summary instructions"));
    let subscription = cx.observe(&model, |this, _, cx| {
      this.sync(cx);
      cx.notify();
    });
    let mut view = Self {
      live,
      model,
      rows: Vec::new(),
      sizes: Rc::new(Vec::new()),
      built_from: None,
      scroll: VirtualListScrollHandle::new(),
      selected: None,
      label,
      instructions,
      _subscription: subscription,
    };
    view.sync(cx);
    view
  }

  /// Rebuilds the rows when the tree changed.
  fn sync(&mut self, cx: &mut Context<Self>) {
    let Some(tree) = self
      .model
      .read(cx)
      .conversation()
      .live(self.live)
      .and_then(|view| view.tree.as_ref())
    else {
      return;
    };
    let fingerprint = (tree.cursor().cloned(), tree.leaf().cloned(), tree.len());
    if self.built_from.as_ref() == Some(&fingerprint) {
      return;
    }
    self.rows = rows(tree);
    self.sizes = Rc::new(self.rows.iter().map(|_| size(px(1.), px(ROW_HEIGHT))).collect());
    self.built_from = Some(fingerprint);
  }

  /// Selects a row as a click on it does. Rows of a virtual list are not reachable through the test window's element
  /// registry, so tests select through here; the buttons below the list are clicked as usual.
  #[cfg(test)]
  pub(crate) fn select(&mut self, id: EntryId, cx: &mut Context<Self>) {
    self.selected = Some(id);
    cx.notify();
  }

  /// Types into the label field, as the user would.
  #[cfg(test)]
  pub(crate) fn type_label(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
    self.label.update(cx, |label, cx| label.set_value(text.to_owned(), window, cx));
  }

  /// The rows, for tests.
  #[cfg(test)]
  pub(crate) fn rows(&self) -> &[Row] {
    &self.rows
  }
}

impl Render for HistoryTreeView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let panel = v_flex()
      .id("history-tree")
      .w(px(320.))
      .h_full()
      .border_l_1()
      .border_color(cx.theme().border)
      .child(div().px_3().py_2().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("History"));
    if self.rows.is_empty() {
      return panel.child(div().px_3().text_color(cx.theme().muted_foreground).child("No entries yet"));
    }
    panel
      .child(
        v_virtual_list(cx.entity(), "history-tree-rows", self.sizes.clone(), |this, range, _, cx| {
          let selected = this.selected.clone();
          range
            .filter_map(|index| {
              this
                .rows
                .get(index)
                .map(|row| row_element(row, selected.as_ref() == Some(&row.id), cx))
            })
            .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .flex_1(),
      )
      .children(self.actions(cx))
  }
}

impl HistoryTreeView {
  /// The actions for the selected row: navigate, branch, fork and label.
  fn actions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
    let entry = self.selected.clone()?;
    let row = self.rows.iter().find(|row| row.id == entry)?;
    let live = self.live;
    let model = self.model.clone();
    let (can_branch, summarizing) = {
      let model = model.read(cx);
      (model.can_branch_from(live, &entry), model.summarizing(live))
    };
    let act = |name: &'static str, label: &'static str| Button::new(ElementId::from(name)).small().ghost().label(label);
    let navigate = |summarize: bool| {
      let (model, entry, instructions) = (model.clone(), entry.clone(), self.instructions.clone());
      move |_: &_, _: &mut Window, cx: &mut gpui_kit::App| {
        let instructions = instructions.read(cx).value().to_string();
        model.update(cx, |model, cx| {
          model.navigate_tree(live, entry.clone(), summarize, instructions, cx)
        });
      }
    };
    let set_label = |clear: bool| {
      let (model, entry, label) = (model.clone(), entry.clone(), self.label.clone());
      move |_: &_, _: &mut Window, cx: &mut gpui_kit::App| {
        let text = label.read(cx).value().trim().to_owned();
        let value = (!clear && !text.is_empty()).then_some(text);
        model.update(cx, |model, cx| model.label(live, entry.clone(), value, cx));
      }
    };
    let (branch_model, branch_entry) = (model.clone(), entry.clone());
    let (fork_model, fork_entry) = (model.clone(), entry.clone());
    let cancel_model = model.clone();
    Some(
      v_flex()
        .id("history-actions")
        .p_2()
        .gap_1()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(div().text_sm().truncate().child(SharedString::from(row.text.clone())))
        .child(
          h_flex()
            .gap_1()
            .flex_wrap()
            .child(act("history-navigate", "Go here").on_click(navigate(false)))
            .child(act("history-navigate-summary", "Go here, summarize").on_click(navigate(true)))
            .child(act("history-branch", "Branch").disabled(!can_branch).on_click(move |_, _, cx| {
              branch_model.update(cx, |model, cx| model.branch(live, branch_entry.clone(), cx));
            }))
            .child(act("history-fork", "Fork").disabled(!row.message).on_click(move |_, _, cx| {
              fork_model.update(cx, |model, cx| model.fork(live, Some(fork_entry.clone()), cx));
            }))
            .children(summarizing.then(|| {
              act("history-cancel-summary", "Cancel summary").on_click(move |_, _, cx| {
                cancel_model.update(cx, |model, cx| model.abort_branch_summary(live, cx));
              })
            })),
        )
        .child(
          Textarea::new(&self.instructions)
            .accessibility_id("history-instructions")
            .aria_label("Summary instructions"),
        )
        .child(Textarea::new(&self.label).accessibility_id("history-label").aria_label("Label"))
        .child(
          h_flex()
            .gap_1()
            .child(act("history-label-set", "Set label").on_click(set_label(false)))
            .child(act("history-label-clear", "Clear label").on_click(set_label(true))),
        )
        .into_any_element(),
    )
  }
}

fn row_element(row: &Row, selected: bool, cx: &mut Context<HistoryTreeView>) -> AnyElement {
  let id = row.id.clone();
  let theme = cx.theme();
  let mut line = h_flex()
    .id(ElementId::from(SharedString::from(format!("tree-row-{}", row.id.as_str()))))
    .h(px(ROW_HEIGHT))
    .pl(px(12. + INDENT * row.depth as f32))
    .pr_2()
    .gap_1()
    .text_sm()
    .child(div().w(px(10.)).child(if row.active { "●" } else { "○" }))
    .child(div().flex_1().min_w_0().truncate().child(SharedString::from(row.text.clone())));
  if row.active {
    line = line.bg(theme.accent).text_color(theme.accent_foreground);
  } else {
    line = line.text_color(theme.muted_foreground);
  }
  if selected {
    line = line.border_1().border_color(theme.ring);
  }
  if let Some(label) = &row.label {
    line = line.child(
      div()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(theme.border)
        .child(SharedString::from(label.clone())),
    );
  }
  line
    .on_click(cx.listener(move |this, _, _, cx| {
      this.selected = Some(id.clone());
      cx.notify();
    }))
    .into_any_element()
}

#[cfg(test)]
mod tests {
  use alvin_conversation::Tree;
  use alvin_protocol::HistoryEntries;

  use super::{Row, rows};

  fn entry(id: &str, parent: Option<&str>, second: u32, kind: &str) -> alvin_protocol::Entry {
    serde_json::from_value(serde_json::json!({
      "id": id,
      "parentId": parent,
      "timestamp": format!("2027-01-01T00:00:{second:02}Z"),
      "type": kind,
    }))
    .unwrap()
  }

  fn tree_of(entries: Vec<alvin_protocol::Entry>, leaf: &str) -> Tree {
    Tree::from_entries(HistoryEntries {
      entries,
      leaf: Some(alvin_protocol::EntryId::from(leaf.to_owned())),
    })
  }

  fn shape(rows: &[Row]) -> Vec<(&str, usize, bool)> {
    rows.iter().map(|row| (row.id.as_str(), row.depth, row.active)).collect()
  }

  #[test]
  fn a_chain_of_single_children_is_one_flat_list() {
    let chain = (0..50)
      .map(|n| {
        entry(
          &format!("e{n:02}"),
          (n > 0).then(|| format!("e{:02}", n - 1)).as_deref(),
          n,
          "message",
        )
      })
      .collect();
    let rows = rows(&tree_of(chain, "e49"));

    assert_eq!(rows.len(), 50);
    assert!(rows.iter().all(|row| row.depth == 0 && row.active));
  }

  #[test]
  fn a_branch_point_indents_its_branches_and_the_active_path_is_marked() {
    //        a ── b ── c        (branch point b: c and d)
    //              └── d ── e   (leaf e)
    let entries = vec![
      entry("a", None, 0, "message"),
      entry("b", Some("a"), 1, "message"),
      entry("c", Some("b"), 2, "message"),
      entry("d", Some("b"), 3, "message"),
      entry("e", Some("d"), 4, "message"),
    ];
    let rows = rows(&tree_of(entries, "e"));

    assert_eq!(
      shape(&rows),
      [("a", 0, true), ("b", 0, true), ("c", 1, false), ("d", 1, true), ("e", 1, true)]
    );
  }

  #[test]
  fn non_message_entries_are_listed_by_their_kind() {
    let rows = rows(&tree_of(vec![entry("a", None, 0, "model_change")], "a"));

    assert_eq!(rows[0].text, "model change");
  }
}
