//! Identities and token counts shared across the rings.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
/// Token counts of a request.
pub struct Usage {
    /// How much of the prompt the provider served from its cache. Part of `prompt_tokens`.
    #[serde(default, rename = "prompt_tokens_details", deserialize_with = "cached_tokens")]
    pub cached_tokens: u32,
    /// Tokens in the reply.
    pub completion_tokens: u32,
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Prompt and reply together.
    pub total_tokens: u32,
}

impl Usage {
    /// Sums two usages, e.g. across the steps of one agent turn.
    #[must_use]
    #[inline]
    pub const fn merge(self, other: Self) -> Self {
        Self {
            prompt_tokens: self.prompt_tokens.saturating_add(other.prompt_tokens),
            completion_tokens: self.completion_tokens.saturating_add(other.completion_tokens),
            total_tokens: self.total_tokens.saturating_add(other.total_tokens),
            cached_tokens: self.cached_tokens.saturating_add(other.cached_tokens),
        }
    }
}

/// Chat completions report cached tokens inside `prompt_tokens_details`.
fn cached_tokens<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Details {
        #[serde(default)]
        cached_tokens: u32,
    }
    Ok(Option::<Details>::deserialize(deserializer)?.map_or(0, |details| details.cached_tokens))
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
