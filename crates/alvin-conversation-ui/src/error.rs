//! Errors from terminal operations and speech input.

use std::error::Error as StdError;

/// A terminal or speech operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// A gateway operation failed.
  #[error("terminal gateway: {0}")]
  Gateway(#[source] alvin_client::Error),
  /// The terminal engine or its byte stream failed.
  #[error("terminal engine: {0}")]
  TerminalEngine(#[source] std::io::Error),
  /// The engine refused input.
  #[error("terminal input: {0}")]
  TerminalInput(#[source] Box<dyn StdError + Send + Sync>),
  /// The terminal engine thread panicked during shutdown.
  #[error("terminal engine thread panicked")]
  TerminalThread,
  /// Base64-encoded image data could not be decoded.
  #[error("Invalid image data: {source}")]
  ImageData {
    /// Underlying base64 decoding error.
    #[source]
    source: base64::DecodeError,
  },
  /// Inline image data had no MIME type.
  #[error("Image data is missing a MIME type")]
  ImageMimeMissing,
  /// Inline image data used an unsupported MIME type.
  #[error("Unsupported image format: {mime}")]
  ImageMimeUnsupported {
    /// MIME type supplied by the image.
    mime: String,
  },
  /// An image contained neither inline data nor a URL.
  #[error("Image has no data or URL")]
  ImageSourceMissing,
  /// Local speech model preparation or inference failed.
  #[error("speech recognition: {source}")]
  Speech {
    #[source]
    source: alvin_stt::Error,
  },
  /// The audio input or recognizer failed.
  #[error("speech capture: {source}")]
  SpeechCapture {
    #[source]
    source: gpui_kit::component::speech::SpeechError,
  },
}

/// A terminal operation result.
pub type Result<T, E = Error> = std::result::Result<T, E>;
