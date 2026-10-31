//! The crate's error type.

use uuid::Uuid;

/// Errors raised while parsing wire values into protocol types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// A gateway-minted id was not a version 7 UUID.
  #[error("id {id} is not a UUIDv7")]
  NotUuidV7 {
    /// The rejected value.
    id: Uuid,
  },
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
