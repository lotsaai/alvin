//! The crate's error type.

use std::path::PathBuf;

/// Errors raised while indexing omp sessions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// Workspace discovery for a session's `cwd` failed for a reason other than the `cwd` not resolving.
  #[error("resolving the workspace for {cwd}")]
  ResolveWorkspace {
    /// The `cwd` recorded in the session header.
    cwd: PathBuf,
    /// The discovery error.
    #[source]
    source: code_workspace::Error,
  },

  /// A checkout root that workspace discovery just found could not be canonicalized.
  #[error("canonicalizing checkout root {root}")]
  CanonicalizeRoot {
    /// The root workspace discovery reported.
    root: PathBuf,
    /// The underlying IO error.
    #[source]
    source: std::io::Error,
  },

  /// A directory could not be listed.
  #[error("reading directory {path}")]
  ReadDir {
    /// The directory.
    path: PathBuf,
    /// The underlying IO error.
    #[source]
    source: std::io::Error,
  },

  /// A session file could not be opened or read.
  #[error("reading session file {path}")]
  ReadFile {
    /// The session file.
    path: PathBuf,
    /// The underlying IO error.
    #[source]
    source: std::io::Error,
  },

  /// A record at the head of a session file is not valid JSON of the expected shape.
  #[error("decoding record at byte {offset} of {path}")]
  DecodeRecord {
    /// The session file.
    path: PathBuf,
    /// Byte offset of the record's line.
    offset: usize,
    /// The decoding error.
    #[source]
    source: serde_json::Error,
  },

  /// A title slot record appeared after the first line of a session file.
  #[error("title slot at byte {offset} of {path} is not the first record")]
  MisplacedTitleSlot {
    /// The session file.
    path: PathBuf,
    /// Byte offset of the record's line.
    offset: usize,
  },

  /// The head window ends before a complete session header.
  #[error("no complete session header in the head of {path}")]
  HeaderIncomplete {
    /// The session file.
    path: PathBuf,
  },
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
