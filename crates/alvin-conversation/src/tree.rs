//! A session's history tree, built from the entries omp appended.
//!
//! The rules are omp's own (`SessionManager.tree` and its label index): an entry goes under its parent, or is a root when it
//! has none or the parent is not in the session; siblings are ordered by timestamp, oldest first, appended order breaking
//! ties; a `label` entry sets or clears the label of its target. Applying the entries appended since the tree's newest gives
//! the tree a fresh read of every entry would.

use std::collections::BTreeMap;

use alvin_protocol::{Entry, EntryId, HistoryEntries};

/// One entry with the entries that follow it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
  /// The entry.
  pub entry: Entry,
  /// The entries that follow this one, oldest first.
  pub children: Vec<EntryId>,
}

/// The branches of a session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tree {
  nodes: BTreeMap<EntryId, Node>,
  roots: Vec<EntryId>,
  labels: BTreeMap<EntryId, String>,
  newest: Option<EntryId>,
  leaf: Option<EntryId>,
}

impl Tree {
  /// The tree of a session's entries, oldest first.
  pub fn from_entries(entries: HistoryEntries) -> Self {
    let mut tree = Self::default();
    tree.apply(entries);
    tree
  }

  /// Adds entries appended after the newest one, and moves the leaf.
  pub fn apply(&mut self, entries: HistoryEntries) {
    for entry in entries.entries {
      self.insert(entry);
    }
    self.leaf = entries.leaf;
  }

  fn insert(&mut self, entry: Entry) {
    let id = entry.id.clone();
    if entry.kind.as_str() == "label"
      && let Some(target) = entry.data.get("targetId").and_then(|target| target.as_str())
    {
      let target = EntryId::from(target.to_owned());
      match entry.data.get("label").and_then(|label| label.as_str()) {
        Some(label) if !label.is_empty() => {
          self.labels.insert(target, label.to_owned());
        }
        _ => {
          self.labels.remove(&target);
        }
      }
    }
    let parent = entry
      .parent_id
      .clone()
      .filter(|parent| *parent != id && self.nodes.contains_key(parent));
    let timestamp = entry.timestamp;
    let nodes = &self.nodes;
    let siblings = match &parent {
      Some(parent) => match self.nodes.get(parent) {
        Some(node) => &node.children,
        None => &self.roots,
      },
      None => &self.roots,
    };
    // After every sibling that is not newer: a stable sort by timestamp.
    let at = siblings.partition_point(|sibling| nodes.get(sibling).is_some_and(|node| node.entry.timestamp <= timestamp));
    match parent {
      Some(parent) => {
        if let Some(node) = self.nodes.get_mut(&parent) {
          node.children.insert(at, id.clone());
        }
      }
      None => self.roots.insert(at, id.clone()),
    }
    self.nodes.insert(
      id.clone(),
      Node {
        entry,
        children: Vec::new(),
      },
    );
    self.newest = Some(id);
  }

  /// The entries without a parent in the session, oldest first.
  pub fn roots(&self) -> &[EntryId] {
    &self.roots
  }

  /// An entry and its children.
  pub fn node(&self, id: &EntryId) -> Option<&Node> {
    self.nodes.get(id)
  }

  /// The label the user gave an entry.
  pub fn label(&self, id: &EntryId) -> Option<&str> {
    self.labels.get(id).map(String::as_str)
  }

  /// The entry the active branch ends at.
  pub fn leaf(&self) -> Option<&EntryId> {
    self.leaf.as_ref()
  }

  /// The newest entry: the `since` of the next read of entries.
  pub fn cursor(&self) -> Option<&EntryId> {
    self.newest.as_ref()
  }

  /// How many entries the session has.
  pub fn len(&self) -> usize {
    self.nodes.len()
  }

  /// Whether the session has no entries.
  pub fn is_empty(&self) -> bool {
    self.nodes.is_empty()
  }

  /// The entries from the root to `id`, root first; empty when `id` is not in the tree.
  pub fn path_to(&self, id: &EntryId) -> Vec<EntryId> {
    let mut path = Vec::new();
    let mut at = self.nodes.get(id);
    while let Some(node) = at {
      path.push(node.entry.id.clone());
      at = node
        .entry
        .parent_id
        .as_ref()
        .filter(|parent| **parent != node.entry.id)
        .and_then(|parent| self.nodes.get(parent));
    }
    path.reverse();
    path
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::HistoryEntries;

  use super::*;

  /// Entries of two real turns: what omp had after the first, after the second, and the entries between.
  struct Recorded {
    before: HistoryEntries,
    after: HistoryEntries,
    delta: HistoryEntries,
  }

  fn recorded() -> Recorded {
    Recorded {
      before: serde_json::from_str(include_str!("../fixtures/tree_turn.entries_before.json")).unwrap(),
      after: serde_json::from_str(include_str!("../fixtures/tree_turn.entries_after.json")).unwrap(),
      delta: serde_json::from_str(include_str!("../fixtures/tree_turn.entries_delta.json")).unwrap(),
    }
  }

  #[test]
  fn entries_since_the_cursor_give_the_tree_a_fresh_read_gives() {
    let recorded = recorded();
    let fresh = Tree::from_entries(recorded.after.clone());

    let mut grown = Tree::from_entries(recorded.before.clone());
    assert_eq!(grown.cursor(), recorded.before.entries.last().map(|entry| &entry.id));
    grown.apply(recorded.delta);

    assert_eq!(grown, fresh);
    assert_eq!(grown.cursor(), recorded.after.entries.last().map(|entry| &entry.id));
    assert_eq!(grown.leaf(), recorded.after.leaf.as_ref());
    assert_eq!(grown.len(), recorded.after.entries.len());
    assert!(grown.len() > recorded.before.entries.len());
  }

  #[test]
  fn the_path_to_the_leaf_runs_from_a_root_through_every_ancestor() {
    let recorded = recorded();
    let tree = Tree::from_entries(recorded.after);
    let leaf = tree.leaf().expect("the session has a leaf").clone();

    let path = tree.path_to(&leaf);

    assert_eq!(path.last(), Some(&leaf));
    assert!(tree.roots().contains(path.first().expect("a path")));
    for pair in path.windows(2) {
      assert_eq!(tree.node(&pair[1]).and_then(|node| node.entry.parent_id.as_ref()), Some(&pair[0]));
    }
  }

  /// An entry as omp writes it, with the fields every entry shares.
  fn entry(id: &str, parent: Option<&str>, timestamp: &str, kind: &str, data: serde_json::Value) -> Entry {
    let mut object = serde_json::json!({ "id": id, "parentId": parent, "timestamp": timestamp, "type": kind });
    if let (Some(object), serde_json::Value::Object(data)) = (object.as_object_mut(), data) {
      object.extend(data);
    }
    serde_json::from_value(object).unwrap()
  }

  fn label(id: &str, parent: &str, at: &str, target: &str, label: &str) -> Entry {
    entry(
      id,
      Some(parent),
      at,
      "label",
      serde_json::json!({ "targetId": target, "label": label }),
    )
  }

  #[test]
  fn a_label_entry_sets_the_label_of_its_target_and_an_empty_one_clears_it() {
    let recorded = recorded();
    let mut tree = Tree::from_entries(recorded.before.clone());
    let last = recorded.before.entries.last().expect("entries").id.clone();
    let target = recorded.before.entries.first().expect("entries").id.clone();

    let set = label("label-1", last.as_str(), "2027-01-01T00:00:01Z", target.as_str(), "start");
    tree.apply(HistoryEntries {
      leaf: Some(set.id.clone()),
      entries: vec![set.clone()],
    });
    assert_eq!(tree.label(&target), Some("start"));
    assert_eq!(tree.leaf(), Some(&set.id));

    let clear = label("label-2", set.id.as_str(), "2027-01-01T00:00:02Z", target.as_str(), "");
    tree.apply(HistoryEntries {
      leaf: Some(clear.id.clone()),
      entries: vec![clear],
    });
    assert_eq!(tree.label(&target), None);
  }

  #[test]
  fn siblings_are_ordered_by_timestamp_with_appended_order_breaking_ties() {
    let none = serde_json::json!({});
    let parent = entry("parent", None, "2027-01-01T00:00:00Z", "custom", none.clone());
    let older = entry("older", Some("parent"), "2027-01-01T00:00:10Z", "custom", none.clone());
    let newer = entry("newer", Some("parent"), "2027-01-01T00:00:20Z", "custom", none.clone());
    let tied = entry("tied", Some("parent"), "2027-01-01T00:00:10Z", "custom", none);

    // Appended newest first, as a clock set back would do.
    let tree = Tree::from_entries(HistoryEntries {
      entries: vec![parent.clone(), newer, older, tied],
      leaf: None,
    });

    let order: Vec<&str> = tree
      .node(&parent.id)
      .expect("the parent")
      .children
      .iter()
      .map(EntryId::as_str)
      .collect();
    assert_eq!(order, ["older", "tied", "newer"]);
  }

  #[test]
  fn an_entry_whose_parent_is_not_in_the_session_is_a_root() {
    let orphan = entry("orphan", Some("missing"), "2027-01-01T00:00:00Z", "custom", serde_json::json!({}));

    let tree = Tree::from_entries(HistoryEntries {
      entries: vec![orphan.clone()],
      leaf: None,
    });

    assert_eq!(tree.roots(), [orphan.id]);
  }

  /// One action on the history of the recorded session and what omp had after it.
  struct Step {
    name: String,
    delta: HistoryEntries,
    after: HistoryEntries,
  }

  fn steps() -> Vec<Step> {
    let steps: Vec<serde_json::Value> = serde_json::from_str(include_str!("../fixtures/tree_turn.steps.json")).unwrap();
    steps
      .into_iter()
      .map(|step| Step {
        name: step["name"].as_str().unwrap().to_owned(),
        delta: serde_json::from_value(step["delta"].clone()).unwrap(),
        after: serde_json::from_value(step["after"].clone()).unwrap(),
      })
      .collect()
  }

  #[test]
  fn labelling_clearing_and_navigating_give_what_a_fresh_read_gives() {
    let mut grown = Tree::from_entries(recorded().after);
    let steps = steps();
    assert_eq!(
      steps.iter().map(|step| step.name.as_str()).collect::<Vec<_>>(),
      ["label", "clear_label", "navigate"]
    );

    for step in steps {
      grown.apply(step.delta);
      assert_eq!(grown, Tree::from_entries(step.after), "after {}", step.name);
    }
  }

  #[test]
  fn a_real_label_shows_on_its_target_until_it_is_cleared_and_navigating_moves_only_the_leaf() {
    let mut grown = Tree::from_entries(recorded().after);
    let mut steps = steps().into_iter();
    let label = steps.next().expect("the label step");
    let target = label
      .delta
      .entries
      .iter()
      .find(|entry| entry.kind.as_str() == "label")
      .and_then(|entry| entry.data.get("targetId")?.as_str().map(|id| EntryId::from(id.to_owned())))
      .expect("omp wrote a label entry");

    grown.apply(label.delta);
    assert_eq!(grown.label(&target), Some("start"));

    let clear = steps.next().expect("the clear step");
    grown.apply(clear.delta);
    assert_eq!(grown.label(&target), None);

    let navigate = steps.next().expect("the navigate step");
    let len = grown.len();
    let leaf = grown.leaf().cloned();
    grown.apply(navigate.delta);
    assert_eq!(grown.len(), len, "navigating appends no entry");
    assert_ne!(grown.leaf().cloned(), leaf);
  }
}
