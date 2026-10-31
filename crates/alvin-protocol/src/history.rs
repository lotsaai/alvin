//! A session's history: every entry omp wrote, in the order it appended them.
//!
//! omp's wire schema leaves entries open, so the fields every entry shares are typed and the rest stay as omp wrote them.
//! `parent_id` carries the branch structure, so the entries are all a reader needs to build the tree. omp's own `get_tree`
//! nests a node's children inside it, one level per entry on a branch; omp-rpc cannot decode it past a few dozen entries
//! (serde_json's recursion limit), so the protocol never carries it.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ids::EntryId;

/// What kind of record an entry is. omp adds kinds across releases, so this is not an enum.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryKind(String);

impl EntryKind {
  /// The kind as omp wrote it, such as `message` or `label`.
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl From<String> for EntryKind {
  fn from(kind: String) -> Self {
    Self(kind)
  }
}

/// One omp session entry: the shared fields typed, the kind-specific fields kept as omp wrote them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
  /// The entry's id.
  pub id: EntryId,
  /// The entry this one follows. Absent for a root.
  #[serde(rename = "parentId")]
  pub parent_id: Option<EntryId>,
  /// When omp wrote it.
  pub timestamp: Timestamp,
  /// What kind of record it is.
  #[serde(rename = "type")]
  pub kind: EntryKind,
  /// Every other field.
  #[serde(flatten)]
  pub data: Map<String, Value>,
}

/// `live/entries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntriesRequest {
  /// The live session.
  pub live: crate::ids::LiveSessionId,
  /// Only entries strictly after this one. Absent: every entry. omp answers an id it does not know with the command error
  /// code `unknown_since`.
  pub since: Option<EntryId>,
}

/// Entries in the order omp appended them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntries {
  /// The entries, oldest first.
  pub entries: Vec<Entry>,
  /// The entry the active branch ends at after them.
  pub leaf: Option<EntryId>,
}
