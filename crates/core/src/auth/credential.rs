use core::fmt;
use core::fmt::Debug;
use core::fmt::Formatter;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// A stored login.
pub enum Credential {
    /// A key the user pasted.
    ApiKey {
        /// The key.
        key: String,
    },
    /// A token that is renewed.
    Oauth {
        /// The token that gets new access tokens.
        refresh: String,
        /// The token that authorises requests.
        access: String,
        /// When `access` stops working, in milliseconds since the epoch.
        expires: u64,
    },
}

impl Debug for Credential {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ApiKey { .. } => f.write_str("Credential::ApiKey(..)"),
            Self::Oauth { expires, .. } => write!(f, "Credential::Oauth(expires {expires})"),
        }
    }
}

#[must_use]
#[inline]
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
