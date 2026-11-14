use core::fmt;
use core::fmt::Debug;
use core::fmt::Formatter;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey { key: String },
    Oauth { refresh: String, access: String, expires: u64 },
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
