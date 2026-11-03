//! Errors from the terminal's engine and gateway connection.

use std::error::Error as StdError;
use std::fmt;

/// A terminal operation failed.
#[derive(Debug)]
pub enum Error {
  /// A gateway operation failed.
  Gateway(alvin_client::Error),
  /// The terminal engine or its byte stream failed.
  TerminalEngine(std::io::Error),
  /// The engine refused input.
  TerminalInput(Box<dyn StdError + Send + Sync>),
  /// The terminal engine thread panicked during shutdown.
  TerminalThread,
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Gateway(source) => write!(f, "terminal gateway: {source}"),
      Self::TerminalEngine(source) => write!(f, "terminal engine: {source}"),
      Self::TerminalInput(source) => write!(f, "terminal input: {source}"),
      Self::TerminalThread => f.write_str("terminal engine thread panicked"),
    }
  }
}

impl StdError for Error {
  fn source(&self) -> Option<&(dyn StdError + 'static)> {
    match self {
      Self::Gateway(source) => Some(source),
      Self::TerminalEngine(source) => Some(source),
      Self::TerminalInput(source) => Some(source.as_ref()),
      Self::TerminalThread => None,
    }
  }
}

/// A terminal operation result.
pub type Result<T, E = Error> = std::result::Result<T, E>;
