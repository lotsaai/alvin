//! Identifiers. Gateway-minted ids are UUIDv7, so they sort in mint order. Project and checkout ids are derived
//! from a canonical root path and session ids are minted by omp; both are opaque strings.

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

macro_rules! string_id {
  ($(#[$meta:meta])* $name:ident) => {
    $(#[$meta])*
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct $name(String);

    impl $name {
      /// The id as a string.
      pub fn as_str(&self) -> &str {
        &self.0
      }
    }

    impl From<String> for $name {
      fn from(id: String) -> Self {
        Self(id)
      }
    }
  };
}

string_id! {
  /// A repository: the gateway derives it from the canonical root path. Opaque to clients, which echo it back.
  ProjectId
}

string_id! {
  /// A checkout (main working copy, git worktree, or jj workspace): derived from the canonical root path.
  CheckoutId
}

string_id! {
  /// An omp session id. omp mints it and writes it as a plain string, so it is not parsed as a UUID.
  SessionId
}

#[cfg(test)]
mod tests {
  use uuid::Uuid;

  use super::{EventId, LiveSessionId, ProjectId};
  use crate::Error;

  #[test]
  fn string_ids_serialize_as_the_bare_string() {
    let id = ProjectId::from("abc".to_owned());
    assert_eq!(serde_json::to_string(&id).unwrap(), r#""abc""#);
    assert_eq!(serde_json::from_str::<ProjectId>(r#""abc""#).unwrap(), id);
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
