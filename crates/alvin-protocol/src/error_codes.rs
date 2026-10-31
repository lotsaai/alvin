//! JSON-RPC error codes the gateway returns. All sit in the JSON-RPC server-error range
//! (`-32000..=-32099`). A code is never reused once retired.

/// The `live` id names no live session.
pub const UNKNOWN_LIVE_SESSION: i32 = -32001;
/// The session id names no omp session file.
pub const UNKNOWN_SESSION: i32 = -32002;
/// The project id names no known project.
pub const UNKNOWN_PROJECT: i32 = -32003;
/// The live session is busy with a turn and cannot take the command.
pub const SESSION_BUSY: i32 = -32004;
/// An `events/subscribe` `after` id is older than the retained log, or the subscriber fell behind.
pub const STALE_CURSOR: i32 = -32005;
/// The command is not allowed in the live session's current state.
pub const COMMAND_NOT_ALLOWED: i32 = -32006;
/// omp rejected the command; the error data carries omp's own `code`.
pub const OMP_COMMAND_FAILED: i32 = -32007;
/// The omp process of the live session exited.
pub const OMP_PROCESS_EXITED: i32 = -32008;
/// The request is well-formed JSON-RPC but semantically invalid.
pub const INVALID_REQUEST: i32 = -32009;

#[cfg(test)]
mod tests {
  use super::*;

  /// The wire values clients depend on. Changing one is a breaking protocol change.
  #[test]
  fn code_table_is_pinned() {
    assert_eq!(
      [
        UNKNOWN_LIVE_SESSION,
        UNKNOWN_SESSION,
        UNKNOWN_PROJECT,
        SESSION_BUSY,
        STALE_CURSOR,
        COMMAND_NOT_ALLOWED,
        OMP_COMMAND_FAILED,
        OMP_PROCESS_EXITED,
        INVALID_REQUEST,
      ],
      [-32001, -32002, -32003, -32004, -32005, -32006, -32007, -32008, -32009],
    );
  }
}
