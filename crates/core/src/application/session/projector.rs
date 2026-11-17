//! Turns what the Agent reports into changes to the transcript.
//!
//! The transcript itself is pure: it knows how to compute a change from the state it holds, but
//! it does not read the clock or know the Agent's event enum. This module joins the two.

use crate::application::session::stats;
use crate::domain::shared::Usage as ProtocolUsage;
use crate::domain::transcript::{Speaker, Transcript, TranscriptChange};
use crate::runtime::agent::{AgentEvent, Observed};

/// Converts the usage a protocol client reports into the usage the view shows. The statistics
/// type therefore never depends on a protocol client.
#[must_use]
#[inline]
pub const fn usage(usage: ProtocolUsage) -> stats::Usage {
    stats::Usage {
        cached_tokens: usage.cached_tokens,
        completion_tokens: usage.completion_tokens,
        prompt_tokens: usage.prompt_tokens,
    }
}

/// The change `event` makes to `transcript` at `now`, or `None` when there is nothing to show for
/// it (a step marker, or progress for a card that has already finished).
#[must_use]
pub fn change(transcript: &Transcript, event: AgentEvent, now: u64) -> Option<TranscriptChange> {
    Some(match Observed::of(event) {
        Observed::Text(delta) => transcript.streamed(Speaker::Agent, delta),
        Observed::Reasoning(delta) => transcript.streamed(Speaker::Thinking, delta),
        Observed::Notice(text) => transcript.said(Speaker::Notice, text),
        Observed::Step(_) => return None,
        Observed::ToolStarted { id, summary } => transcript.started_tool(id, summary, now),
        Observed::ToolProgress { id, status } => return transcript.tool_progress(&id, status),
        Observed::ToolFinished { id, summary, result } => transcript.tool_finished(&id, summary, &result, now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tool::Outcome as ToolResult;
    use crate::domain::transcript::Content;
    use crate::domain::transcript::tool_call::ToolStatus;

    const NOW: u64 = 1_000;

    fn apply(transcript: &mut Transcript, event: AgentEvent) {
        let change = change(transcript, event, NOW).expect("the event changes the transcript");
        transcript.apply(&change);
    }

    #[test]
    fn text_and_reasoning_stream_into_their_own_messages() {
        let mut transcript = Transcript::default();
        apply(&mut transcript, AgentEvent::Reasoning("think".into()));
        apply(&mut transcript, AgentEvent::Text("say".into()));
        let messages: Vec<_> = transcript
            .items()
            .iter()
            .filter_map(|item| match item.content {
                Content::Message(ref message) => Some(message.text.clone()),
                Content::Tool(_) => None,
            })
            .collect();
        assert_eq!(messages, ["think", "say"]);
    }

    #[test]
    fn a_notice_becomes_a_notice_message() {
        let mut transcript = Transcript::default();
        apply(&mut transcript, AgentEvent::Notice("careful".into()));
        assert!(matches!(
            transcript.items()[0].content,
            Content::Message(ref message) if message.speaker == Speaker::Notice && message.text == "careful"
        ));
    }

    #[test]
    fn protocol_usage_becomes_the_view_usage() {
        let reported = ProtocolUsage { cached_tokens: 3, completion_tokens: 5, prompt_tokens: 10, total_tokens: 15 };
        assert_eq!(usage(reported), stats::Usage { cached_tokens: 3, completion_tokens: 5, prompt_tokens: 10 });
    }

    #[test]
    fn a_step_marker_changes_nothing() {
        let transcript = Transcript::default();
        assert!(change(&transcript, AgentEvent::StepStarted { step: 1 }, NOW).is_none());
    }

    #[test]
    fn a_tool_call_starts_and_finishes_at_the_given_time() {
        let mut transcript = Transcript::default();
        let started = AgentEvent::ToolStarted { id: "a".into(), summary: "read_file(x)".into() };
        apply(&mut transcript, started);
        let finished =
            AgentEvent::ToolFinished { id: "a".into(), summary: "read_file(x)".into(), result: ToolResult::ok("x") };
        apply(&mut transcript, finished);
        let call = match transcript.items()[0].content {
            Content::Tool(ref call) => call,
            Content::Message(_) => panic!("the event should make a tool card"),
        };
        assert_eq!(call.status, ToolStatus::Done);
        assert_eq!(call.duration_ms(NOW.saturating_add(9_999)), 0);
    }

    #[test]
    fn progress_for_a_finished_or_unknown_card_changes_nothing() {
        let mut transcript = Transcript::default();
        apply(&mut transcript, AgentEvent::ToolStarted { id: "a".into(), summary: "bash(x)".into() });
        let finished =
            AgentEvent::ToolFinished { id: "a".into(), summary: "bash(x)".into(), result: ToolResult::ok("") };
        apply(&mut transcript, finished);
        let late = AgentEvent::ToolProgress { id: "a".into(), status: "too late".into() };
        assert!(change(&transcript, late, NOW).is_none());
        let unknown = AgentEvent::ToolProgress { id: "zz".into(), status: "who?".into() };
        assert!(change(&transcript, unknown, NOW).is_none());
    }
}
