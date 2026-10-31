//! Typed parsing of the head of an omp session file: the optional title slot, then the session header.

use std::path::{Path, PathBuf};

use alvin_protocol::SessionId;
use serde::Deserialize;

use crate::{Error, Result};

/// What the head of one session file says about the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHead {
  /// The session file.
  pub path: PathBuf,
  /// The id omp wrote in the header.
  pub id: SessionId,
  /// The working directory the session ran in.
  pub cwd: PathBuf,
  /// The session title as omp shows it: the title slot's, else the header's. `None` when there is no title.
  pub title: Option<String>,
}

/// A record that can open a session file.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Record {
  /// The fixed-width slot omp rewrites in place when the title changes.
  Title(TitleSlot),
  Session(Header),
}

#[derive(Deserialize)]
struct TitleSlot {
  title: String,
}

#[derive(Deserialize)]
struct Header {
  id: String,
  cwd: PathBuf,
  title: Option<String>,
}

/// Parses the head bytes of the session file at `path`. Only complete lines count: a line cut off by the end
/// of the window is a torn record.
pub(crate) fn parse(path: &Path, head: &[u8]) -> Result<SessionHead> {
  let mut offset = 0;
  let mut slot: Option<TitleSlot> = None;
  for chunk in head.split_inclusive(|byte| *byte == b'\n') {
    let line = chunk
      .strip_suffix(b"\n")
      .ok_or_else(|| Error::HeaderIncomplete { path: path.to_path_buf() })?;
    let record = serde_json::from_slice(line).map_err(|source| Error::DecodeRecord {
      path: path.to_path_buf(),
      offset,
      source,
    })?;
    match record {
      Record::Title(title) if offset == 0 => slot = Some(title),
      Record::Title(_) => {
        return Err(Error::MisplacedTitleSlot {
          path: path.to_path_buf(),
          offset,
        });
      }
      Record::Session(header) => {
        // A title slot overrides the header's title; an empty slot title means there is none.
        let title = match slot {
          Some(slot) => Some(slot.title).filter(|title| !title.is_empty()),
          None => header.title,
        };
        return Ok(SessionHead {
          path: path.to_path_buf(),
          id: SessionId::from(header.id),
          cwd: header.cwd,
          title,
        });
      }
    }
    offset += chunk.len();
  }
  Err(Error::HeaderIncomplete { path: path.to_path_buf() })
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use super::parse;
  use crate::Error;
  use crate::test_support::{TempDir, run_omp};

  /// The head of a real omp session: the title slot line, then the header line.
  fn real_head() -> (String, String) {
    let tmp = TempDir::new();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let file = run_omp(&project, &tmp.path().join("sessions"), "Reply with the single word ok.");
    let text = std::fs::read_to_string(file).unwrap();
    let mut lines = text.lines();
    (lines.next().unwrap().to_owned(), lines.next().unwrap().to_owned())
  }

  #[test]
  fn a_non_empty_title_slot_is_the_title() {
    let (slot, header) = real_head();
    // omp rewrites the slot's title in place, keeping its fixed width.
    let slot = slot.replacen(r#""title":"""#, r#""title":"from slot""#, 1);

    let head = parse(Path::new("s.jsonl"), format!("{slot}\n{header}\n").as_bytes()).unwrap();

    assert_eq!(head.title.as_deref(), Some("from slot"));
  }

  #[test]
  fn an_empty_title_slot_overrides_the_header_title_with_none() {
    let (slot, header) = real_head();
    let header = header.replacen(r#""cwd""#, r#""title":"from header","cwd""#, 1);

    let head = parse(Path::new("s.jsonl"), format!("{slot}\n{header}\n").as_bytes()).unwrap();

    assert_eq!(head.title, None);
  }

  #[test]
  fn without_a_title_slot_the_header_title_is_used() {
    let (_slot, header) = real_head();
    let header = header.replacen(r#""cwd""#, r#""title":"from header","cwd""#, 1);

    let head = parse(Path::new("s.jsonl"), format!("{header}\n").as_bytes()).unwrap();

    assert_eq!(head.title.as_deref(), Some("from header"));
  }

  #[test]
  fn a_head_cut_before_the_header_ends_is_incomplete() {
    let (slot, header) = real_head();
    let cut = &header[..header.len() / 2];

    let error = parse(Path::new("s.jsonl"), format!("{slot}\n{cut}").as_bytes()).unwrap_err();

    assert!(matches!(error, Error::HeaderIncomplete { .. }), "{error:?}");
  }
}
