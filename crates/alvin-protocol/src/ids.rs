//! Identifiers. Gateway-minted ids are UUIDv7, so they sort in mint order. Project and checkout ids are
//! digests of a canonical root path. Session ids are minted by omp.

use serde::{Deserialize, Serialize};
use uuid::{Uuid, Version};

use crate::{Error, Result};

macro_rules! uuid_v7_id {
  ($(#[$meta:meta])* $name:ident) => {
    $(#[$meta])*
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(try_from = "Uuid", into = "Uuid")]
    pub struct $name(Uuid);

    impl $name {
      /// Mints a fresh id.
      pub fn new() -> Self {
        Self(Uuid::now_v7())
      }

      /// The underlying UUID.
      pub fn as_uuid(&self) -> &Uuid {
        &self.0
      }
    }

    impl Default for $name {
      fn default() -> Self {
        Self::new()
      }
    }

    impl TryFrom<Uuid> for $name {
      type Error = Error;

      fn try_from(id: Uuid) -> Result<Self> {
        match id.get_version() {
          Some(Version::SortRand) => Ok(Self(id)),
          _ => Err(Error::NotUuidV7 { id }),
        }
      }
    }

    impl From<$name> for Uuid {
      fn from(id: $name) -> Uuid {
        id.0
      }
    }
  };
}

uuid_v7_id! {
  /// One live session: the omp process the gateway supervises for it.
  LiveSessionId
}

uuid_v7_id! {
  /// One event in a live session's log; subscriptions resume after an `EventId`.
  EventId
}

macro_rules! digest_id {
  ($(#[$meta:meta])* $name:ident) => {
    $(#[$meta])*
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct $name([u8; 32]);

    impl $name {
      /// Wraps a 32-byte digest.
      pub fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
      }

      /// The digest bytes.
      pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
      }
    }

    impl std::fmt::Display for $name {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
      }
    }

    impl std::str::FromStr for $name {
      type Err = Error;

      fn from_str(value: &str) -> Result<Self> {
        let invalid = || Error::InvalidDigest { value: value.to_owned() };
        let bytes = value.as_bytes();
        if bytes.len() != 64 {
          return Err(invalid());
        }
        let mut digest = [0u8; 32];
        for (slot, pair) in digest.iter_mut().zip(bytes.chunks_exact(2)) {
          let high = pair.first().copied().and_then(lower_hex_digit).ok_or_else(invalid)?;
          let low = pair.get(1).copied().and_then(lower_hex_digit).ok_or_else(invalid)?;
          *slot = (high << 4) | low;
        }
        Ok(Self(digest))
      }
    }

    impl Serialize for $name {
      fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
      }
    }

    impl<'de> Deserialize<'de> for $name {
      fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
      }
    }
  };
}

/// Value of a lowercase hex digit; uppercase is rejected so every digest has one serialized form.
fn lower_hex_digit(byte: u8) -> Option<u8> {
  match byte {
    b'0'..=b'9' => Some(byte - b'0'),
    b'a'..=b'f' => Some(byte - b'a' + 10),
    _ => None,
  }
}

digest_id! {
  /// A repository: blake3 of its canonical root path, serialized as 64 lowercase hex digits.
  ProjectId
}

digest_id! {
  /// A checkout (main working copy, git worktree, or jj workspace): blake3 of its canonical root path.
  CheckoutId
}

/// An omp session id. omp mints it and writes it as a plain string, so it is not parsed as a UUID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
  /// The id as omp wrote it.
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl From<String> for SessionId {
  fn from(id: String) -> Self {
    Self(id)
  }
}

#[cfg(test)]
mod tests {
  use uuid::Uuid;

  use super::{CheckoutId, EventId, LiveSessionId, ProjectId};
  use crate::Error;

  #[test]
  fn digest_id_serializes_as_64_lowercase_hex_and_round_trips() {
    let mut digest = [0u8; 32];
    digest[0] = 0xab;
    digest[31] = 0x07;
    let id = ProjectId::from_digest(digest);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"ab{}07\"", "00".repeat(30)));
    assert_eq!(serde_json::from_str::<ProjectId>(&json).unwrap(), id);
  }

  #[test]
  fn digest_id_rejects_uppercase_short_and_non_hex() {
    assert!(matches!("AB".repeat(32).parse::<CheckoutId>(), Err(Error::InvalidDigest { .. })));
    assert!("abcd".parse::<CheckoutId>().is_err());
    assert!("g".repeat(64).parse::<CheckoutId>().is_err());
  }

  #[test]
  fn minted_ids_are_version_7() {
    assert_eq!(EventId::new().as_uuid().get_version_num(), 7);
    assert_eq!(LiveSessionId::new().as_uuid().get_version_num(), 7);
  }

  #[test]
  fn ids_sort_in_mint_order() {
    let minted: Vec<EventId> = (0..1000).map(|_| EventId::new()).collect();
    assert!(minted.is_sorted());
  }

  #[test]
  fn deserializing_a_non_v7_uuid_fails() {
    let v4 = Uuid::from_u128(0x0123_4567_89ab_4def_8123_4567_89ab_cdef);
    let json = serde_json::to_string(&v4).unwrap();
    assert!(serde_json::from_str::<EventId>(&json).is_err());
    assert!(matches!(EventId::try_from(v4), Err(Error::NotUuidV7 { id }) if id == v4));
  }

  #[test]
  fn id_round_trips_as_a_bare_uuid_string() {
    let id = LiveSessionId::new();
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{}\"", id.as_uuid()));
    assert_eq!(serde_json::from_str::<LiveSessionId>(&json).unwrap(), id);
  }
}
