/// A Tool call card.
pub mod tool_call;

use crate::agent::Event as AgentEvent;
use crate::agent::tools::tool::Outcome as ToolResult;
use crate::auth::credential::now_ms;

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
pub enum Change {
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
    /// The change an agent event makes, or `None` when there is nothing to show for it (progress
    /// for a card that has already finished).
    #[must_use]
    #[inline]
    pub fn agent_event(&self, event: AgentEvent) -> Option<Change> {
        Some(match event {
            AgentEvent::Text(delta) => self.streamed(Speaker::Agent, delta),
            AgentEvent::Reasoning(delta) => self.streamed(Speaker::Thinking, delta),
            AgentEvent::Notice(text) => self.said(Speaker::Notice, text),
            AgentEvent::StepStarted { .. } => return None,
            AgentEvent::ToolStarted { id, summary } => {
                let call = ToolCall::running(id, summary, now_ms());
                Change::Added(self.new_item(Content::Tool(call)))
            },
            AgentEvent::ToolProgress { id, status } => return self.tool_progress(&id, status),
            AgentEvent::ToolFinished { id, summary, result } => self.tool_finished(&id, summary, &result),
        })
    }

    #[inline]
    /// Applies a change a Session sent, keeping the transcript in step.
    pub fn apply(&mut self, change: &Change) {
        match *change {
            Change::Added(ref item) => {
                self.next_id = self.next_id.max(item.id.saturating_add(1));
                self.items.push(item.clone());
            },
            Change::Appended { id, revision, ref text } => {
                if let Some(item) = self.find_mut(id)
                    && let Content::Message(ref mut message) = item.content
                {
                    message.text.push_str(text);
                    item.revision = revision;
                }
            },
            Change::Replaced(ref replacement) => {
                if let Some(item) = self.find_mut(replacement.id) {
                    *item = replacement.clone();
                }
            },
            Change::Cleared => self.items.clear(),
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
    fn replace_card(item: &Item, change: impl FnOnce(&mut ToolCall)) -> Change {
        let mut call = match item.content {
            Content::Tool(ref call) => call.clone(),
            Content::Message(_) => unreachable!("only Tool cards are looked up by call id"),
        };
        change(&mut call);
        Change::Replaced(Item { id: item.id, revision: item.revision.saturating_add(1), content: Content::Tool(call) })
    }

    /// The running card for call `call_id`, if there is one.
    fn running_card(&self, call_id: &str) -> Option<&Item> {
        self.items
            .iter()
            .rev()
            .find(|item| matches!(&item.content, Content::Tool(call) if call.call_id == call_id && call.is_running()))
    }

    #[inline]
    pub fn said<IntoString>(&self, speaker: Speaker, text: IntoString) -> Change
    where
        IntoString: Into<String>,
    {
        Change::Added(self.new_item(Content::Message(Message { speaker, text: text.into() })))
    }

    fn streamed(&self, speaker: Speaker, delta: String) -> Change {
        match self.items.last() {
            Some(item) if matches!(&item.content, Content::Message(m) if m.speaker == speaker) => {
                Change::Appended { id: item.id, revision: item.revision.saturating_add(1), text: delta }
            },
            _ => self.said(speaker, delta),
        }
    }

    #[expect(clippy::option_if_let_else, reason = "both branches consume `finish`")]
    fn tool_finished(&self, call_id: &str, summary: String, result: &ToolResult) -> Change {
        let now = now_ms();
        let finish = |call: &mut ToolCall| call.finish(now, result.is_error, &result.content);
        if let Some(item) = self.running_card(call_id) {
            Self::replace_card(item, finish)
        } else {
            let mut call = ToolCall::running(call_id, summary, now);
            finish(&mut call);
            Change::Added(self.new_item(Content::Tool(call)))
        }
    }

    fn tool_progress(&self, call_id: &str, status: String) -> Option<Change> {
        let item = self.running_card(call_id)?;
        Some(Self::replace_card(item, |call| call.progress = Some(status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tool_call::ToolStatus;

    fn record(transcript: &mut Transcript, event: AgentEvent) {
        let change = transcript.agent_event(event).expect("the event changes the transcript");
        transcript.apply(&change);
    }

    fn say(transcript: &mut Transcript, speaker: Speaker, text: &str) {
        let change = transcript.said(speaker, text);
        transcript.apply(&change);
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

    fn text(delta: &str) -> AgentEvent {
        AgentEvent::Text(delta.to_owned())
    }

    /// A started call. Its id is its summary, unless the test says otherwise.
    fn started(summary: &str) -> AgentEvent {
        started_with_id(summary, summary)
    }

    fn started_with_id(id: &str, summary: &str) -> AgentEvent {
        AgentEvent::ToolStarted { id: id.to_owned(), summary: summary.to_owned() }
    }

    fn finished(summary: &str, result: ToolResult) -> AgentEvent {
        finished_with_id(summary, summary, result)
    }

    fn finished_with_id(id: &str, summary: &str, result: ToolResult) -> AgentEvent {
        AgentEvent::ToolFinished { id: id.to_owned(), summary: summary.to_owned(), result }
    }

    #[test]
    fn streamed_text_joins_the_current_agent_reply() {
        let mut transcript = Transcript::default();
        say(&mut transcript, Speaker::You, "q");
        record(&mut transcript, text("Hel"));
        record(&mut transcript, text("lo"));
        assert_eq!(messages(&transcript), [(Speaker::You, "q".into()), (Speaker::Agent, "Hello".into())]);
    }

    #[test]
    fn thinking_streams_into_its_own_message_before_the_reply() {
        let mut transcript = Transcript::default();
        record(&mut transcript, AgentEvent::Reasoning("Let me ".into()));
        record(&mut transcript, AgentEvent::Reasoning("look.".into()));
        record(&mut transcript, text("Found it"));
        assert_eq!(
            messages(&transcript),
            [(Speaker::Thinking, "Let me look.".into()), (Speaker::Agent, "Found it".into())]
        );
    }

    #[test]
    fn a_reply_after_a_tool_call_starts_a_new_message() {
        let mut transcript = Transcript::default();
        record(&mut transcript, text("before"));
        record(&mut transcript, started("call"));
        record(&mut transcript, text("after"));
        assert_eq!(messages(&transcript), [(Speaker::Agent, "before".into()), (Speaker::Agent, "after".into())]);
    }

    #[test]
    fn a_tool_call_appears_when_it_starts() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("read_file(Cargo.toml)"));
        let calls = calls(&transcript);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].summary, "read_file(Cargo.toml)");
        assert!(calls[0].is_running());
    }

    #[test]
    fn a_tool_result_lands_on_the_card_that_started_it() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("read_file(a)"));
        record(&mut transcript, finished("read_file(a)", ToolResult::ok("file contents")));
        let calls = calls(&transcript);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].status, ToolStatus::Done);
        assert_eq!(calls[0].preview(), ["file contents"]);
    }

    #[test]
    fn a_running_tool_is_found_only_in_the_current_turn() {
        let mut transcript = Transcript::default();
        assert!(!transcript.has_running_tool());
        record(&mut transcript, started("bash(sleep)"));
        assert!(transcript.has_running_tool());
        record(&mut transcript, finished("bash(sleep)", ToolResult::ok("")));
        assert!(!transcript.has_running_tool());

        // A card left running by an earlier turn does not keep the screen redrawing.
        record(&mut transcript, started("bash(old)"));
        say(&mut transcript, Speaker::You, "next");
        assert!(!transcript.has_running_tool());
    }

    #[test]
    fn a_failed_tool_call_is_marked_failed() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started("read_file(../x)"));
        record(&mut transcript, finished("read_file(../x)", ToolResult::error("denied")));
        assert_eq!(calls(&transcript)[0].status, ToolStatus::Failed);
    }

    #[test]
    fn a_result_without_a_started_call_still_shows_a_card() {
        let mut transcript = Transcript::default();
        record(&mut transcript, finished("read_file(a)", ToolResult::ok("x")));
        assert_eq!(calls(&transcript)[0].status, ToolStatus::Done);
    }

    #[test]
    fn a_change_appends_or_replaces_the_item_it_names_and_bumps_its_revision() {
        let mut transcript = Transcript::default();
        record(&mut transcript, text("a"));
        let (id, before) = (transcript.items()[0].id, transcript.items()[0].revision);
        record(&mut transcript, text("b"));
        assert_eq!(transcript.items()[0].id, id);
        assert!(transcript.items()[0].revision > before);
    }

    #[test]
    fn a_client_that_applies_the_changes_ends_with_the_same_transcript() {
        let mut session_side = Transcript::default();
        let mut client = Transcript::default();
        let mut changes = Vec::new();
        for event in [text("hi "), text("there"), started("read_file(a)")] {
            let change = session_side.agent_event(event).unwrap();
            session_side.apply(&change);
            changes.push(change);
        }
        let change = session_side.agent_event(finished("read_file(a)", ToolResult::ok("x"))).unwrap();
        session_side.apply(&change);
        changes.push(change);
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
        transcript.apply(&Change::Cleared);
        say(&mut transcript, Speaker::You, "b");
        assert_ne!(transcript.items()[0].id, first);
    }

    #[test]
    fn results_and_progress_match_their_card_by_call_id_even_with_the_same_summary() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started_with_id("a", "read_file(x)"));
        record(&mut transcript, started_with_id("b", "read_file(x)"));
        record(&mut transcript, AgentEvent::ToolProgress { id: "a".into(), status: "reading".into() });
        record(&mut transcript, finished_with_id("b", "read_file(x)", ToolResult::ok("second")));

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
    fn a_step_starting_changes_nothing_on_the_transcript() {
        let transcript = Transcript::default();
        assert!(transcript.agent_event(AgentEvent::StepStarted { step: 1 }).is_none());
    }

    #[test]
    fn progress_for_a_finished_or_unknown_card_changes_nothing() {
        let mut transcript = Transcript::default();
        record(&mut transcript, started_with_id("a", "bash(x)"));
        record(&mut transcript, finished_with_id("a", "bash(x)", ToolResult::ok("")));
        let late = AgentEvent::ToolProgress { id: "a".into(), status: "too late".into() };
        assert!(transcript.agent_event(late).is_none());
        let unknown = AgentEvent::ToolProgress { id: "zz".into(), status: "who?".into() };
        assert!(transcript.agent_event(unknown).is_none());
    }
}
