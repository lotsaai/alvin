use std::path::PathBuf;

/// Failure preparing or using the local speech model.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// The model cache could not be accessed.
  #[error("accessing speech cache {}", path.display())]
  CacheIo {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  /// A model asset request failed.
  #[error("downloading speech asset {}", path.display())]
  Download {
    path: PathBuf,
    #[source]
    source: ureq::Error,
  },
  /// Reading or writing a downloaded asset failed.
  #[error("transferring speech asset {}", path.display())]
  DownloadIo {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  /// The downloaded asset has an unexpected size.
  #[error("speech asset {} has {actual} bytes, expected {expected}", path.display())]
  AssetLength { path: PathBuf, expected: u64, actual: u64 },
  /// The native runtime could not initialize.
  #[error("initializing speech runtime")]
  Runtime {
    #[source]
    source: parakeet_rs::ort::Error,
  },
  /// The speech model could not be opened.
  #[error("loading speech model {}", path.display())]
  ModelLoad {
    path: PathBuf,
    #[source]
    source: parakeet_rs::Error,
  },
  /// Inference failed.
  #[error("transcribing speech")]
  Transcription {
    #[source]
    source: parakeet_rs::Error,
  },
  /// The progress consumer cancelled preparation.
  #[error("speech model preparation cancelled")]
  Cancelled,
}

/// Result of preparing or using the speech model.
pub type Result<T> = std::result::Result<T, Error>;
