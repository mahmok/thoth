/// Summarising old turns.
mod compaction;
/// Dropping bulky old tool output.
mod elision;

use crate::agent::context::Elision;
use crate::agent::tools::tool::Outcome as ToolResult;
use crate::agent::types::AgentMessage;
use crate::llm::client::chat::{Message, Reasoning, Role, ToolCall};

#[derive(Default, Debug)]
/// The messages sent to the model.
pub struct Conversation {
    messages: Vec<Message>,
}

#[expect(clippy::multiple_inherent_impl, reason = "its impls are split by topic across files")]
impl Conversation {
    #[inline]
    /// Starts a Turn with `prompt` as the user message.
    pub fn begin_turn<IntoString>(&mut self, prompt: IntoString) -> Turn<'_>
    where
        IntoString: Into<String>,
    {
        let start = self.messages.len();
        self.messages.push(Message::user(prompt));
        Turn {
            conversation: self,
            start,
            protected: start,
            has_tool_steps: false,
            has_changes: false,
            committed: false,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.messages.retain(|m| m.role == Role::System);
    }

    #[inline]
    pub fn estimated_tokens(&self) -> usize {
        self.messages.iter().map(message_tokens).sum()
    }

    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    #[inline]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self { messages: Vec::new() }
    }

    #[inline]
    /// Sets the system prompt, replacing the one already there.
    pub fn set_system_prompt<IntoString>(&mut self, prompt: IntoString)
    where
        IntoString: Into<String>,
    {
        let message = Message::system(prompt);
        match self.messages.first_mut() {
            Some(first) if first.role == Role::System => *first = message,
            _ => self.messages.insert(0, message),
        }
    }
}

#[derive(Debug)]
/// One user prompt and everything the Agent does for it.
#[expect(clippy::struct_excessive_bools, reason = "the flags are independent facts about one turn")]
pub struct Turn<'conversation> {
    committed: bool,
    conversation: &'conversation mut Conversation,
    /// Whether a tool step may have changed something outside the Conversation, such as a file.
    has_changes: bool,
    has_tool_steps: bool,
    /// Where the part of the Conversation the model has not read yet begins: the prompt, or
    /// the latest tool step.
    protected: usize,
    start: usize,
}

impl Turn<'_> {
    #[inline]
    /// Records the final reply. A Turn with no text is kept only if Tools ran.
    pub fn complete<IntoAgentmessage>(&mut self, message: IntoAgentmessage)
    where
        IntoAgentmessage: Into<AgentMessage>,
    {
        let reply = message.into();
        let Some(text) = reply.text.filter(|text| !text.is_empty()) else {
            self.committed = self.has_tool_steps;
            return;
        };
        self.conversation.messages.push(Message::assistant(Some(text), Vec::new()).with_reasoning(reply.reasoning));
        self.committed = true;
    }

    /// Elides what came before the part the model has not read yet. Returns whether anything
    /// was elided.
    #[inline]
    pub fn elide(&mut self, rules: &Elision) -> bool {
        self.conversation.elide_before(self.protected, rules)
    }

    #[must_use]
    #[inline]
    pub fn estimated_tokens(&self) -> usize {
        self.conversation.estimated_tokens()
    }

    /// Keeps the tool steps taken so far, but only when they may have changed something. A
    /// failed Turn that only read things is dropped as if it never started; one that edited
    /// files is kept, so the model still knows about the edits on its next Turn.
    #[inline]
    pub const fn keep_if_changed(&mut self) {
        self.committed = self.has_tool_steps && self.has_changes;
    }

    /// Keeps the tool steps taken so far. A turn with none is dropped, as if it never started.
    #[inline]
    pub const fn keep_partial(&mut self) {
        self.committed = self.has_tool_steps;
    }

    #[must_use]
    #[inline]
    pub fn messages(&self) -> &[Message] {
        &self.conversation.messages
    }

    /// Says that the tool steps so far may have changed something outside the Conversation.
    #[inline]
    pub const fn note_changes(&mut self) {
        self.has_changes = true;
    }

    #[inline]
    /// Records the model's Tool calls and their results.
    pub fn record_tool_step<IntoAgentmessage>(
        &mut self,
        message: IntoAgentmessage,
        executed: Vec<(ToolCall, ToolResult)>,
    ) where
        IntoAgentmessage: Into<AgentMessage>,
    {
        let agent_message = message.into();
        self.protected = self.conversation.messages.len();
        let (calls, results): (Vec<ToolCall>, Vec<ToolResult>) = executed.into_iter().unzip();
        self.conversation
            .messages
            .push(Message::assistant(agent_message.text, calls.clone()).with_reasoning(agent_message.reasoning));
        for (call, result) in calls.into_iter().zip(results) {
            self.conversation.messages.push(Message::tool(call.id, result.content).with_error(result.is_error));
        }
        self.has_tool_steps = true;
    }
}

impl Drop for Turn<'_> {
    #[inline]
    fn drop(&mut self) {
        if !self.committed {
            self.conversation.messages.truncate(self.start);
        }
    }
}

const fn tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

fn message_tokens(message: &Message) -> usize {
    let content = message.content.as_deref().map_or(0, tokens);
    let calls: usize = message.tool_calls.iter().map(|c| tokens(&c.function.arguments)).sum();
    let reasoning: usize = message
        .reasoning
        .iter()
        .map(|&(Reasoning::Responses(ref v) | Reasoning::Anthropic(ref v))| tokens(&v.to_string()))
        .sum();
    content.saturating_add(calls).saturating_add(reasoning)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::client::chat::{FunctionCall, Reasoning, ToolCallKind};
    use crate::test::assert_empty;

    fn roles(conversation: &Conversation) -> Vec<Role> {
        conversation.messages().iter().map(|m| m.role).collect()
    }

    pub(super) fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall { name: "t".into(), arguments: "{}".into() },
        }
    }

    #[test]
    fn system_prompt_is_inserted_first() {
        let mut conversation = Conversation::new();
        conversation.begin_turn("hi").complete("hello");
        conversation.set_system_prompt("be brief");
        assert_eq!(roles(&conversation), [Role::System, Role::User, Role::Assistant]);
        assert_eq!(conversation.messages()[0].content.as_deref(), Some("be brief"));
    }

    #[test]
    fn setting_system_prompt_again_replaces_it() {
        let mut conversation = Conversation::new();
        conversation.set_system_prompt("one");
        conversation.set_system_prompt("two");
        assert_eq!(roles(&conversation), [Role::System]);
        assert_eq!(conversation.messages()[0].content.as_deref(), Some("two"));
    }

    #[test]
    fn a_completed_turn_is_kept() {
        let mut conversation = Conversation::new();
        conversation.begin_turn("hi").complete("hello");
        assert_eq!(roles(&conversation), [Role::User, Role::Assistant]);
    }

    #[test]
    fn a_dropped_turn_leaves_no_trace() {
        let mut conversation = Conversation::new();
        conversation.set_system_prompt("sys");
        conversation.begin_turn("first").complete("ok");
        {
            let mut turn = conversation.begin_turn("second");
            turn.record_tool_step(AgentMessage::default(), vec![(call("c1"), ToolResult::ok("out"))]);
            assert_eq!(turn.messages().len(), 1 + 2 + 3, "turn sees its own progress");
        }
        assert_eq!(roles(&conversation), [Role::System, Role::User, Role::Assistant]);
    }

    #[test]
    fn a_failed_turn_is_kept_only_when_its_steps_changed_something() {
        let mut conversation = Conversation::new();
        {
            let mut turn = conversation.begin_turn("read");
            turn.record_tool_step(AgentMessage::default(), vec![(call("a"), ToolResult::ok("out"))]);
            turn.keep_if_changed();
        }
        assert!(conversation.messages().is_empty());
        {
            let mut turn = conversation.begin_turn("edit");
            turn.record_tool_step(AgentMessage::default(), vec![(call("b"), ToolResult::ok("out"))]);
            turn.note_changes();
            turn.keep_if_changed();
        }
        assert_eq!(roles(&conversation), [Role::User, Role::Assistant, Role::Tool]);
    }

    #[test]
    fn reasoning_is_recorded_with_the_message_it_belongs_to() {
        let thought = || vec![Reasoning::Responses(serde_json::json!({ "type": "reasoning" }))];
        let mut conversation = Conversation::new();
        let mut turn = conversation.begin_turn("go");
        turn.record_tool_step(AgentMessage::new(None, thought()), vec![(call("a"), ToolResult::ok("ra"))]);
        turn.complete(AgentMessage::new(Some("done".into()), Vec::new()));
        drop(turn);

        let messages = conversation.messages();
        assert!(messages.len() >= 4);
        assert_eq!(messages[1].reasoning, thought());
        assert_empty(&messages[3].reasoning);
    }

    #[test]
    fn an_empty_reply_discards_the_turn() {
        let mut conversation = Conversation::new();
        conversation.begin_turn("hi").complete("");
        assert!(conversation.messages().is_empty());
    }

    #[test]
    fn tool_steps_pair_every_call_with_its_result() {
        let mut conversation = Conversation::new();
        let mut turn = conversation.begin_turn("go");
        turn.record_tool_step("looking", vec![(call("a"), ToolResult::ok("ra")), (call("b"), ToolResult::ok("rb"))]);
        turn.complete("done");
        drop(turn);

        let messages = conversation.messages();
        assert_eq!(roles(&conversation), [Role::User, Role::Assistant, Role::Tool, Role::Tool, Role::Assistant]);
        assert!(messages.len() >= 4);
        assert_eq!(messages[1].tool_calls.len(), 2);
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("a"));
        assert_eq!(messages[2].content.as_deref(), Some("ra"));
        assert_eq!(messages[3].tool_call_id.as_deref(), Some("b"));
    }
}
