//! Reading omp's history tree and entries into the protocol's types.
//!
//! omp-rpc hands over entries as open JSON objects. The fields every entry shares are parsed here, once; an entry that lacks
//! them is a typed error naming its id.

use alvin_protocol::{Entry, HistoryEntries};
use omp_rpc::SessionEntries;
use serde_json::{Map, Value};

use crate::{Error, Result};

/// One entry. `what` names it in the error: its id when it has one.
fn entry(object: Map<String, Value>) -> Result<Entry> {
  let id = object.get("id").and_then(Value::as_str).unwrap_or("(no id)").to_owned();
  serde_json::from_value(Value::Object(object)).map_err(|source| Error::Entry { entry: id, source })
}

/// The entries of `get_entries`, in append order.
pub(crate) fn entries(raw: SessionEntries) -> Result<HistoryEntries> {
  Ok(HistoryEntries {
    entries: raw.entries.into_iter().map(entry).collect::<Result<_>>()?,
    leaf: raw.leaf_id.map(Into::into),
  })
}
