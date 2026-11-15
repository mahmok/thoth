//! The identity of a Session.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
/// The identity of a Session: a ULID minted when it starts.
pub struct SessionId(Ulid);

impl SessionId {
    #[must_use]
    #[inline]
    /// An id drawn at random, for a conversation that is not a Session: a Sub-agent.
    pub fn mint() -> Self {
        Self(Ulid::generate())
    }

    #[must_use]
    #[inline]
    pub const fn from_ulid(ulid: Ulid) -> Self {
        Self(ulid)
    }
}

impl From<Ulid> for SessionId {
    #[inline]
    fn from(ulid: Ulid) -> Self {
        Self(ulid)
    }
}

impl fmt::Display for SessionId {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for SessionId {
    type Err = ulid::DecodeError;

    #[inline]
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ulid::from_str(text).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_its_bare_ulid_and_parses_back() {
        let id = SessionId::mint();
        let shown = id.to_string();
        assert_eq!(shown.len(), 26);
        assert_eq!(shown.parse::<SessionId>(), Ok(id));
    }

    #[test]
    fn an_id_is_stored_as_the_ulid_string() {
        let id = SessionId::mint();
        let stored = serde_json::to_string(&id).unwrap();
        assert_eq!(stored, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<SessionId>(&stored).unwrap(), id);
    }
}
