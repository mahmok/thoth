/// Output lines shown before a card is expanded.
const PREVIEW_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How far a Tool call has got.
pub enum ToolStatus {
    Done,
    Failed,
    Running,
}

/// One Tool call on the Transcript. Times are unix milliseconds, so every client shows the
/// same duration for a finished call and can count up a running one from the same start.
#[derive(Debug, Clone)]
#[expect(clippy::partial_pub_fields, reason = "the private fields keep the invariants of the type")]
pub struct ToolCall {
    /// The id the agent gave the call; finished results and progress are matched to it.
    pub call_id: String,
    /// How long it took, once it has ended.
    elapsed_ms: Option<u64>,
    /// The whole output, one entry per line.
    output: Vec<String>,
    /// How many lines of `output` are shown at first: a few, or all of them when the call failed.
    preview_len: usize,
    /// A one-line status from the running call, such as a sub-agent's current step.
    pub progress: Option<String>,
    /// When it started, in milliseconds since the epoch.
    started_at: u64,
    /// Where the call is.
    pub status: ToolStatus,
    /// A short description of the call.
    pub summary: String,
}

impl ToolCall {
    /// How long the call took, or how long it has been running as of `now`.
    #[must_use]
    #[inline]
    pub fn duration_ms(&self, now: u64) -> u64 {
        self.elapsed_ms.unwrap_or_else(|| now.saturating_sub(self.started_at))
    }

    #[inline]
    /// Records the outcome and output of the call.
    pub fn finish(&mut self, finished_at: u64, failed: bool, output: &str) {
        self.status = if failed { ToolStatus::Failed } else { ToolStatus::Done };
        self.elapsed_ms = Some(finished_at.saturating_sub(self.started_at));
        self.progress = None;
        self.output = output.lines().map(str::to_owned).collect();
        self.preview_len = if failed { self.output.len() } else { PREVIEW_LINES.min(self.output.len()) };
    }

    /// How many lines of the output the preview leaves out.
    #[must_use]
    #[inline]
    pub const fn hidden_lines(&self) -> usize {
        self.output.len().saturating_sub(self.preview_len)
    }

    #[must_use]
    #[inline]
    pub fn is_running(&self) -> bool {
        self.status == ToolStatus::Running
    }

    /// The whole output, for showing in full.
    #[must_use]
    #[inline]
    pub fn output(&self) -> &[String] {
        &self.output
    }

    /// The first lines of the output, enough to show at a glance.
    #[must_use]
    #[inline]
    pub fn preview(&self) -> &[String] {
        self.output.get(..self.preview_len).unwrap_or(&self.output)
    }

    #[inline]
    /// A Tool call that has just started.
    pub fn running<CallId, Summary>(call_id: CallId, summary: Summary, started_at: u64) -> Self
    where
        CallId: Into<String>,
        Summary: Into<String>,
    {
        Self {
            call_id: call_id.into(),
            summary: summary.into(),
            status: ToolStatus::Running,
            progress: None,
            started_at,
            elapsed_ms: None,
            output: Vec::new(),
            preview_len: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_call_reports_live_time() {
        let call = ToolCall::running("1", "read_file(a)", 1_000);
        assert_eq!(call.duration_ms(2_500), 1_500);
    }

    #[test]
    fn a_finished_call_keeps_its_time_and_previews_a_few_lines() {
        let mut call = ToolCall::running("1", "read_file(a)", 1_000);
        call.finish(1_250, false, "one\ntwo\nthree\nfour\nfive");
        assert_eq!(call.duration_ms(10_000), 250);
        assert_eq!(call.preview(), ["one", "two", "three"]);
        assert_eq!(call.hidden_lines(), 2);
        assert_eq!(call.output().len(), 5, "the whole output is kept");
    }

    #[test]
    fn a_failed_call_keeps_the_whole_error() {
        let mut call = ToolCall::running("1", "x", 0);
        call.finish(0, true, "a\nb\nc\nd");
        assert_eq!(call.preview().len(), 4);
        assert_eq!(call.hidden_lines(), 0);
    }
}
