use thoth_core::session::stats::Stats;

/// The usage totals as the footer shows them.
pub fn summary(stats: &Stats) -> String {
    let last = match stats.last {
        Some(u) if u.cached_tokens > 0 => {
            format!("{} in ({} cached) / {} out", u.prompt_tokens, u.cached_tokens, u.completion_tokens)
        },
        Some(u) => format!("{} in / {} out", u.prompt_tokens, u.completion_tokens),
        None => "-".to_owned(),
    };
    let cached = match stats.cached_tokens {
        0 => String::new(),
        tokens => format!(", {tokens} cached"),
    };
    format!(
        "Requests: {} | Last: {} | Total: {} tokens ({} in / {} out{cached})",
        stats.requests,
        last,
        stats.total_tokens(),
        stats.prompt_tokens,
        stats.completion_tokens
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use thoth_core::llm::client::chat::Usage;

    fn usage(prompt: u32, completion: u32) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt.saturating_add(completion),
            ..Default::default()
        }
    }

    #[test]
    fn starts_empty() {
        assert_eq!(summary(&Stats::default()), "Requests: 0 | Last: - | Total: 0 tokens (0 in / 0 out)");
    }

    #[test]
    fn accumulates_across_requests() {
        let mut stats = Stats::default();
        stats.record(Some(usage(26, 6)));
        stats.record(Some(usage(40, 10)));
        assert_eq!(summary(&stats), "Requests: 2 | Last: 40 in / 10 out | Total: 82 tokens (66 in / 16 out)");
    }

    #[test]
    fn cached_input_is_shown_only_when_there_is_some() {
        let mut stats = Stats::default();
        stats.record(Some(Usage { cached_tokens: 30, ..usage(40, 10) }));
        stats.record(Some(Usage { cached_tokens: 20, ..usage(50, 5) }));
        assert_eq!(
            summary(&stats),
            "Requests: 2 | Last: 50 in (20 cached) / 5 out | Total: 105 tokens (90 in / 15 out, 50 cached)"
        );
    }

    #[test]
    fn counts_request_without_usage() {
        let mut stats = Stats::default();
        stats.record(Some(usage(5, 5)));
        stats.record(None);
        assert_eq!(summary(&stats), "Requests: 2 | Last: 5 in / 5 out | Total: 10 tokens (5 in / 5 out)");
    }
}
