use alloc::fmt;
use core::error;

use crate::auth::Failure as AuthError;

#[derive(Debug)]
/// Why a request to a model failed.
pub enum Failure {
    /// The API reported an error.
    Api(String),
    /// Authorising the request failed.
    Auth(AuthError),
    /// The API refused the request.
    BadRequest(String),
    /// The reply was not what was expected.
    Decode(serde_json::Error),
    /// The request could not be made or finished.
    Http(reqwest::Error),
}

impl Failure {
    #[must_use]
    #[inline]
    /// The message of a bad request error, if it is one.
    pub fn bad_request(&self) -> Option<&str> {
        match *self {
            Self::BadRequest(ref message) => Some(message),
            Self::Http(_) | Self::Decode(_) | Self::Api(_) | Self::Auth(_) => None,
        }
    }

    #[must_use]
    #[inline]
    /// Whether the error says the prompt did not fit the model's context.
    pub fn is_context_overflow(&self) -> bool {
        const PHRASES: [&str; 6] = [
            "prompt is too long",
            "context length",
            "context_length_exceeded",
            "context window",
            "maximum context",
            "too many tokens",
        ];
        let message = match *self {
            Self::BadRequest(ref message) | Self::Api(ref message) => message,
            Self::Http(_) | Self::Decode(_) | Self::Auth(_) => return false,
        };
        let lowered = message.to_lowercase();
        PHRASES.iter().any(|phrase| lowered.contains(phrase))
    }
}

impl fmt::Display for Failure {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Http(ref e) => write!(f, "http error: {e}"),
            Self::Decode(ref e) => write!(f, "decode error: {e}"),
            Self::Api(ref message) => write!(f, "api error: {message}"),
            Self::Auth(ref e) => e.fmt(f),
            Self::BadRequest(ref message) => write!(f, "bad request: {message}"),
        }
    }
}

impl error::Error for Failure {
    #[inline]
    fn cause(&self) -> Option<&dyn error::Error> {
        None
    }

    #[inline]
    fn description(&self) -> &'static str {
        "a request to a model failed"
    }

    #[inline]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

impl From<reqwest::Error> for Failure {
    #[inline]
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<serde_json::Error> for Failure {
    #[inline]
    fn from(e: serde_json::Error) -> Self {
        Self::Decode(e)
    }
}

impl From<AuthError> for Failure {
    #[inline]
    fn from(e: AuthError) -> Self {
        Self::Auth(e)
    }
}

pub(super) fn cut_off(reason: &str) -> Failure {
    Failure::Api(format!("the reply was cut off before it finished ({reason})"))
}
