/// A Tool call card.
pub mod tool_call;

use crate::domain::tool::Outcome as ToolResult;

use tool_call::ToolCall;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Who said a message.
pub enum Speaker {
    Agent,
    Error,
    Login,
    Models,
    Notice,
    Skills,
    Thinking,
    Tool,
    You,
}

#[derive(Debug, Clone)]
/// A message in the transcript.
pub struct Message {
    pub speaker: Speaker,
    pub text: String,
}

#[derive(Debug, Clone)]
/// A message or a Tool card.
pub enum Content {
    Message(Message),
    Tool(ToolCall),
}

#[derive(Debug, Clone)]
/// One entry of the transcript.
pub struct Item {
    /// The message or card.
    pub content: Content,
    /// Unique in the transcript.
    pub id: u64,
    /// Counts changes, so a client knows to redraw.
    pub revision: u64,
}

#[derive(Debug, Clone)]
/// A change to the transcript.
pub enum TranscriptChange {
    /// An item was added.
    Added(Item),
    /// Text was added to a message.
    Appended {
        /// The message it extends.
        id: u64,
        /// The message's new revision.
        revision: u64,
        /// The text to add.
        text: String,
    },
    /// The transcript was emptied.
    Cleared,
    /// An item was replaced whole.
    Replaced(Item),
}

#[derive(Debug, Clone, Default)]
/// What the user sees.
pub struct Transcript {
    items: Vec<Item>,
    next_id: u64,
}

impl Transcript {
    #[inline]
    /// Applies a change a Session sent, keeping the transcript in step.
    pub fn apply(&mut self, change: &TranscriptChange) {
        match *change {
            TranscriptChange::Added(ref item) => {
                self.next_id = self.next_id.max(item.id.saturating_add(1));
                self.items.push(item.clone());
            },
            TranscriptChange::Appended { id, revision, ref text } => {
                if let Some(item) = self.find_mut(id)
                    && let Content::Message(ref mut message) = item.content
                {
                    message.text.push_str(text);
                    item.revision = revision;
                }
            },
            TranscriptChange::Replaced(ref replacement) => {
                if let Some(item) = self.find_mut(replacement.id) {
                    *item = replacement.clone();
                }
            },
            TranscriptChange::Cleared => self.items.clear(),
        }
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().rev().find(|item| item.id == id)
    }

    /// Whether a Tool card is still running. Only the cards of the current Turn can be, so the
    /// search stops at the prompt that began it.
    #[must_use]
    #[inline]
    pub fn has_running_tool(&self) -> bool {
        self.items
            .iter()
            .rev()
            .take_while(|item| !matches!(&item.content, Content::Message(m) if m.speaker == Speaker::You))
            .any(|item| matches!(&item.content, Content::Tool(call) if call.is_running()))
    }

    #[must_use]
    #[inline]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    const fn new_item(&self, content: Content) -> Item {
        Item { id: self.next_id, revision: 0, content }
    }

    /// The running card for `call_id`, changed by `change`. The card is replaced whole, with a
    /// new revision, so a client redraws it.
    #[expect(clippy::unreachable, reason = "only Tool cards are looked up by call id")]
    fn replace_card(item: &Item, change: impl FnOnce(&mut ToolCall)) -> TranscriptChange {
        let mut call = match item.content {
            Content::Tool(ref call) => call.clone(),
            Content::Message(_) => unreachable!("only Tool cards are looked up by call id"),
        };
        change(&mut call);
        TranscriptChange::Replaced(Item {
            id: item.id,
            revision: item.revision.saturating_add(1),
            content: Content::Tool(call),
        })
    }

    /// The running card for call `call_id`, if there is one.
    fn running_card(&self, call_id: &str) -> Option<&Item> {
        self.items
            .iter()
            .rev()
            .find(|item| matches!(&item.content, Content::Tool(call) if call.call_id == call_id && call.is_running()))
    }

    #[inline]
    pub fn said<IntoString>(&self, speaker: Speaker, text: IntoString) -> TranscriptChange
    where
        IntoString: Into<String>,
    {
        TranscriptChange::Added(self.new_item(Content::Message(Message { speaker, text: text.into() })))
    }

    /// The change a streamed fragment makes: it joins the current message of `speaker`, if there
    /// is one, and starts a new message otherwise.
    #[must_use]
    #[inline]
    pub fn streamed(&self, speaker: Speaker, delta: String) -> TranscriptChange {
        match self.items.last() {
            Some(item) if matches!(&item.content, Content::Message(m) if m.speaker == speaker) => {
                TranscriptChange::Appended { id: item.id, revision: item.revision.saturating_add(1), text: delta }
            },
            _ => self.said(speaker, delta),
        }
    }

    /// The change a started Tool call makes: a new running card.
    #[must_use]
    #[inline]
    pub fn started_tool(&self, call_id: String, summary: String, started_at: u64) -> TranscriptChange {
        TranscriptChange::Added(self.new_item(Content::Tool(ToolCall::running(call_id, summary, started_at))))
    }

    /// The change a finished Tool call makes, at `finished_at`.
    #[must_use]
    #[expect(clippy::option_if_let_else, reason = "both branches consume `finish`")]
    pub fn tool_finished(
        &self,
        call_id: &str,
        summary: String,
        result: &ToolResult,
        finished_at: u64,
    ) -> TranscriptChange {
        let finish = |call: &mut ToolCall| call.finish(finished_at, result.is_error, &result.content);
        if let Some(item) = self.running_card(call_id) {
            Self::replace_card(item, finish)
        } else {
            let mut call = ToolCall::running(call_id, summary, finished_at);
            finish(&mut call);
            TranscriptChange::Added(self.new_item(Content::Tool(call)))
        }
    }

    /// The change a progress report makes, or `None` when its card has already finished.
    #[must_use]
    #[inline]
    pub fn tool_progress(&self, call_id: &str, status: String) -> Option<TranscriptChange> {
        let item = self.running_card(call_id)?;
        Some(Self::replace_card(item, |call| call.progress = Some(status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::transcript::tool_call::ToolStatus;

    const NOW: u64 = 1_000;

    fn record(transcript: &mut Transcript, change: &TranscriptChange) {
        transcript.apply(change);
    }

    fn say(transcript: &mut Transcript, speaker: Speaker, text: &str) {
        let change = transcript.said(speaker, text);
        record(transcript, &change);
    }

    fn stream(transcript: &mut Transcript, speaker: Speaker, delta: &str) {
        let change = transcript.streamed(speaker, delta.to_owned());
        record(transcript, &change);
    }

    fn start(transcript: &mut Transcript, id: &str, summary: &str) {
        let change = transcript.started_tool(id.to_owned(), summary.to_owned(), NOW);
        record(transcript, &change);
    }

    fn finish(transcript: &mut Transcript, id: &str, summary: &str, result: &ToolResult) {
        let change = transcript.tool_finished(id, summary.to_owned(), result, NOW.saturating_add(250));
        record(transcript, &change);
    }

    fn messages(transcript: &Transcript) -> Vec<(Speaker, String)> {
        transcript
            .items()
            .iter()
            .filter_map(|item| match item.content {
                Content::Message(ref m) => Some((m.speaker, m.text.clone())),
                Content::Tool(_) => None,
            })
            .collect()
    }

    fn calls(transcript: &Transcript) -> Vec<&ToolCall> {
        transcript
            .items()
            .iter()
            .filter_map(|item| match item.content {
                Content::Tool(ref call) => Some(call),
                Content::Message(_) => None,
            })
            .collect()
    }

    #[test]
    fn streamed_text_joins_the_current_agent_reply() {
        let mut transcript = Transcript::default();
        say(&mut transcript, Speaker::You, "q");
        stream(&mut transcript, Speaker::Agent, "Hel");
        stream(&mut transcript, Speaker::Agent, "lo");
        assert_eq!(messages(&transcript), [(Speaker::You, "q".into()), (Speaker::Agent, "Hello".into())]);
    }

    #[test]
    fn thinking_streams_into_its_own_message_before_the_reply() {
        let mut transcript = Transcript::default();
        stream(&mut transcript, Speaker::Thinking, "Let me ");
        stream(&mut transcript, Speaker::Thinking, "look.");
        stream(&mut transcript, Speaker::Agent, "Found it");
        assert_eq!(
            messages(&transcript),
            [(Speaker::Thinking, "Let me look.".into()), (Speaker::Agent, "Found it".into())]
        );
    }

    #[test]
    fn a_reply_after_a_tool_call_starts_a_new_message() {
        let mut transcript = Transcript::default();
        stream(&mut transcript, Speaker::Agent, "before");
        start(&mut transcript, "call", "call");
        stream(&mut transcript, Speaker::Agent, "after");
        assert_eq!(messages(&transcript), [(Speaker::Agent, "before".into()), (Speaker::Agent, "after".into())]);
    }

    #[test]
    fn a_tool_call_appears_when_it_starts() {
        let mut transcript = Transcript::default();
        start(&mut transcript, "read_file(Cargo.toml)", "read_file(Cargo.toml)");
        let calls = calls(&transcript);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].summary, "read_file(Cargo.toml)");
        assert!(calls[0].is_running());
    }

    #[test]
    fn a_tool_result_lands_on_the_card_that_started_it() {
        let mut transcript = Transcript::default();
        start(&mut transcript, "read_file(a)", "read_file(a)");
        finish(&mut transcript, "read_file(a)", "read_file(a)", &ToolResult::ok("file contents"));
        let calls = calls(&transcript);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].status, ToolStatus::Done);
        assert_eq!(calls[0].preview(), ["file contents"]);
    }

    #[test]
    fn a_running_tool_is_found_only_in_the_current_turn() {
        let mut transcript = Transcript::default();
        assert!(!transcript.has_running_tool());
        start(&mut transcript, "bash(sleep)", "bash(sleep)");
        assert!(transcript.has_running_tool());
        finish(&mut transcript, "bash(sleep)", "bash(sleep)", &ToolResult::ok(""));
        assert!(!transcript.has_running_tool());

        // A card left running by an earlier turn does not keep the screen redrawing.
        start(&mut transcript, "bash(old)", "bash(old)");
        say(&mut transcript, Speaker::You, "next");
        assert!(!transcript.has_running_tool());
    }

    #[test]
    fn a_failed_tool_call_is_marked_failed() {
        let mut transcript = Transcript::default();
        start(&mut transcript, "read_file(../x)", "read_file(../x)");
        finish(&mut transcript, "read_file(../x)", "read_file(../x)", &ToolResult::error("denied"));
        assert_eq!(calls(&transcript)[0].status, ToolStatus::Failed);
    }

    #[test]
    fn a_result_without_a_started_call_still_shows_a_card() {
        let mut transcript = Transcript::default();
        finish(&mut transcript, "read_file(a)", "read_file(a)", &ToolResult::ok("x"));
        assert_eq!(calls(&transcript)[0].status, ToolStatus::Done);
    }

    #[test]
    fn a_change_appends_or_replaces_the_item_it_names_and_bumps_its_revision() {
        let mut transcript = Transcript::default();
        stream(&mut transcript, Speaker::Agent, "a");
        let (id, before) = (transcript.items()[0].id, transcript.items()[0].revision);
        stream(&mut transcript, Speaker::Agent, "b");
        assert_eq!(transcript.items()[0].id, id);
        assert!(transcript.items()[0].revision > before);
    }

    #[test]
    fn a_client_that_applies_the_changes_ends_with_the_same_transcript() {
        let mut session_side = Transcript::default();
        let mut client = Transcript::default();
        let mut changes = Vec::new();
        for (speaker, delta) in [(Speaker::Agent, "hi "), (Speaker::Agent, "there")] {
            let change = session_side.streamed(speaker, delta.to_owned());
            session_side.apply(&change);
            changes.push(change);
        }
        let started = session_side.started_tool("read_file(a)".into(), "read_file(a)".into(), NOW);
        session_side.apply(&started);
        changes.push(started);
        let finished = session_side.tool_finished("read_file(a)", "read_file(a)".into(), &ToolResult::ok("x"), NOW);
        session_side.apply(&finished);
        changes.push(finished);
        for recorded in &changes {
            client.apply(recorded);
        }
        assert_eq!(messages(&client), messages(&session_side));
        assert_eq!(client.items().len(), session_side.items().len());
        assert_eq!(calls(&client)[0].status, ToolStatus::Done);
        assert_eq!(calls(&client)[0].preview(), calls(&session_side)[0].preview());
    }

    #[test]
    fn ids_are_not_reused_after_a_clear() {
        let mut transcript = Transcript::default();
        say(&mut transcript, Speaker::You, "a");
        let first = transcript.items()[0].id;
        transcript.apply(&TranscriptChange::Cleared);
        say(&mut transcript, Speaker::You, "b");
        assert_ne!(transcript.items()[0].id, first);
    }

    #[test]
    fn results_and_progress_match_their_card_by_call_id_even_with_the_same_summary() {
        let mut transcript = Transcript::default();
        start(&mut transcript, "a", "read_file(x)");
        start(&mut transcript, "b", "read_file(x)");
        let progress = transcript.tool_progress("a", "reading".into()).unwrap();
        record(&mut transcript, &progress);
        finish(&mut transcript, "b", "read_file(x)", &ToolResult::ok("second"));

        let calls = calls(&transcript);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call_id, "a");
        assert_eq!(calls[0].progress.as_deref(), Some("reading"));
        assert!(calls[0].is_running());
        assert_eq!(calls[1].call_id, "b");
        assert_eq!(calls[1].status, ToolStatus::Done);
        assert_eq!(calls[1].progress, None);
    }

    #[test]
    fn progress_for_a_finished_or_unknown_card_changes_nothing() {
        let mut transcript = Transcript::default();
        start(&mut transcript, "a", "bash(x)");
        finish(&mut transcript, "a", "bash(x)", &ToolResult::ok(""));
        assert!(transcript.tool_progress("a", "too late".into()).is_none());
        assert!(transcript.tool_progress("zz", "who?".into()).is_none());
    }
}
