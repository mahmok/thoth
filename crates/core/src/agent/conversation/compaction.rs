use super::elision::elide_messages;
use super::{Conversation, message_tokens};
use crate::agent::context::Elision;
use crate::agent::sub_agents::SubAgentInfo;
use crate::agent::tools::tool::Outcome as ToolResult;
use crate::agent::{Agent, COMPACTION_CALL_ID, Event as AgentEvent, Failure as AgentError};
use crate::llm::client::any::Client as AnyClient;
use crate::llm::client::chat::{Message, Role, Usage};
use crate::llm::client::error::Failure as ClientError;
use crate::llm::client::{CreateCompletionRequest, Llm as _, StreamedCompletion, ToolChoice};
use core::fmt::Write as _;

/// The prefix of a summary message.
const SUMMARY: &str = "[Summary of the earlier conversation]\n";

// TODO: Make this configurable
/// The prompt that asks for a summary.
const INSTRUCTION: &str = "Summarize the conversation so far, including any earlier summary, so \
that you can carry on from the summary alone. Keep the user's goals and constraints, decisions \
and their reasons, files read or changed and their current state, results that still matter, \
open problems and next steps. Be concise and specific. Reply with the summary only; do not call tools.";

impl Conversation {
    fn compaction_cut(&self, keep_recent: usize) -> Option<usize> {
        let start = self.turns_start();
        let (mut kept, mut cut) = (0usize, None);
        for (index, message) in self.messages.iter().enumerate().skip(start).rev() {
            kept = kept.saturating_add(message_tokens(message));
            if message.role == Role::User {
                if cut.is_some() && kept > keep_recent {
                    break;
                }
                cut = Some(index);
            }
        }
        cut.filter(|&candidate| candidate > start)
    }

    fn replace_with_summary(&mut self, cut: usize, summary: &str) {
        let replacement = [
            Message::user(format!("{SUMMARY}{summary}")),
            Message::assistant(Some("Understood. I'll continue from this summary.".into()), Vec::new()),
        ];
        drop(self.messages.splice(self.system_end()..cut, replacement));
    }

    fn system_end(&self) -> usize {
        usize::from(self.messages.first().is_some_and(|m| m.role == Role::System))
    }

    fn turns_start(&self) -> usize {
        let start = self.system_end();
        let has_summary = self.messages.get(start).and_then(|m| m.content.as_deref());
        start.saturating_add(usize::from(has_summary.is_some_and(|c| c.starts_with(SUMMARY))).saturating_mul(2))
    }
}

impl Agent {
    /// Requests a summary, eliding everything first if the context overflows.
    #[expect(clippy::too_many_arguments, reason = "the arguments are separate pieces of one call")]
    async fn ask_for_summary(
        &self,
        client: &AnyClient,
        conversation: &mut Conversation,
        cut: usize,
        instruction: &str,
        choice: ToolChoice,
    ) -> Result<StreamedCompletion, ClientError> {
        let first = {
            // A Turn that is never committed, so the instruction leaves no trace.
            let turn = conversation.begin_turn(instruction);
            self.request_summary(client, turn.messages(), choice).await
        };
        match first {
            Err(e) if e.is_context_overflow() => {
                let mut messages = conversation.messages.clone();
                if let Some(head) = messages.get_mut(..cut) {
                    let _elided = elide_messages(head, &Elision::EVERYTHING);
                }
                messages.push(Message::user(instruction));
                self.request_summary(client, &messages, choice).await
            },
            other => other,
        }
    }

    /// Replaces older turns with a summary, keeping `keep_recent` tokens, and reports it as a card.
    pub(in crate::agent) async fn compact_conversation<F>(
        &self,
        conversation: &mut Conversation,
        keep_recent: usize,
        focus: Option<&str>,
        on_event: &mut F,
    ) -> Result<Option<Usage>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let cut = conversation.compaction_cut(keep_recent).ok_or(AgentError::NothingToCompact)?;
        let turns = conversation
            .messages
            .get(conversation.turns_start()..cut)
            .unwrap_or_default()
            .iter()
            .filter(|m| m.role == Role::User)
            .count();
        let card = format!("compact {turns} turns into a summary");
        on_event(AgentEvent::ToolStarted { id: COMPACTION_CALL_ID.into(), summary: card.clone() });
        let result = self.summarize(conversation, cut, focus).await;
        let shown = match result {
            Ok((ref summary, _)) => ToolResult::ok(summary.clone()),
            Err(ref e) => ToolResult::error(e.to_string()),
        };
        on_event(AgentEvent::ToolFinished { id: COMPACTION_CALL_ID.into(), summary: card, result: shown });
        let (summary, usage) = result?;
        conversation.replace_with_summary(cut, &summary);
        Ok(usage)
    }

    async fn request_summary(
        &self,
        client: &AnyClient,
        messages: &[Message],
        tool_choice: ToolChoice,
    ) -> Result<StreamedCompletion, ClientError> {
        let request = CreateCompletionRequest {
            messages,
            tools: self.tools.definitions(),
            thinking: self.thinking(),
            tool_choice,
        };
        client.create_completion_stream(request, |_| {}).await
    }

    /// Gets a summary of the Conversation up to `cut`, asking again without Tools if the model calls one.
    async fn summarize(
        &self,
        conversation: &mut Conversation,
        cut: usize,
        focus: Option<&str>,
    ) -> Result<(String, Option<Usage>), AgentError> {
        let client = self.client()?;
        let instruction = instruction(focus, &self.sub_agents().infos());
        let mut completion = self
            .ask_for_summary(&client, conversation, cut, &instruction, ToolChoice::Auto)
            .await
            .map_err(AgentError::Client)?;
        // A model that calls a tool instead of summarising is asked again, with tools forbidden.
        // Only then: changing the choice makes the provider drop its cache of the Conversation,
        // and the first request is the one that reads that cache.
        if completion.text.trim().is_empty() && !completion.tool_calls.is_empty() {
            completion = self
                .ask_for_summary(&client, conversation, cut, &instruction, ToolChoice::None)
                .await
                .map_err(AgentError::Client)?;
        }
        if completion.cut_off.is_some() {
            return Err(AgentError::Compaction("the summary was cut off"));
        }
        match completion.text.trim() {
            "" => Err(AgentError::Compaction("the model returned an empty summary")),
            text => Ok((text.to_owned(), completion.usage)),
        }
    }
}

/// The instruction for one Summary: the standing one, the Sub-agents to keep the ids of, and what
/// to focus on.
fn instruction(focus: Option<&str>, sub_agents: &[SubAgentInfo]) -> String {
    let mut text = INSTRUCTION.to_owned();
    if !sub_agents.is_empty() {
        let listed: Vec<String> = sub_agents.iter().map(|sub| format!("{} ({})", sub.id, sub.persona)).collect();
        let _written = write!(
            text,
            " Sub-agents are still running, and can be messaged by their ids: {}. Keep each id and \
             its Persona in the summary, so they can still be reached.",
            listed.join(", ")
        );
    }
    if let Some(topic) = focus {
        let _written = write!(text, "\nPay most attention to: {topic}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::{Budget as ContextBudget, Compaction, Context as AgentContext};
    use crate::agent::tests::{build, text};
    use crate::llm::client::StreamedCompletion;
    use crate::llm::client::error::Failure as ClientError;
    use crate::llm::client::fake;
    use crate::llm::client::fake::Client as FakeClient;
    use std::path::Path;

    /// Compacts once the Conversation is over `budget` tokens, keeping only the latest Turn.
    fn agent(budget: usize, script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
        let (agent, fake) = build(script);
        let context_budget =
            ContextBudget { elision: None, compaction: Some(Compaction { budget, limit_share: 1.0, keep_recent: 0 }) };
        (agent.with_context(&AgentContext::new(Path::new("")).with_budget(context_budget)), fake)
    }

    async fn contents(agent: &Agent) -> Vec<(Role, String)> {
        let conversation = agent.conversation.lock().await;
        conversation.messages().iter().map(|m| (m.role, m.content.clone().unwrap_or_default())).collect()
    }

    async fn compact(agent: &Agent) -> (Result<Option<Usage>, AgentError>, Vec<AgentEvent>) {
        let mut events = Vec::new();
        let result = agent.compact(None, |e| events.push(e)).await;
        (result, events)
    }

    #[tokio::test]
    async fn older_turns_become_one_summary_and_the_latest_turn_is_kept() {
        let (agent, fake) = agent(usize::MAX, vec![text("a"), text("b"), text("c"), text("the gist")]);
        for prompt in ["1", "2", "3"] {
            agent.respond(prompt.into(), |_| {}).await.unwrap();
        }
        let (result, events) = compact(&agent).await;
        result.unwrap();

        let all = contents(&agent).await;
        let roles: Vec<Role> = all.iter().map(|&(role, _)| role).collect();
        assert_eq!(roles, [Role::System, Role::User, Role::Assistant, Role::User, Role::Assistant]);
        assert_eq!(all[1].1, format!("{SUMMARY}the gist"));
        assert_eq!((all[3].1.as_str(), all[4].1.as_str()), ("3", "c"));
        assert_eq!(fake.requests().last().unwrap().0, 1 + 6 + 1, "the whole Conversation, then the instruction");
        assert!(matches!(&events[1], AgentEvent::ToolFinished { summary, result: shown, .. }
            if summary == "compact 2 turns into a summary" && shown.content == "the gist"));
    }

    #[tokio::test]
    async fn there_is_only_ever_one_summary() {
        let (agent, _) = agent(usize::MAX, vec![text("a"), text("b"), text("first"), text("c"), text("second")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        compact(&agent).await.0.unwrap();
        agent.respond("3".into(), |_| {}).await.unwrap();
        compact(&agent).await.0.unwrap();

        let all = contents(&agent).await;
        assert_eq!(all.len(), 5);
        assert_eq!(all[1].1, format!("{SUMMARY}second"));
        assert_eq!(all[3].1, "3");
    }

    #[tokio::test]
    async fn asked_for_it_keeps_only_the_latest_turn_however_short_the_conversation() {
        let (plain, _) = build(vec![text("a"), text("b"), text("c"), text("gist")]);
        let agent = plain.with_context(&AgentContext::new(Path::new("")));
        for prompt in ["1", "2", "3"] {
            agent.respond(prompt.into(), |_| {}).await.unwrap();
        }
        compact(&agent).await.0.unwrap();
        let all = contents(&agent).await;
        assert_eq!(all.len(), 5);
        assert_eq!(all[1].1, format!("{SUMMARY}gist"));
    }

    #[tokio::test]
    async fn with_only_the_latest_turn_there_is_nothing_to_compact_and_that_is_no_error() {
        let (agent, fake) = agent(usize::MAX, vec![text("a")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        let (result, events) = compact(&agent).await;
        assert!(matches!(result, Ok(None)));
        assert!(matches!(&*events, [AgentEvent::ToolFinished { result: shown, .. }]
            if !shown.is_error && shown.content.starts_with("Nothing to compact yet")));
        assert_eq!(fake.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_model_that_calls_a_tool_instead_of_summarising_is_asked_again_without_tools() {
        use crate::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind};
        let tool_call = Ok(StreamedCompletion {
            tool_calls: vec![ToolCall {
                id: "c".into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: "echo".into(), arguments: "{}".into() },
            }],
            ..Default::default()
        });
        let (agent, fake) = agent(usize::MAX, vec![text("a"), text("b"), tool_call, text("the gist")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();

        compact(&agent).await.0.unwrap();
        let choices = fake.tool_choices();
        assert_eq!(
            &choices[2..],
            [ToolChoice::Auto, ToolChoice::None],
            "the first request keeps the cache, the retry forbids tools"
        );
        let all = contents(&agent).await;
        assert_eq!(all[1].1, format!("{SUMMARY}the gist"));
    }

    #[tokio::test]
    async fn a_failed_or_empty_summary_changes_nothing() {
        let (agent, _) = agent(usize::MAX, vec![text("a"), text("b"), Err(fake::failure()), text("  ")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        let before = contents(&agent).await;

        let (result, events) = compact(&agent).await;
        assert!(matches!(result, Err(AgentError::Client(_))));
        assert!(matches!(&events[1], AgentEvent::ToolFinished { result: shown, .. } if shown.is_error));
        assert!(matches!(compact(&agent).await.0, Err(AgentError::Compaction(_))));
        assert_eq!(contents(&agent).await, before);
    }

    #[tokio::test]
    async fn a_cancelled_compaction_changes_nothing() {
        let (agent, _) = agent(usize::MAX, vec![text("a"), text("b"), text("never")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        let before = contents(&agent).await;

        let interrupt = |event| {
            if let AgentEvent::ToolStarted { .. } = event {
                agent.interrupt();
            }
        };
        let result = agent.compact(None, interrupt).await;
        assert!(matches!(result, Err(AgentError::Interrupted)));
        assert_eq!(contents(&agent).await, before);
    }

    #[tokio::test]
    async fn over_budget_a_turn_compacts_first_and_counts_the_summary_usage() {
        let mut summary = text("gist").unwrap();
        summary.usage = Some(Usage { prompt_tokens: 7, ..Default::default() });
        let (agent, _) = agent(1, vec![text("a"), text("b"), Ok(summary), text("c")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        let response = agent.respond("3".into(), |_| {}).await.unwrap();

        assert_eq!(response.usage.unwrap().prompt_tokens, 7);
        let all = contents(&agent).await;
        assert_eq!(all[1].1, format!("{SUMMARY}gist"));
        assert_eq!(all[3..].iter().map(|entry| entry.1.as_str()).collect::<Vec<_>>(), ["2", "b", "3", "c"]);
    }

    #[tokio::test]
    async fn the_tool_definitions_count_toward_the_budget() {
        // A budget the conversation alone stays under, but not with the tools it is sent with.
        let tools = build(vec![]).0.tools.estimated_tokens();
        assert!(tools > 0);
        let (agent, fake) = agent(tools, vec![text("a"), text("b"), text("gist"), text("c")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        agent.respond("3".into(), |_| {}).await.unwrap();

        assert_eq!(fake.requests().len(), 4, "a summary was asked for");
        assert_eq!(contents(&agent).await[1].1, format!("{SUMMARY}gist"));
    }

    #[tokio::test]
    async fn a_failed_automatic_compaction_does_not_stop_the_turn() {
        let (agent, _) = agent(1, vec![text("a"), text("b"), Err(fake::failure()), text("c")]);
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        let response = agent.respond("3".into(), |_| {}).await.unwrap();
        assert_eq!(response.message.text.as_deref(), Some("c"));
        assert_eq!(contents(&agent).await.len(), 1 + 6);
    }

    #[tokio::test]
    async fn turned_off_it_never_runs() {
        let (plain, fake) = build(vec![text("a"), text("b"), text("c")]);
        let budget = ContextBudget { elision: None, compaction: None };
        let agent = plain.with_context(&AgentContext::new(Path::new("")).with_budget(budget));
        for prompt in ["1", "2", "3"] {
            agent.respond(prompt.into(), |_| {}).await.unwrap();
        }
        assert_eq!(fake.requests().len(), 3);
        assert!(matches!(compact(&agent).await.0, Err(AgentError::Compaction("compaction is turned off"))));
    }

    #[test]
    fn the_instruction_names_running_sub_agents_so_their_ids_are_kept() {
        let plain = instruction(None, &[]);
        assert_eq!(plain, INSTRUCTION);

        let subs = [
            SubAgentInfo { id: "tester-1".into(), persona: "tester".into() },
            SubAgentInfo { id: "explorer-1".into(), persona: "explorer".into() },
        ];
        let with_subs = instruction(Some("the tests"), &subs);
        assert!(with_subs.contains("tester-1 (tester), explorer-1 (explorer)"), "{with_subs}");
        assert!(with_subs.contains("Keep each id"), "{with_subs}");
        assert!(with_subs.ends_with("Pay most attention to: the tests"), "the focus comes last: {with_subs}");
    }

    #[tokio::test]
    async fn compacting_keeps_the_sub_agents_running() {
        use crate::auth::store::Credentials as CredentialStore;
        use crate::persona::Persona;
        use alloc::sync::Arc;

        let (agent, _) = agent(usize::MAX, vec![text("a"), text("b"), text("gist")]);
        agent
            .sub_agents()
            .spawn(&Persona::new("tester", "Tester", ""), || Agent::new(Arc::new(CredentialStore::in_memory())))
            .unwrap();
        for prompt in ["1", "2"] {
            agent.respond(prompt.into(), |_| {}).await.unwrap();
        }
        compact(&agent).await.0.unwrap();

        let ids: Vec<String> = agent.sub_agents().infos().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["tester-1"]);
    }
}
