//! The crate's error type. Every variant that comes from `omp-rpc` keeps it as its `#[source]` and names the session's
//! working directory and, where one is involved, the command.

use std::path::PathBuf;

use crate::transport::Error as RpcError;

/// What can go wrong supervising an omp process.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// The omp process could not be started or did not complete its handshake.
  #[error("starting omp in {cwd}")]
  Spawn {
    /// The working directory the session was to run in.
    cwd: PathBuf,
    /// The failure.
    #[source]
    source: Box<RpcError>,
  },

  /// omp did not send its `ready` frame in time.
  #[error("omp did not become ready in {cwd}")]
  ReadyTimeout {
    /// The working directory the session was to run in.
    cwd: PathBuf,
    /// The timeout.
    #[source]
    source: Box<RpcError>,
  },

  /// omp rejected a command. `code` is omp's own error code when it sent one.
  #[error("omp command `{command}` failed in {cwd}")]
  Command {
    /// The session's working directory.
    cwd: PathBuf,
    /// The command's wire name.
    command: &'static str,
    /// omp's error code, if any.
    code: Option<String>,
    /// omp's answer.
    #[source]
    source: Box<RpcError>,
  },

  /// The omp process is gone: it exited or the session was closed.
  #[error("omp has exited in {cwd}")]
  ProcessExited {
    /// The session's working directory.
    cwd: PathBuf,
    /// What the client saw.
    #[source]
    source: Box<RpcError>,
  },

  /// Any other failure talking to omp: a deadline, a malformed frame, a broken pipe.
  #[error("omp command `{command}` could not be completed in {cwd}")]
  Rpc {
    /// The session's working directory.
    cwd: PathBuf,
    /// The command's wire name.
    command: &'static str,
    /// The failure.
    #[source]
    source: Box<RpcError>,
  },

  /// An entry of omp's history lacks the fields every entry has.
  #[error("history entry {entry} is malformed")]
  Entry {
    /// The entry's id, when it has one.
    entry: String,
    /// What is missing or wrong.
    #[source]
    source: serde_json::Error,
  },

  /// A value could not be converted between `omp-rpc`'s generated types and the protocol's.
  #[error("converting {what} between omp-rpc and the protocol")]
  Convert {
    /// The value's name.
    what: &'static str,
    /// The failure.
    #[source]
    source: serde_json::Error,
  },

  /// Closing could not confirm that omp and its children are gone.
  #[error("tearing down omp in {cwd}")]
  Teardown {
    /// The session's working directory.
    cwd: PathBuf,
    /// The failure.
    #[source]
    source: Box<RpcError>,
  },

  /// A hook cancelled the switch to the session file being resumed.
  #[error("omp did not switch to {path} in {cwd}")]
  ResumeCancelled {
    /// The session's working directory.
    cwd: PathBuf,
    /// The session file.
    path: PathBuf,
  },

  /// A hook cancelled creation of a new session.
  #[error("omp did not create a new session in {cwd}")]
  NewSessionCancelled {
    /// The working directory the process was started in.
    cwd: PathBuf,
  },

  /// omp's protocol carries paths as strings, and this one is not UTF-8.
  #[error("{path} is not valid UTF-8")]
  PathNotUtf8 {
    /// The path.
    path: PathBuf,
  },
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
  /// Classifies an `omp-rpc` failure of `command` in `cwd`.
  pub(crate) fn from_rpc(cwd: &std::path::Path, command: &'static str, source: RpcError) -> Self {
    let cwd = cwd.to_path_buf();
    let source = Box::new(source);
    match *source {
      RpcError::Closed => Self::ProcessExited { cwd, source },
      RpcError::Command { ref code, .. } => Self::Command {
        cwd,
        command,
        code: code.clone(),
        source,
      },
      _ => Self::Rpc { cwd, command, source },
    }
  }
}
