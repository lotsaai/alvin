//! How a session's transcript ends, derived from the tail window of its file the way omp derives it.

use std::io::{Read, Seek, SeekFrom};

use alvin_protocol::SessionStatus;
use serde::Deserialize;

/// omp reads the last 32 KiB of a session file for its status; so does the index. A final message larger than
/// the window leaves nothing to classify, which is `Unknown` rather than a guess.
pub(crate) const TAIL_BYTES: u64 = 32 * 1024;

/// Reads the last [`TAIL_BYTES`] of `file`, or all of it when it is shorter.
pub(crate) fn read_tail(file: &mut (impl Read + Seek)) -> std::io::Result<Vec<u8>> {
  let len = file.seek(SeekFrom::End(0))?;
  file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
  let mut tail = Vec::new();
  file.take(TAIL_BYTES).read_to_end(&mut tail)?;
  Ok(tail)
}

#[derive(Deserialize)]
struct Record {
  #[serde(rename = "type")]
  kind: String,
  message: Option<Message>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
  role: String,
  stop_reason: Option<String>,
  content: Option<serde_json::Value>,
}

/// Classifies the last message record of `tail`. Records are newline-terminated when written, so only the
/// first line of the window can be a fragment; it starts mid-record and does not decode, and is skipped.
pub(crate) fn status_of(tail: &[u8]) -> SessionStatus {
  let message = tail
    .rsplit(|byte| *byte == b'\n')
    .filter(|line| line.first() == Some(&b'{'))
    .filter_map(|line| serde_json::from_slice::<Record>(line).ok())
    .find_map(|record| (record.kind == "message").then_some(record.message).flatten());
  let Some(message) = message else {
    return SessionStatus::Unknown;
  };
  match message.role.as_str() {
    "assistant" => match message.stop_reason.as_deref() {
      Some("error") => SessionStatus::Error,
      Some("aborted") => SessionStatus::Aborted,
      Some("length") => SessionStatus::Interrupted,
      // A turn that ends with unanswered tool calls was cut off before they ran.
      _ if has_tool_call(message.content.as_ref()) => SessionStatus::Interrupted,
      _ => SessionStatus::Complete,
    },
    // Tools ran, but the agent never produced the assistant turn that follows.
    "toolResult" => SessionStatus::Interrupted,
    "user" => SessionStatus::Pending,
    _ => SessionStatus::Unknown,
  }
}

/// Whether `line` is a complete record of an assistant message. A line that is not JSON is not one.
pub(crate) fn is_assistant_message(line: &[u8]) -> bool {
  line.first() == Some(&b'{')
    && serde_json::from_slice::<Record>(line)
      .ok()
      .is_some_and(|record| record.kind == "message" && record.message.is_some_and(|message| message.role == "assistant"))
}

fn has_tool_call(content: Option<&serde_json::Value>) -> bool {
  content.and_then(serde_json::Value::as_array).is_some_and(|blocks| {
    blocks
      .iter()
      .any(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("toolCall"))
  })
}

#[cfg(test)]
mod tests {
  use alvin_protocol::SessionStatus;

  use super::status_of;

  fn tail(records: &[&str]) -> Vec<u8> {
    let mut text = records.join("\n");
    text.push('\n');
    text.into_bytes()
  }

  fn assistant(stop: &str, content: &str) -> String {
    format!(r#"{{"type":"message","message":{{"role":"assistant","content":{content},"stopReason":"{stop}"}}}}"#)
  }

  #[test]
  fn the_last_message_decides_the_status() {
    let text = r#"[{"type":"text","text":"ok"}]"#;
    let call = r#"[{"type":"toolCall","id":"1","name":"read"}]"#;
    let cases = [
      (assistant("stop", text), SessionStatus::Complete),
      (assistant("stop", call), SessionStatus::Interrupted),
      (assistant("length", text), SessionStatus::Interrupted),
      (assistant("aborted", text), SessionStatus::Aborted),
      (assistant("error", text), SessionStatus::Error),
      (
        r#"{"type":"message","message":{"role":"toolResult"}}"#.to_owned(),
        SessionStatus::Interrupted,
      ),
      (
        r#"{"type":"message","message":{"role":"user","content":"hi"}}"#.to_owned(),
        SessionStatus::Pending,
      ),
    ];
    for (record, expected) in cases {
      let trailing = r#"{"type":"custom","customType":"session_exit"}"#;
      assert_eq!(status_of(&tail(&[&record, trailing])), expected, "{record}");
    }
  }

  #[test]
  fn a_window_without_a_message_is_unknown() {
    assert_eq!(status_of(b""), SessionStatus::Unknown);
    assert_eq!(
      status_of(&tail(&[r#"{"type":"custom","customType":"session_exit"}"#])),
      SessionStatus::Unknown
    );
  }

  #[test]
  fn the_fragment_the_window_starts_in_is_skipped_and_a_torn_end_does_not_hide_earlier_messages() {
    let complete = assistant("stop", r#"[{"type":"text","text":"ok"}]"#);
    let mut bytes = br#"ssage","message":{"role":"user"}}"#.to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(&tail(&[&complete]));
    bytes.extend_from_slice(br#"{"type":"message","message":{"role":"us"#);

    assert_eq!(status_of(&bytes), SessionStatus::Complete);
  }

  #[test]
  fn a_multi_megabyte_session_is_read_through_its_head_and_tail_windows_only() {
    use std::io::{Read, Seek, SeekFrom, Write};

    use crate::scan::{HEAD_BYTES, head_bytes};
    use crate::status::{TAIL_BYTES, read_tail};
    use crate::test_support::{TempDir, run_omp};

    /// A real file handle that counts the bytes read through it.
    struct Counting {
      file: std::fs::File,
      read: u64,
    }
    impl Read for Counting {
      fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(buf)?;
        self.read += n as u64;
        Ok(n)
      }
    }
    impl Seek for Counting {
      fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(to)
      }
    }

    let tmp = TempDir::new();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let real = run_omp(&project, &tmp.path().join("sessions"), "Reply with the single word ok.");
    let real = std::fs::read_to_string(real).unwrap();
    let lines: Vec<&str> = real.lines().collect();
    // Head records, then 16 MiB that is not JSON at all, then the real closing records.
    let big = tmp.path().join("big.jsonl");
    let mut file = std::fs::File::create(&big).unwrap();
    writeln!(file, "{}", lines[..4].join("\n")).unwrap();
    file.write_all(&vec![0xff; 16 * 1024 * 1024]).unwrap();
    writeln!(file, "\n{}", lines[lines.len() - 2..].join("\n")).unwrap();
    drop(file);

    let mut handle = Counting {
      file: std::fs::File::open(&big).unwrap(),
      read: 0,
    };
    let head = head_bytes(&mut handle.file).unwrap();
    let tail = read_tail(&mut handle).unwrap();

    assert_eq!(head.len() as u64, HEAD_BYTES);
    assert!(
      handle.read <= TAIL_BYTES,
      "read {} bytes for the tail of a 16 MiB file",
      handle.read
    );
    assert_eq!(status_of(&tail), SessionStatus::Complete);
  }
}
