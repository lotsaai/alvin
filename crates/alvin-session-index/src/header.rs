//! Typed parsing of the head of an omp session file: the optional title slot, the session header, and the
//! records that fit in the window after it.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use alvin_protocol::SessionId;
use jiff::Timestamp;
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
  /// The display title, as omp computes it: the title slot's, else the header's, else the latest compaction
  /// summary, else the first user message, each reduced to its first line. `None` when there is none.
  pub title: Option<String>,
  /// When the session started, from the header.
  pub created: Timestamp,
  /// When the file was last modified.
  pub modified: Timestamp,
  /// The session file this session branched from.
  pub parent: Option<PathBuf>,
  /// Whether the head window shows an assistant message, which makes the session not empty.
  pub assistant_in_head: bool,
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
#[serde(rename_all = "camelCase")]
struct Header {
  id: String,
  cwd: PathBuf,
  title: Option<String>,
  timestamp: Timestamp,
  parent_session: Option<PathBuf>,
}

/// A record after the header; only the two that can carry a title matter.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Entry {
  Message {
    message: Message,
  },
  Compaction {
    #[serde(rename = "shortSummary")]
    short_summary: Option<String>,
  },
  #[serde(other)]
  Other,
}

#[derive(Deserialize)]
struct Message {
  role: String,
  /// Absent on messages that are not chat turns, such as `bashExecution`.
  #[serde(default)]
  content: serde_json::Value,
}

/// The text of a message's content: a bare string, or the text blocks joined by a space.
fn text_of(content: &serde_json::Value) -> String {
  match content {
    serde_json::Value::String(text) => text.clone(),
    serde_json::Value::Array(blocks) => blocks
      .iter()
      .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
      .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
      .collect::<Vec<_>>()
      .join(" "),
    _ => String::new(),
  }
}

/// Whether raw bytes carry an assistant role marker. omp counts these in the windows it reads, so a message
/// whose record is cut off by a window still counts.
pub(crate) fn has_assistant_marker(bytes: &[u8]) -> bool {
  const MARKER: &[u8] = br#""role":"assistant""#;
  bytes.windows(MARKER.len()).any(|window| window == MARKER)
}

/// The first line of `text` without control characters, trimmed; `None` when nothing is left.
fn sanitize(text: &str) -> Option<String> {
  let line = text.lines().next().unwrap_or_default();
  let cleaned: String = line.chars().filter(|character| !character.is_control()).collect();
  let trimmed = cleaned.trim();
  (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The text of a user message cut off by the end of the window, read straight from its raw bytes.
///
/// omp does the same for the fragment of its 4 KiB prefix: a long first prompt starts inside the window and
/// ends outside it, and it is still the session's name.
fn torn_user_text(fragment: &[u8]) -> Option<String> {
  let fragment = String::from_utf8_lossy(fragment);
  let message = fragment.find(r#""role":"user""#)?;
  let after_key = &fragment[message..];
  let text = after_key
    .find(r#""text":""#)
    .map(|at| (at, r#""text":""#.len()))
    .or_else(|| after_key.find(r#""content":""#).map(|at| (at, r#""content":""#.len())))?;
  let raw = &after_key[text.0 + text.1..];
  let mut escaped = false;
  let end = raw
    .char_indices()
    .find(|&(_, character)| match (escaped, character) {
      (true, _) => {
        escaped = false;
        false
      }
      (false, '\\') => {
        escaped = true;
        false
      }
      (false, '"') => true,
      _ => false,
    })
    .map_or(raw.len(), |(at, _)| at);
  let raw = raw[..end].strip_suffix('\\').unwrap_or(&raw[..end]);
  // A cut-off escape sequence cannot decode; the raw text is still a fine name.
  Some(serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.to_owned()))
}

/// Finds the first user message when it lies beyond the head window.
pub(crate) fn first_user_title(path: &Path, mut reader: impl BufRead) -> Result<Option<String>> {
  let mut line = Vec::new();
  let mut offset = 0;
  loop {
    line.clear();
    let count = reader.read_until(b'\n', &mut line).map_err(|source| Error::ReadFile {
      path: path.to_path_buf(),
      source,
    })?;
    if count == 0 {
      return Ok(None);
    }
    let Some(record) = line.strip_suffix(b"\n") else {
      return Ok(torn_user_text(&line).as_deref().and_then(sanitize));
    };
    let entry: Entry = serde_json::from_slice(record).map_err(|source| Error::DecodeRecord {
      path: path.to_path_buf(),
      offset,
      source,
    })?;
    if let Entry::Message { message } = entry
      && message.role == "user"
    {
      return Ok(sanitize(&text_of(&message.content)));
    }
    offset += count;
  }
}

/// Parses the head bytes of the session file at `path`, last modified at `modified`. Complete lines are decoded;
/// the line cut off by the end of the window is a torn record, which can only still name the session.
pub(crate) fn parse(path: &Path, head: &[u8], modified: Timestamp) -> Result<SessionHead> {
  let decode = |source, offset| Error::DecodeRecord {
    path: path.to_path_buf(),
    offset,
    source,
  };
  let incomplete = || Error::HeaderIncomplete { path: path.to_path_buf() };

  let mut offset = 0;
  let mut slot: Option<TitleSlot> = None;
  let mut chunks = head.split_inclusive(|byte| *byte == b'\n');
  let header = loop {
    let chunk = chunks.next().ok_or_else(incomplete)?;
    let line = chunk.strip_suffix(b"\n").ok_or_else(incomplete)?;
    let record = serde_json::from_slice(line).map_err(|source| decode(source, offset))?;
    offset += chunk.len();
    match record {
      Record::Title(title) if offset == chunk.len() => slot = Some(title),
      Record::Title(_) => {
        return Err(Error::MisplacedTitleSlot {
          path: path.to_path_buf(),
          offset: offset - chunk.len(),
        });
      }
      Record::Session(header) => break header,
    }
  };

  let mut summary = None;
  let mut first_message = None;
  let mut assistant_in_head = false;
  for chunk in chunks {
    let Some(line) = chunk.strip_suffix(b"\n") else {
      first_message = first_message.or_else(|| torn_user_text(chunk));
      assistant_in_head |= has_assistant_marker(chunk);
      break;
    };
    match serde_json::from_slice(line).map_err(|source| decode(source, offset))? {
      Entry::Compaction { short_summary: Some(text) } => summary = Some(text),
      Entry::Message { message } if message.role == "assistant" => assistant_in_head = true,
      Entry::Message { message } if message.role == "user" && first_message.is_none() => {
        first_message = Some(text_of(&message.content));
      }
      Entry::Compaction { short_summary: None } | Entry::Message { .. } | Entry::Other => {}
    }
    offset += chunk.len();
  }

  // The slot overrides the header's title, and an empty slot means no title.
  let named = match slot {
    Some(slot) => Some(slot.title),
    None => header.title,
  };
  let title = named
    .as_deref()
    .and_then(sanitize)
    .or_else(|| summary.as_deref().and_then(sanitize))
    .or_else(|| first_message.as_deref().and_then(sanitize));
  Ok(SessionHead {
    path: path.to_path_buf(),
    id: SessionId::from(header.id),
    cwd: header.cwd,
    title,
    created: header.timestamp,
    modified,
    parent: header.parent_session,
    assistant_in_head,
  })
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use super::{first_user_title, parse};
  use crate::Error;
  use crate::test_support::{TempDir, run_omp};

  fn modified() -> jiff::Timestamp {
    jiff::Timestamp::UNIX_EPOCH
  }

  fn user_message(text: &str) -> String {
    format!(r#"{{"type":"message","id":"u1","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#)
  }

  #[test]
  fn finds_first_user_title_after_large_metadata_records() {
    let records = format!(
      "{{\"type\":\"custom\",\"data\":\"{}\"}}\n{{\"type\":\"custom_message\",\"content\":\"{}\"}}\n{}\n{}\n",
      "x".repeat(5000),
      "y".repeat(5000),
      user_message("hi are you there?"),
      user_message("later")
    );

    let title = first_user_title(Path::new("s.jsonl"), records.as_bytes()).unwrap();

    assert_eq!(title.as_deref(), Some("hi are you there?"));
  }

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

    let head = parse(Path::new("s.jsonl"), format!("{slot}\n{header}\n").as_bytes(), modified()).unwrap();

    assert_eq!(head.title.as_deref(), Some("from slot"));
  }

  #[test]
  fn an_empty_title_slot_overrides_the_header_title_with_none() {
    let (slot, header) = real_head();
    let header = header.replacen(r#""cwd""#, r#""title":"from header","cwd""#, 1);

    let head = parse(Path::new("s.jsonl"), format!("{slot}\n{header}\n").as_bytes(), modified()).unwrap();

    assert_eq!(head.title, None);
  }

  #[test]
  fn without_a_title_slot_the_header_title_is_used() {
    let (_slot, header) = real_head();
    let header = header.replacen(r#""cwd""#, r#""title":"from header","cwd""#, 1);

    let head = parse(Path::new("s.jsonl"), format!("{header}\n").as_bytes(), modified()).unwrap();

    assert_eq!(head.title.as_deref(), Some("from header"));
  }

  #[test]
  fn a_message_without_content_does_not_fail_the_head() {
    let (_slot, header) = real_head();
    let bash = r#"{"type":"message","id":"b1","message":{"role":"bashExecution","command":"ls","output":"","exitCode":0}}"#;

    let head = parse(
      Path::new("s.jsonl"),
      format!("{header}\n{bash}\n{}\n", user_message("after bash")).as_bytes(),
      modified(),
    )
    .unwrap();

    assert_eq!(head.title.as_deref(), Some("after bash"));
  }

  #[test]
  fn a_head_cut_before_the_header_ends_is_incomplete() {
    let (slot, header) = real_head();
    let cut = &header[..header.len() / 2];

    let error = parse(Path::new("s.jsonl"), format!("{slot}\n{cut}").as_bytes(), modified()).unwrap_err();

    assert!(matches!(error, Error::HeaderIncomplete { .. }), "{error:?}");
  }

  #[test]
  fn the_first_user_message_names_a_session_without_a_title() {
    let (slot, header) = real_head();
    let head = format!(
      "{slot}\n{header}\n{}\n{}\n",
      user_message("first line\\nsecond"),
      user_message("later")
    );

    let parsed = parse(Path::new("s.jsonl"), head.as_bytes(), modified()).unwrap();

    assert_eq!(parsed.title.as_deref(), Some("first line"));
  }

  #[test]
  fn a_compaction_summary_beats_the_first_message_and_a_title_beats_both() {
    let (slot, header) = real_head();
    let compaction = r#"{"type":"compaction","id":"c1","shortSummary":"summary"}"#;
    let body = format!("{header}\n{}\n{compaction}\n", user_message("hello"));

    let untitled = parse(Path::new("s.jsonl"), format!("{slot}\n{body}").as_bytes(), modified()).unwrap();
    let titled_slot = slot.replacen(r#""title":"""#, r#""title":"slot title""#, 1);
    let titled = parse(Path::new("s.jsonl"), format!("{titled_slot}\n{body}").as_bytes(), modified()).unwrap();

    assert_eq!(untitled.title.as_deref(), Some("summary"));
    assert_eq!(titled.title.as_deref(), Some("slot title"));
  }

  #[test]
  fn a_first_message_cut_off_by_the_window_still_names_the_session() {
    let (slot, header) = real_head();
    let text = "a long pasted prompt that does not fit";
    let full = user_message(text);
    let cut = &full[..full.find(text).unwrap() + 6];
    let head = format!("{slot}\n{header}\n{cut}");

    let parsed = parse(Path::new("s.jsonl"), head.as_bytes(), modified()).unwrap();

    assert_eq!(parsed.title.as_deref(), Some("a long"));
  }

  #[test]
  fn the_header_carries_creation_time_and_parent() {
    let (slot, header) = real_head();
    let header = header.replacen(r#""cwd""#, r#""parentSession":"/sessions/parent.jsonl","cwd""#, 1);

    let parsed = parse(Path::new("s.jsonl"), format!("{slot}\n{header}\n").as_bytes(), modified()).unwrap();

    assert_eq!(parsed.parent.as_deref(), Some(Path::new("/sessions/parent.jsonl")));
    assert!(parsed.created > jiff::Timestamp::UNIX_EPOCH);
  }
}
