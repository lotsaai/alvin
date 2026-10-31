//! JSON-RPC error codes the gateway returns, and the typed data that rides with them. All codes sit in the
//! JSON-RPC server-error range (`-32000..=-32099`). A code is never reused once retired.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A stable gateway error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
  /// The `live` id names no live session.
  UnknownLiveSession,
  /// The session id names no omp session file.
  UnknownSession,
  /// The project id names no known project.
  UnknownProject,
  /// The live session is busy with a turn and cannot take the command.
  SessionBusy,
  /// An `events/subscribe` `after` id is older than the retained log, or the subscriber fell behind.
  StaleCursor,
  /// The command is not allowed in the live session's current state.
  CommandNotAllowed,
  /// omp rejected the command; the error data carries omp's own `code`.
  OmpCommandFailed,
  /// The omp process of the live session exited.
  OmpProcessExited,
  /// The request is well-formed JSON-RPC but semantically invalid.
  InvalidRequest,
  /// Reading omp's session files failed; the error data names the path.
  SessionIndexFailed,
}

impl ErrorCode {
  const ALL: [Self; 10] = [
    Self::UnknownLiveSession,
    Self::UnknownSession,
    Self::UnknownProject,
    Self::SessionBusy,
    Self::StaleCursor,
    Self::CommandNotAllowed,
    Self::OmpCommandFailed,
    Self::OmpProcessExited,
    Self::InvalidRequest,
    Self::SessionIndexFailed,
  ];

  /// The JSON-RPC code on the wire.
  pub const fn code(self) -> i32 {
    match self {
      Self::UnknownLiveSession => -32001,
      Self::UnknownSession => -32002,
      Self::UnknownProject => -32003,
      Self::SessionBusy => -32004,
      Self::StaleCursor => -32005,
      Self::CommandNotAllowed => -32006,
      Self::OmpCommandFailed => -32007,
      Self::OmpProcessExited => -32008,
      Self::InvalidRequest => -32009,
      Self::SessionIndexFailed => -32010,
    }
  }

  /// The error code for a wire value, or `None` when the table does not contain it.
  pub fn from_code(code: i32) -> Option<Self> {
    Self::ALL.into_iter().find(|candidate| candidate.code() == code)
  }
}

/// Typed context carried in the `data` of a gateway error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ErrorData {
  /// Reading omp's session files failed at `path`.
  SessionIndex {
    /// The directory, session file, or working directory the index was reading.
    path: PathBuf,
  },
}

#[cfg(test)]
mod tests {
  use super::{ErrorCode, ErrorData};

  /// The wire values clients depend on. Changing one is a breaking protocol change.
  #[test]
  fn code_table_is_pinned() {
    assert_eq!(
      ErrorCode::ALL.map(ErrorCode::code),
      [-32001, -32002, -32003, -32004, -32005, -32006, -32007, -32008, -32009, -32010],
    );
  }

  #[test]
  fn every_code_maps_back_and_unknown_codes_do_not() {
    for code in ErrorCode::ALL {
      assert_eq!(ErrorCode::from_code(code.code()), Some(code));
    }
    assert_eq!(ErrorCode::from_code(-32099), None);
  }

  #[test]
  fn error_data_has_a_stable_wire_shape() {
    let data = ErrorData::SessionIndex { path: "/sessions".into() };
    assert_eq!(
      serde_json::to_value(&data).unwrap(),
      serde_json::json!({ "kind": "sessionIndex", "path": "/sessions" }),
    );
  }
}
