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
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
