//! The Sub-agents a Root agent has started. They last as long as the Root agent's Conversation
//! (clearing it ends them), and are reached by a readable id such as `tester-1`.

use alloc::fmt;
use alloc::sync::Arc;
use core::error;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::SemaphorePermit;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::agent::delegation::Delegation;
use crate::agent::tools::tool::{Context as ToolContext, Outcome as ToolResult};
use crate::agent::{Agent, Event as AgentEvent, Failure as AgentError, Settings};
use crate::persona::Persona;
use crate::sync::MutexExt as _;
use crate::workspace::truncate;

/// The first line of a successful Sub-agent result, so the Root agent knows which id to message.
const ID_HEADER: &str = "agent_id: ";

/// The most of a Sub-agent's reply that goes into the Root agent's Conversation.
const MAX_REPLY_BYTES: usize = 40_000;

#[derive(Clone, Debug)]
/// A running Sub-agent.
pub struct SubAgent {
    pub agent: Arc<Agent>,
    pub id: String,
    pub persona: Persona,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// What the Root agent knows of a Sub-agent.
pub struct SubAgentInfo {
    /// The Sub-agent's id.
    pub id: String,
    /// The key of its Persona.
    pub persona: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Returned when the Sub-agent limit is reached.
pub struct LimitReached {
    pub limit: usize,
}

impl fmt::Display for LimitReached {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "this session already has {} sub-agents", self.limit)
    }
}

impl error::Error for LimitReached {
    #[inline]
    fn cause(&self) -> Option<&dyn error::Error> {
        None
    }

    #[inline]
    fn description(&self) -> &'static str {
        "the Sub-agent limit was reached"
    }

    #[inline]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

#[derive(Default, Debug)]
/// The Sub-agents of a Root agent.
struct State {
    /// The last number given to each Persona key. Kept across clears, so an id is never reused.
    last_number: HashMap<String, usize>,
    /// Live Sub-agents, in the order they were started.
    live: Vec<SubAgent>,
}

#[derive(Debug)]
/// The registry of a Root agent's Sub-agents.
pub struct SubAgents {
    limits: Delegation,
    /// One permit per Sub-agent Turn that may run at once.
    running: Semaphore,
    state: Mutex<State>,
}

impl SubAgents {
    /// Ends every Sub-agent. Their ids are not given out again.
    #[inline]
    pub fn clear(&self) {
        self.state.lock_unpoisoned().live.clear();
    }

    #[inline]
    pub fn get(&self, id: &str) -> Option<SubAgent> {
        self.state.lock_unpoisoned().live.iter().find(|sub| sub.id == id).cloned()
    }

    #[inline]
    /// What the Root agent knows of its live Sub-agents.
    pub fn infos(&self) -> Vec<SubAgentInfo> {
        self.state
            .lock_unpoisoned()
            .live
            .iter()
            .map(|sub| SubAgentInfo { id: sub.id.clone(), persona: sub.persona.key.clone() })
            .collect()
    }

    #[inline]
    pub const fn limits(&self) -> &Delegation {
        &self.limits
    }

    #[must_use]
    #[inline]
    pub fn new(limits: Delegation) -> Self {
        Self { running: Semaphore::new(limits.max_running), limits, state: Mutex::default() }
    }

    /// Runs one Turn of Sub-agent `id` on `task` and returns its reply as the Tool result, with
    /// the tokens it spent. The Turn ends with the calling Tool's Turn. A failure is an error
    /// result that names the Sub-agent; its Conversation keeps what its Turn rules keep, so the
    /// Root agent can message it again. While it runs, its steps and Tool calls show as progress
    /// on the calling Tool's card.
    #[inline]
    pub async fn run(&self, id: &str, task: String, cx: &ToolContext) -> ToolResult {
        let Some(sub) = self.get(id) else {
            return ToolResult::error(format!("there is no sub-agent named `{id}`"));
        };
        let Some(_permit) = wait_for_permit(&self.running, &cx.cancel).await else {
            return interrupted(id);
        };
        inherit_settings(&sub, &cx.settings);
        let mut step = 0;
        let on_event = |event| match event {
            AgentEvent::StepStarted { step: now } => {
                step = now;
                cx.report(format!("{id} · step {step}"));
            },
            AgentEvent::ToolStarted { summary, .. } => {
                cx.report(format!("{id} · step {step} · {summary}"));
            },
            AgentEvent::Text(_)
            | AgentEvent::Reasoning(_)
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::Notice(_)
            | AgentEvent::ToolFinished { .. } => {},
        };
        match sub.agent.respond_under(&cx.cancel, task, on_event).await {
            Ok(response) => {
                // The reply stays in the Root agent's Conversation, so a very long one is cut.
                let reply = truncate(response.message.text.unwrap_or_default(), MAX_REPLY_BYTES);
                let result = ToolResult::ok(format!("{ID_HEADER}{id}\n\n{reply}"));
                match response.usage {
                    Some(usage) => result.with_usage(usage),
                    None => result,
                }
            },
            Err(AgentError::Interrupted) => interrupted(id),
            Err(e) => ToolResult::error(format!("sub-agent {id} failed: {e}")),
        }
    }

    /// Starts a Sub-agent playing `persona` and returns its id. `build` is only called when the
    /// Session has room for one, and the Sub-agent gets the Delegation's step limit.
    #[inline]
    /// Starts a Sub-agent for `persona`, built by `build`.
    ///
    /// # Errors
    ///
    /// Fails if the Sub-agent limits are reached.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    pub fn spawn<Callback>(&self, persona: &Persona, build: Callback) -> Result<String, LimitReached>
    where
        Callback: FnOnce() -> Agent,
    {
        let mut state = self.state.lock_unpoisoned();
        if state.live.len() >= self.limits.max_per_session {
            return Err(LimitReached { limit: self.limits.max_per_session });
        }
        let number = {
            let last = state.last_number.entry(persona.key.clone()).or_insert(0);
            *last = last.saturating_add(1);
            *last
        };
        let id = format!("{}-{number}", persona.key);
        let agent = build().with_max_steps(self.limits.max_steps);
        state.live.push(SubAgent { id: id.clone(), persona: persona.clone(), agent: Arc::new(agent) });
        Ok(id)
    }
}

/// A permit to run a Turn, or `None` if the calling Turn is cancelled while waiting for one.
#[expect(clippy::integer_division_remainder_used, reason = "tokio::select! expands to a remainder")]
async fn wait_for_permit<'semaphore>(
    running: &'semaphore Semaphore,
    cancel: &CancellationToken,
) -> Option<SemaphorePermit<'semaphore>> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => None,
        permit = running.acquire() => permit.ok(),
    }
}

fn interrupted(id: &str) -> ToolResult {
    ToolResult::error(format!("sub-agent {id} was interrupted"))
}

/// A Sub-agent takes the Root agent's model and Thinking level, unless its Persona names its own.
/// The model is only set when it changes: setting it makes a new client, and a new client starts a
/// new prompt-cache key, which would throw away what the provider cached for the Sub-agent.
fn inherit_settings(sub: &SubAgent, root: &Settings) {
    if sub.persona.model.is_none()
        && let Some(model) = root.model.as_ref()
        && sub.agent.model().as_ref() != Some(model)
    {
        sub.agent.set_model(model.clone());
    }
    if sub.persona.thinking.is_none() {
        sub.agent.set_thinking(root.thinking);
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::*;
    use crate::agent::skills::SkillSet;
    use crate::auth::store::Credentials as CredentialStore;
    use crate::llm::client::StreamedCompletion;
    use crate::llm::client::any::Client as AnyClient;
    use crate::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind, Usage};
    use crate::llm::client::error::Failure as ClientError;
    use crate::llm::client::fake;
    use crate::llm::client::fake::Client as FakeClient;
    use crate::llm::provider::{Model, ModelSpec, Provider};
    use crate::llm::thinking::Thinking;
    use crate::test::assert_empty;
    use core::iter;
    use core::time::Duration;
    use tokio::sync::mpsc;
    use tokio::time;
    use tokio_util::sync::CancellationToken;

    fn spec(model: &str) -> ModelSpec {
        ModelSpec::new(Arc::new(Provider::ollama()), Model::new(model))
    }

    fn persona(key: &str) -> Persona {
        Persona::new(key, key, "")
    }

    fn agent() -> Agent {
        Agent::new(Arc::new(CredentialStore::in_memory()))
    }

    /// A Sub-agent whose model replies are `script`.
    fn scripted(script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
        let fake = FakeClient::new(script);
        let agent = agent().with_model(spec("sub")).with_client(AnyClient::Fake(fake.clone()));
        (agent, fake)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    fn reply(text: &str) -> Result<StreamedCompletion, ClientError> {
        Ok(StreamedCompletion { text: text.into(), ..Default::default() })
    }

    fn with_limits(max_per_session: usize) -> SubAgents {
        SubAgents::new(Delegation { max_per_session, ..Delegation::default() })
    }

    #[test]
    fn ids_count_up_for_each_persona() {
        let subs = with_limits(16);
        assert_eq!(subs.spawn(&persona("tester"), agent).unwrap(), "tester-1");
        assert_eq!(subs.spawn(&persona("explorer"), agent).unwrap(), "explorer-1");
        assert_eq!(subs.spawn(&persona("tester"), agent).unwrap(), "tester-2");
        let infos: Vec<String> = subs.infos().into_iter().map(|info| info.id).collect();
        assert_eq!(infos, ["tester-1", "explorer-1", "tester-2"]);
    }

    #[test]
    fn a_sub_agent_is_found_by_its_id_and_an_unknown_id_is_not() {
        let subs = with_limits(16);
        let id = subs.spawn(&persona("tester"), agent).unwrap();
        assert!(subs.get(&id).is_some());
        assert!(subs.get("tester-9").is_none());
    }

    #[test]
    fn the_session_limit_refuses_without_building_another_agent() {
        let subs = with_limits(2);
        subs.spawn(&persona("a"), agent).unwrap();
        subs.spawn(&persona("b"), agent).unwrap();
        let built = Cell::new(false);
        let refused = subs.spawn(&persona("c"), || {
            built.set(true);
            agent()
        });
        assert_eq!(refused, Err(LimitReached { limit: 2 }));
        assert!(!built.get());
        assert_eq!(subs.infos().len(), 2);
    }

    #[test]
    fn clearing_ends_them_and_frees_their_places_but_ids_keep_counting() {
        let subs = with_limits(1);
        let first = subs.spawn(&persona("tester"), agent).unwrap();
        subs.spawn(&persona("tester"), agent).unwrap_err();

        subs.clear();
        assert_empty(subs.infos());
        assert!(subs.get(&first).is_none());
        assert_eq!(subs.spawn(&persona("tester"), agent).unwrap(), "tester-2");
    }

    #[test]
    fn a_sub_agent_takes_the_root_settings_unless_its_persona_names_its_own() {
        let root = Settings { model: Some(spec("root")), thinking: Thinking::Low };
        let plain = SubAgent { id: "plain-1".into(), persona: persona("plain"), agent: Arc::new(agent()) };
        inherit_settings(&plain, &root);
        assert_eq!(plain.agent.model(), Some(spec("root")));
        assert_eq!(plain.agent.thinking(), Thinking::Low);

        let mut own_persona = persona("own").with_thinking(Thinking::High);
        own_persona.model = Some(Model::reference("ollama", "own"));
        // The builder gives the Agent its Persona's own settings; inheritance must leave them alone.
        let built = agent().with_model(spec("own"));
        built.set_thinking(Thinking::High);
        let own = SubAgent { id: "own-1".into(), persona: own_persona, agent: Arc::new(built) };
        inherit_settings(&own, &root);
        assert_eq!(own.agent.model(), Some(spec("own")));
        assert_eq!(own.agent.thinking(), Thinking::High);
    }

    #[tokio::test]
    async fn a_run_returns_the_reply_under_the_id_with_the_tokens_it_spent() {
        let subs = with_limits(16);
        let (sub, _) = scripted(vec![Ok(StreamedCompletion {
            text: "found it".into(),
            usage: Some(Usage { prompt_tokens: 4, completion_tokens: 2, total_tokens: 6, ..Default::default() }),
            ..Default::default()
        })]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();

        let result = subs.run(&id, "look".into(), &ToolContext::detached()).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "agent_id: tester-1\n\nfound it");
        assert_eq!(result.usage.map(|u| u.total_tokens), Some(6));
    }

    #[tokio::test]
    async fn running_an_unknown_sub_agent_is_an_error_result() {
        let result = with_limits(16).run("ghost-1", "hi".into(), &ToolContext::detached()).await;
        assert!(result.is_error);
        assert!(result.content.contains("ghost-1"), "{}", result.content);
    }

    #[tokio::test]
    async fn a_failed_run_names_the_sub_agent_and_the_next_message_carries_on() {
        let subs = with_limits(16);
        let (sub, _) = scripted(vec![Err(fake::failure()), reply("again")]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();

        let failed = subs.run(&id, "one".into(), &ToolContext::detached()).await;
        assert!(failed.is_error);
        assert!(failed.content.contains("tester-1"), "{}", failed.content);
        assert!(failed.content.contains("fake failure"), "{}", failed.content);

        let next = subs.run(&id, "two".into(), &ToolContext::detached()).await;
        assert!(!next.is_error);
        assert_eq!(next.content, "agent_id: tester-1\n\nagain");
    }

    #[tokio::test]
    async fn a_cancelled_root_turn_stops_the_sub_agent_before_it_asks_the_model() {
        let subs = with_limits(16);
        let (sub, fake) = scripted(vec![reply("never")]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let (progress, _) = mpsc::unbounded_channel();
        let cx = ToolContext::new("call", cancelled, Settings::default(), progress, SkillSet::default());

        let result = subs.run(&id, "go".into(), &cx).await;
        assert!(result.is_error);
        assert_empty(fake.requests());
    }

    #[tokio::test]
    async fn a_spawned_sub_agent_gets_the_delegation_step_limit() {
        let subs = SubAgents::new(Delegation { max_steps: 1, ..Delegation::default() });
        let unknown_tool = Ok(StreamedCompletion {
            tool_calls: vec![ToolCall {
                id: "c".into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: "nope".into(), arguments: "{}".into() },
            }],
            ..Default::default()
        });
        let (sub, _) = scripted(vec![unknown_tool, reply("never")]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();

        let result = subs.run(&id, "go".into(), &ToolContext::detached()).await;
        assert!(result.is_error);
        assert!(result.content.contains("stopped after 1 tool steps"), "{}", result.content);
    }

    /// A reply that calls a tool which does not exist, so the Turn takes one more step.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    fn calls_unknown_tool() -> Result<StreamedCompletion, ClientError> {
        Ok(StreamedCompletion {
            tool_calls: vec![ToolCall {
                id: "c".into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: "nope".into(), arguments: "{}".into() },
            }],
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn a_run_waits_for_a_free_slot_and_then_runs() {
        let subs = SubAgents::new(Delegation { max_running: 1, ..Delegation::default() });
        let (sub, _) = scripted(vec![reply("ok")]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();

        let held = subs.running.acquire().await.unwrap();
        let waiting =
            time::timeout(Duration::from_millis(50), subs.run(&id, "go".into(), &ToolContext::detached())).await;
        assert!(waiting.is_err(), "the only slot is taken, so the run waits");

        drop(held);
        let result = subs.run(&id, "go".into(), &ToolContext::detached()).await;
        assert_eq!(result.content, "agent_id: tester-1\n\nok");
    }

    #[tokio::test]
    async fn a_run_waiting_for_a_slot_gives_up_when_its_turn_is_cancelled() {
        let subs = SubAgents::new(Delegation { max_running: 1, ..Delegation::default() });
        let (sub, fake) = scripted(vec![reply("never")]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();
        let _held = subs.running.acquire().await.unwrap();

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let (progress, _) = mpsc::unbounded_channel();
        let cx = ToolContext::new("call", cancelled, Settings::default(), progress, SkillSet::default());
        let result = subs.run(&id, "go".into(), &cx).await;
        assert!(result.is_error && result.content.contains("interrupted"));
        assert_empty(fake.requests());
    }

    #[tokio::test]
    async fn a_sub_agents_steps_and_tool_calls_show_as_progress_on_the_calling_card() {
        let subs = with_limits(16);
        let (sub, _) = scripted(vec![calls_unknown_tool(), reply("done")]);
        let id = subs.spawn(&persona("tester"), move || sub).unwrap();
        let (progress, mut reports) = mpsc::unbounded_channel();
        let cx =
            ToolContext::new("call_1", CancellationToken::new(), Settings::default(), progress, SkillSet::default());

        subs.run(&id, "go".into(), &cx).await;

        let statuses: Vec<String> = iter::from_fn(|| reports.try_recv().ok())
            .map(|report| {
                assert_eq!(report.call_id, "call_1");
                report.status
            })
            .collect();
        assert_eq!(
            statuses,
            ["tester-1 \u{b7} step 1", "tester-1 \u{b7} step 1 \u{b7} nope (unknown tool)", "tester-1 \u{b7} step 2",]
        );
    }
}
