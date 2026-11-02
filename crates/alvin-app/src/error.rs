//! Errors reading or writing local project presentation preferences.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
  #[error("reading project preferences at {}: {source}", path.display())]
  ReadPreferences {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error("parsing project preferences at {}: {source}", path.display())]
  ParsePreferences {
    path: PathBuf,
    #[source]
    source: serde_json::Error,
  },
  #[error("invalid project preferences at {}", path.display())]
  InvalidPreferences { path: PathBuf },
  #[error("writing project preferences at {}: {source}", path.display())]
  WritePreferences {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}
