use core::fmt;
use core::fmt::Debug;
use core::fmt::Formatter;

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
