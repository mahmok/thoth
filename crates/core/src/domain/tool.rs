//! What a Tool call returns.

use crate::domain::shared::Usage;

#[derive(Debug, Clone, PartialEq, Eq)]
/// What a Tool returns.
pub struct Outcome {
    /// What the model reads.
    pub content: String,
    /// Whether the call failed.
    pub is_error: bool,
    /// Tokens the call spent on a model of its own, such as a sub-agent's. They count toward the
    /// Turn that ran the call.
    pub usage: Option<Usage>,
}

impl Outcome {
    #[inline]
    pub fn error<IntoString>(message: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self { content: format!("error: {}", message.into()), is_error: true, usage: None }
    }

    #[inline]
    pub fn ok<IntoString>(content: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self { content: content.into(), is_error: false, usage: None }
    }

    #[must_use]
    #[inline]
    pub const fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

impl From<Result<String, String>> for Outcome {
    #[inline]
    fn from(result: Result<String, String>) -> Self {
        match result {
            Ok(content) => Self::ok(content),
            Err(message) => Self::error(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_results_are_marked_and_prefixed() {
        let result = Outcome::error("boom");
        assert!(result.is_error);
        assert_eq!(result.content, "error: boom");
        assert!(!Outcome::ok("fine").is_error);
    }
}
