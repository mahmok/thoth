use crate::llm::client::chat;

/// One Turn's token usage, as a client shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(clippy::struct_field_names, reason = "the names follow the provider's wire format or what the type wraps")]
pub struct Usage {
    /// Part of `prompt_tokens`.
    pub cached_tokens: u32,
    pub completion_tokens: u32,
    pub prompt_tokens: u32,
}

impl From<chat::Usage> for Usage {
    #[inline]
    fn from(usage: chat::Usage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            cached_tokens: usage.cached_tokens,
        }
    }
}

/// Token usage totals for a Session's Turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Part of `prompt_tokens`.
    pub cached_tokens: u64,
    pub completion_tokens: u64,
    /// The usage of the latest Turn that reported any.
    pub last: Option<Usage>,
    pub prompt_tokens: u64,
    /// Turns that finished, whether or not the provider reported usage.
    pub requests: u64,
}

impl Stats {
    #[inline]
    /// Counts a request and adds its token usage.
    pub fn record(&mut self, usage: Option<chat::Usage>) {
        self.requests = self.requests.saturating_add(1);
        if let Some(reported) = usage {
            let converted = Usage::from(reported);
            self.prompt_tokens = self.prompt_tokens.saturating_add(u64::from(converted.prompt_tokens));
            self.cached_tokens = self.cached_tokens.saturating_add(u64::from(converted.cached_tokens));
            self.completion_tokens = self.completion_tokens.saturating_add(u64::from(converted.completion_tokens));
            self.last = Some(converted);
        }
    }

    #[must_use]
    #[inline]
    pub const fn total_tokens(&self) -> u64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(prompt: u32, completion: u32, cached: u32) -> chat::Usage {
        chat::Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt.saturating_add(completion),
            cached_tokens: cached,
        }
    }

    #[test]
    fn accumulates_across_requests_and_keeps_the_last_usage() {
        let mut stats = Stats::default();
        stats.record(Some(usage(26, 6, 0)));
        stats.record(Some(usage(40, 10, 30)));
        assert_eq!((stats.requests, stats.prompt_tokens, stats.completion_tokens), (2, 66, 16));
        assert_eq!((stats.cached_tokens, stats.total_tokens()), (30, 82));
        assert_eq!(stats.last, Some(Usage { prompt_tokens: 40, completion_tokens: 10, cached_tokens: 30 }));
    }

    #[test]
    fn a_request_without_usage_is_counted_and_keeps_the_last_usage() {
        let mut stats = Stats::default();
        stats.record(Some(usage(5, 5, 0)));
        stats.record(None);
        assert_eq!(stats.requests, 2);
        assert_eq!(stats.last.map(|u| u.prompt_tokens), Some(5));
    }
}
