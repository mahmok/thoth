/// Prompt context: AGENTS.md files and the token budget.
pub mod context;
/// Runs an Agent on its own task and reports back.
pub mod controller;
/// The message history and how it shrinks.
pub mod conversation;
pub mod delegation;
/// Skills a prompt can pull in.
pub mod skills;
pub mod sub_agents;
/// Tools an Agent can call.
pub mod tools;
/// The messages and responses of an Agent.
pub mod types;

use crate::agent::context::{Budget as ContextBudget, Context as AgentContext, Elision};
use crate::agent::conversation::Turn;
use crate::agent::delegation::Delegation;
use crate::agent::skills::SkillSet;
use crate::agent::sub_agents::SubAgents;
use crate::agent::tools::tool::{Context as ToolContext, Outcome as ToolResult, ProgressReport};
use crate::agent::tools::tool_set::ToolSet;
use crate::agent::types::{AgentMessage, AgentResponse};
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::client::any::Client as AnyClient;
use crate::llm::client::chat::{ToolCall, Usage};
use crate::llm::client::error::Failure as ClientError;
use crate::llm::client::{CreateCompletionRequest, Delta, Llm as _, StreamedCompletion, ToolChoice};
use crate::llm::provider::ModelSpec;
use crate::llm::thinking::Thinking;
use crate::sync::{MutexExt as _, RwLockExt as _};
use crate::utils::numbers;
use alloc::fmt;
use alloc::sync::Arc;
use conversation::Conversation;
use core::error;
use core::fmt::Write as _;
use core::future::Future;
use futures_util::future::join_all;
use std::collections::HashSet;
use std::sync;
use std::sync::RwLock;
use tokio::select;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

/// How many model steps a Turn may take before it stops with a step-limit error.
pub const DEFAULT_MAX_STEPS: usize = 20;

/// How many steps before the limit the model starts being told that few are left.
const STEPS_LEFT_WARNING: usize = 3;

/// Added to a failed tool call that is identical to one that already failed in the Turn.
const REPEATED_FAILURE: &str = "\n[this exact call already failed earlier in this turn; if nothing has changed since, repeating it will fail again. Change the call, or read the file again first]";

/// The id of the card that shows a compaction. It is not a model tool call, so it has no id from
/// the model.
pub const COMPACTION_CALL_ID: &str = "compaction";

#[derive(Debug, Clone)]
/// Something an Agent reports while it works.
pub enum Event {
    /// Something the user should know about the turn that is not the model's own words.
    Notice(String),
    /// A fragment of the model's thinking, shown apart from the reply.
    Reasoning(String),
    /// A model call (a Step) is starting; steps are numbered from 1 within a Turn.
    StepStarted {
        /// The step number, from 1.
        step: usize,
    },
    /// A fragment of the reply text, as it streams in.
    Text(String),
    /// A tool call finished. The full result is included so a UI can show it.
    ToolFinished {
        /// The id of the call.
        id: String,
        /// A short description of the call.
        summary: String,
        /// What the Tool returned.
        result: ToolResult,
    },
    /// A running tool call reports a status line, such as a sub-agent's current step.
    ToolProgress {
        /// The id of the call.
        id: String,
        /// What the call is doing.
        status: String,
    },
    /// A tool call is starting. `id` is the id the model gave the call; `summary` describes it.
    ToolStarted {
        /// The id of the call.
        id: String,
        /// A short description of the call.
        summary: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// The model and thinking level of an Agent.
pub struct Settings {
    /// The model in use, if one is chosen.
    pub model: Option<ModelSpec>,
    /// The thinking level.
    pub thinking: Thinking,
}

#[derive(Debug)]
/// A change to apply to an Agent.
pub enum Change {
    /// Forget the conversation.
    ClearConversation,
    /// Stop the running turn.
    Interrupt,
    /// Switch to this model.
    SetModel(ModelSpec),
    /// Change the thinking level.
    SetThinking(Thinking),
}

#[derive(Debug)]
/// Why an Agent's turn failed.
pub enum Failure {
    /// The model request failed.
    Client(ClientError),
    /// Compaction could not run, with the reason.
    Compaction(&'static str),
    /// The user stopped the turn.
    Interrupted,
    /// No model is chosen.
    NoModel,
    /// There are too few turns to summarise.
    NothingToCompact,
    /// The turn used its steps without a final answer.
    StepLimit(usize),
}

impl fmt::Display for Failure {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoModel => f.write_str("No model selected. Run /refresh-models, then /model."),
            Self::Client(ref e) => e.fmt(f),
            Self::StepLimit(n) => {
                write!(
                    f,
                    "stopped after {n} tool steps without a final answer; the work so far is kept, so say \"continue\" to go on"
                )
            },
            Self::Interrupted => f.write_str("interrupted"),
            Self::NothingToCompact => f.write_str("nothing to compact yet"),
            Self::Compaction(reason) => f.write_str(reason),
        }
    }
}

impl error::Error for Failure {
    #[inline]
    fn cause(&self) -> Option<&dyn error::Error> {
        None
    }

    #[inline]
    fn description(&self) -> &'static str {
        "an Agent's turn failed"
    }

    #[inline]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

#[derive(Default, Debug)]
/// The cancellation tokens of running work.
struct Active {
    next: u64,
    tokens: Vec<(u64, CancellationToken)>,
}

/// Takes its token off the Agent's list when the work is over.
struct ActiveGuard<'agent> {
    agent: &'agent Agent,
    id: u64,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        let mut active = self.agent.active.lock_unpoisoned();
        active.tokens.retain(|&(id, _)| id != self.id);
    }
}
#[derive(Debug)]

/// A model and its client.
struct Model {
    client: Arc<AnyClient>,
    spec: ModelSpec,
}

#[derive(Debug)]
/// A model with a Conversation and Tools.
pub struct Agent {
    /// The tokens of the work in progress and of the work waiting for the Conversation, so that
    /// `interrupt` reaches all of it.
    active: sync::Mutex<Active>,
    budget: ContextBudget,
    /// How many real prompt tokens one estimated token turned out to be at the last request. The
    /// estimate (bytes / 4) is too low for code and JSON, so thresholds would be reached late.
    calibration: sync::Mutex<f32>,
    conversation: Mutex<Conversation>,
    current: RwLock<Option<Model>>,
    description: String,
    max_steps: usize,
    name: String,
    skills: SkillSet,
    store: Arc<CredentialStore>,
    sub_agents: Arc<SubAgents>,
    thinking: RwLock<Thinking>,
    tools: ToolSet,
}

#[expect(clippy::multiple_inherent_impl, reason = "its impls are split by topic across files")]
impl Agent {
    #[inline]
    /// Applies a change that was queued for the Agent.
    pub fn apply(&self, change: Change) {
        match change {
            Change::SetModel(spec) => self.set_model(spec),
            Change::SetThinking(thinking) => self.set_thinking(thinking),
            Change::ClearConversation => {
                let _cleared = self.clear_conversation();
            },
            Change::Interrupt => self.interrupt(),
        }
    }

    #[inline]
    /// Clears the Conversation and the Sub-agents. Returns `false` if a turn is running.
    pub fn clear_conversation(&self) -> bool {
        self.conversation.try_lock().map_or_else(
            |_| false,
            |mut conversation| {
                conversation.clear();
                self.sub_agents.clear();
                true
            },
        )
    }

    fn client(&self) -> Result<Arc<AnyClient>, Failure> {
        let current = self.current.read_unpoisoned();
        Ok(Arc::clone(&current.as_ref().ok_or(Failure::NoModel)?.client))
    }

    #[inline]
    /// Summarises older turns now, with an optional focus. Fails if compaction is off.
    ///
    /// # Errors
    ///
    /// Fails if compaction is off, there is nothing to compact or the summary request fails.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    pub async fn compact<F>(&self, focus: Option<String>, mut on_event: F) -> Result<Option<Usage>, Failure>
    where
        F: FnMut(Event) + Send,
    {
        if self.budget.compaction.is_none() {
            return Err(Failure::Compaction("compaction is turned off"));
        }
        let mut conversation = self.conversation.lock().await;
        let (cancelled, _active) = self.new_cancellation(&CancellationToken::new());
        let compaction = self.compact_conversation(&mut conversation, 0, focus.as_deref(), &mut on_event);
        match until(&cancelled, compaction).await {
            Err(Failure::NothingToCompact) => {
                on_event(Event::ToolFinished {
                    id: COMPACTION_CALL_ID.into(),
                    summary: "compact conversation".into(),
                    result: ToolResult::ok("Nothing to compact yet: only the latest turn is there."),
                });
                Ok(None)
            },
            other => other,
        }
    }

    #[inline]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[inline]
    /// Cancels everything the Agent is doing.
    pub fn interrupt(&self) {
        let active = self.active.lock_unpoisoned();
        for entry in &active.tokens {
            entry.1.cancel();
        }
    }

    /// Learns from a request: `sent` tokens were estimated, and the provider counted its own.
    fn learn(&self, sent: usize, usage: Option<&Usage>) {
        let Some(reported) = usage.filter(|candidate| candidate.prompt_tokens > 0) else {
            return;
        };
        if sent > 0 {
            let ratio = numbers::ratio(reported.prompt_tokens, sent);
            *self.calibration.lock_unpoisoned() = ratio.clamp(0.5, 3.0);
        }
    }

    #[inline]
    pub fn model(&self) -> Option<ModelSpec> {
        self.current.read_unpoisoned().as_ref().map(|current| current.spec.clone())
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    /// Creates an Agent with no model, no Tools and an empty Conversation.
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self {
            store,
            current: RwLock::new(None),
            tools: ToolSet::new(),
            conversation: Mutex::new(Conversation::new()),
            budget: ContextBudget::default(),
            max_steps: DEFAULT_MAX_STEPS,
            sub_agents: Arc::new(SubAgents::new(Delegation::default())),
            skills: SkillSet::default(),
            thinking: RwLock::new(Thinking::default()),
            active: sync::Mutex::default(),
            calibration: sync::Mutex::new(1.0),
            name: String::new(),
            description: String::new(),
        }
    }

    /// A token for the work about to start, child of `parent`, and the guard that keeps it
    /// reachable by `interrupt` until the work is done. Work that waits for the Conversation is
    /// registered too, so an interrupt stops the running Turn and the ones queued behind it.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    fn new_cancellation(&self, parent: &CancellationToken) -> (CancellationToken, ActiveGuard<'_>) {
        let token = parent.child_token();
        let mut active = self.active.lock_unpoisoned();
        active.next = active.next.saturating_add(1);
        let id = active.next;
        active.tokens.push((id, token.clone()));
        (token, ActiveGuard { agent: self, id })
    }

    #[inline]
    /// Runs a Turn for `prompt`, reporting events as they happen.
    ///
    /// # Errors
    ///
    /// Fails if no model is chosen, the model request fails, the turn is interrupted or it runs out of steps.
    pub async fn respond<F>(&self, prompt: String, on_event: F) -> Result<AgentResponse, Failure>
    where
        F: FnMut(Event) + Send,
    {
        self.respond_under(&CancellationToken::new(), prompt, on_event).await
    }

    /// Like [`respond`](Self::respond), but the Turn also ends when `parent` is cancelled. A
    /// Sub-agent's Turn runs this way, so cancelling the Root agent's Turn cancels it too.
    #[inline]
    /// Runs a Turn that a parent's cancellation also stops.
    ///
    /// # Errors
    ///
    /// Fails as `respond` does.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    pub async fn respond_under<F>(
        &self,
        parent: &CancellationToken,
        prompt: String,
        mut on_event: F,
    ) -> Result<AgentResponse, Failure>
    where
        F: FnMut(Event) + Send,
    {
        drop(self.client()?);
        // Registered before waiting for the Conversation, so an interrupt during the wait counts.
        let (cancelled, _active) = self.new_cancellation(parent);
        let mut conversation = self.conversation.lock().await;
        let usage = until(&cancelled, self.shrink(&mut conversation, &prompt, &mut on_event)).await?;
        let mut turn = conversation.begin_turn(prompt);
        let result = self.run_loop(&mut turn, usage, &cancelled, on_event).await;
        match result {
            Err(Failure::Interrupted) => turn.keep_partial(),
            // The files stay changed when the Turn fails, so the model must still know.
            Err(_) => turn.keep_if_changed(),
            Ok(_) => {},
        }
        result
    }

    /// Runs one Tool call with its context. A call whose reply was cut off mid-argument is not
    /// run: the model is told, and can try again with less.
    async fn run_call(&self, call: &ToolCall, cx: ToolContext, cut_off: bool) -> ToolResult {
        let (name, arguments) = (&call.function.name, &call.function.arguments);
        if cut_off && serde_json::from_str::<serde_json::Value>(arguments).is_err() {
            return ToolResult::error(
                "your reply was cut off while writing this tool call, so it was not run; try again with a smaller call",
            );
        }
        self.tools.run(name, arguments, &cx).await
    }

    /// Like [`run_call`](Self::run_call), but a cancel ends it with an error result instead of
    /// dropping the whole step. The step is then still recorded, so that what its other calls
    /// did is not lost.
    #[expect(clippy::integer_division_remainder_used, reason = "tokio::select! expands to a remainder")]
    async fn run_call_guarded(
        &self,
        call: &ToolCall,
        cx: ToolContext,
        cut_off: bool,
        cancel: &CancellationToken,
    ) -> ToolResult {
        select! {
            biased;
            () = cancel.cancelled() => ToolResult::error(
                "interrupted by the user before this call finished; it may or may not have taken effect",
            ),
            result = self.run_call(call, cx, cut_off) => result,
        }
    }

    /// Runs the model and Tool steps of a Turn until the model answers, up to the step limit.
    async fn run_loop<F>(
        &self,
        turn: &mut Turn<'_>,
        mut usage: Option<Usage>,
        cancel: &CancellationToken,
        mut on_event: F,
    ) -> Result<AgentResponse, Failure>
    where
        F: FnMut(Event) + Send,
    {
        // The calls that failed in this Turn, to notice the model repeating one.
        let mut failed: HashSet<(String, String)> = HashSet::new();
        for step in 1..=self.max_steps {
            // Checked here too, not only where the work awaits: a cancel that came during the
            // last Tool calls must not start another model request.
            if cancel.is_cancelled() {
                return Err(Failure::Interrupted);
            }
            on_event(Event::StepStarted { step });
            self.trim(turn);
            let sent = turn.estimated_tokens().saturating_add(self.tools.estimated_tokens());
            // The last step must answer in text, so the Turn ends with a reply that says what was
            // done and what is left, not with an error. A Turn of one step has no such choice.
            let tool_choice =
                if step == self.max_steps && self.max_steps > 1 { ToolChoice::None } else { ToolChoice::Auto };
            let completion = until(cancel, self.step(turn, tool_choice, &mut on_event)).await?;

            self.learn(sent, completion.usage.as_ref());
            usage = add_usage(usage, completion.usage);

            if completion.tool_calls.is_empty() {
                if let Some(reason) = completion.cut_off.as_ref() {
                    on_event(Event::Notice(format!("the reply was cut off ({reason}); say \"continue\" to go on")));
                }
                let message = AgentMessage::new(Some(completion.text), completion.reasoning);
                turn.complete(message.clone());
                return Ok(AgentResponse { message, usage });
            }

            let msg = AgentMessage::new((!completion.text.is_empty()).then_some(completion.text), completion.reasoning);
            let settings = self.settings();
            let changes = !completion
                .tool_calls
                .iter()
                .all(|call| self.tools.traits(&call.function.name, &call.function.arguments).read_only);
            let results = self
                .run_tools(&completion.tool_calls, completion.cut_off.is_some(), cancel, &settings, &mut on_event)
                .await;
            usage = results.iter().fold(usage, |total, result| add_usage(total, result.usage));
            let mut executed: Vec<(ToolCall, ToolResult)> = completion
                .tool_calls
                .into_iter()
                .zip(results)
                .map(|(call, mut result)| {
                    let key = (call.function.name.clone(), call.function.arguments.clone());
                    if result.is_error && !failed.insert(key) {
                        result.content.push_str(REPEATED_FAILURE);
                    }
                    (call, result)
                })
                .collect();
            let remaining = self.max_steps.saturating_sub(step);
            if (1..=STEPS_LEFT_WARNING).contains(&remaining)
                && let Some(last) = executed.last_mut()
            {
                let _written = write!(
                    last.1.content,
                    "\n[{remaining} model step{} left in this turn: stop exploring, finish the work and give the user a final answer]",
                    if remaining == 1 { "" } else { "s" }
                );
            }

            turn.record_tool_step(msg, executed);
            if changes {
                turn.note_changes();
            }
        }

        turn.keep_partial();
        Err(Failure::StepLimit(self.max_steps))
    }

    /// Runs the calls of one step and reports each as it starts, progresses and finishes. A step
    /// of read-only calls runs them at the same time; anything else runs them one by one, in order.
    #[expect(clippy::too_many_arguments, reason = "the arguments are separate pieces of one call")]
    async fn run_tools<F>(
        &self,
        calls: &[ToolCall],
        cut_off: bool,
        cancel: &CancellationToken,
        settings: &Settings,
        on_event: &mut F,
    ) -> Vec<ToolResult>
    where
        F: FnMut(Event) + Send,
    {
        let (progress, mut reports) = unbounded_channel();
        let context = |call: &ToolCall| {
            ToolContext::new(&call.id, cancel.clone(), settings.clone(), progress.clone(), self.skills.clone())
        };
        let summaries: Vec<String> =
            calls.iter().map(|call| self.tools.describe(&call.function.name, &call.function.arguments)).collect();
        let in_parallel = calls.len() > 1
            && calls.iter().all(|call| self.tools.traits(&call.function.name, &call.function.arguments).read_only);

        let mut results = Vec::with_capacity(calls.len());
        if in_parallel {
            for (call, summary) in calls.iter().zip(&summaries) {
                on_event(started(call, summary));
            }
            let work = join_all(calls.iter().map(|call| self.run_call_guarded(call, context(call), cut_off, cancel)));
            let finished = pump(work, &mut reports, on_event).await;
            for ((call, summary), result) in calls.iter().zip(&summaries).zip(finished) {
                on_event(finish(call, summary, &result));
                results.push(result);
            }
        } else {
            for (call, summary) in calls.iter().zip(&summaries) {
                on_event(started(call, summary));
                let work = self.run_call_guarded(call, context(call), cut_off, cancel);
                let result = pump(work, &mut reports, on_event).await;
                on_event(finish(call, summary, &result));
                results.push(result);
            }
        }
        results
    }

    /// An estimate of prompt tokens, corrected by what the provider last reported.
    fn scaled(&self, estimate: usize) -> usize {
        numbers::scaled(estimate, *self.calibration.lock_unpoisoned())
    }

    #[inline]
    /// Switches to `spec`, with a client of its own, and restarts token calibration.
    pub fn set_model(&self, spec: ModelSpec) {
        let client = Arc::new(AnyClient::for_model(&spec, &self.store));
        *self.current.write_unpoisoned() = Some(Model { client, spec });
        // Another model counts tokens its own way.
        *self.calibration.lock_unpoisoned() = 1.0;
    }

    /// Applies from the next model call on, including the rest of a turn in progress.
    #[inline]
    pub fn set_thinking(&self, thinking: Thinking) {
        *self.thinking.write_unpoisoned() = thinking;
    }

    #[inline]
    pub fn settings(&self) -> Settings {
        Settings { model: self.model(), thinking: self.thinking() }
    }

    /// Elides or compacts before a request when the next one would not fit the context.
    async fn shrink<F>(
        &self,
        conversation: &mut Conversation,
        prompt: &str,
        on_event: &mut F,
    ) -> Result<Option<Usage>, Failure>
    where
        F: FnMut(Event) + Send,
    {
        let limit = self.model().and_then(|model| model.context_limit);
        // The prompt counts: a large one, with files attached, is part of the next request.
        let incoming = prompt.len().div_ceil(4);
        let used = self.scaled(
            conversation.estimated_tokens().saturating_add(self.tools.estimated_tokens()).saturating_add(incoming),
        );
        if let Some(rules) = self.budget.elision.as_ref() {
            // Elision cuts the cache and makes the model read files again, so it waits until the
            // Conversation is half way to compaction. Without compaction there is no such
            // threshold, and it runs as before.
            let wait = self.budget.compaction.as_ref().is_some_and(|c| used <= c.threshold(limit).div_euclid(2));
            if !wait {
                conversation.elide(rules);
            }
        }
        let Some(rules) = self.budget.compaction.as_ref() else {
            return Ok(None);
        };
        let in_use = self.scaled(
            conversation.estimated_tokens().saturating_add(self.tools.estimated_tokens()).saturating_add(incoming),
        );
        if in_use <= rules.threshold(limit) {
            return Ok(None);
        }
        Ok(self.compact_conversation(conversation, rules.keep_recent, None, on_event).await.unwrap_or(None))
    }

    #[inline]
    pub const fn skills(&self) -> &SkillSet {
        &self.skills
    }

    /// Asks the model for one reply, recovering once from a context overflow by eliding.
    async fn step<F>(
        &self,
        turn: &mut Turn<'_>,
        tool_choice: ToolChoice,
        on_event: &mut F,
    ) -> Result<StreamedCompletion, Failure>
    where
        F: FnMut(Event) + Send,
    {
        let mut recovered = false;
        loop {
            let client = self.client()?;
            let result = client
                .create_completion_stream(
                    CreateCompletionRequest {
                        messages: turn.messages(),
                        tools: self.tools.definitions(),
                        thinking: self.thinking(),
                        tool_choice,
                    },
                    |delta| match delta {
                        Delta::Text(text) => on_event(Event::Text(text.to_owned())),
                        Delta::Reasoning(text) => on_event(Event::Reasoning(text.to_owned())),
                    },
                )
                .await;
            match result {
                Err(e) if !recovered && e.is_context_overflow() && turn.elide(&Elision::EVERYTHING) => {
                    recovered = true;
                    on_event(Event::Notice(
                        "the conversation was too long: dropped old tool output and retrying".into(),
                    ));
                },
                other => return other.map_err(Failure::Client),
            }
        }
    }

    #[inline]
    pub fn sub_agents(&self) -> &SubAgents {
        &self.sub_agents
    }

    #[inline]
    pub fn thinking(&self) -> Thinking {
        *self.thinking.read_unpoisoned()
    }

    #[inline]
    pub const fn tools(&self) -> &ToolSet {
        &self.tools
    }

    fn trim(&self, turn: &mut Turn<'_>) {
        let (Some(elision), Some(compaction)) = (self.budget.elision.as_ref(), self.budget.compaction.as_ref()) else {
            return;
        };
        let limit = self.model().and_then(|model| model.context_limit);
        let used = self.scaled(turn.estimated_tokens().saturating_add(self.tools.estimated_tokens()));
        if used > compaction.threshold(limit) {
            let _elided = turn.elide(elision);
        }
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    /// Replaces the client of the current model, for tests.
    #[must_use]
    pub fn with_client(self, client: AnyClient) -> Self {
        if let Some(current) = self.current.write_unpoisoned().as_mut() {
            current.client = Arc::new(client);
        }
        self
    }

    #[inline]
    /// Sets the system prompt and the context budget from `context`.
    #[must_use]
    pub fn with_context(mut self, context: &AgentContext) -> Self {
        self.conversation.get_mut().set_system_prompt(context.get_combined_system_prompt());
        self.budget = *context.budget();
        self
    }

    #[inline]
    #[must_use]
    pub fn with_description<IntoString>(mut self, description: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        self.description = description.into();
        self
    }

    /// How many model steps a Turn of this Agent may take.
    #[inline]
    #[must_use]
    pub const fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    #[must_use]
    pub fn with_model(self, spec: ModelSpec) -> Self {
        self.set_model(spec);
        self
    }

    #[inline]
    #[must_use]
    pub fn with_name<IntoString>(mut self, name: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        self.name = name.into();
        self
    }

    #[inline]
    #[must_use]
    pub fn with_skills(mut self, skills: SkillSet) -> Self {
        self.skills = skills;
        self
    }

    /// The Sub-agents this Agent may start and reach.
    #[inline]
    #[must_use]
    pub fn with_sub_agents(mut self, sub_agents: Arc<SubAgents>) -> Self {
        self.sub_agents = sub_agents;
        self
    }

    #[inline]
    #[must_use]
    pub fn with_tools(mut self, tools: ToolSet) -> Self {
        self.tools = tools;
        self
    }
}

/// Adds one step's usage to a Turn's total. Either side may be absent.
fn add_usage(total: Option<Usage>, step: Option<Usage>) -> Option<Usage> {
    match (total, step) {
        (Some(so_far), Some(latest)) => Some(so_far.merge(latest)),
        (so_far, latest) => so_far.or(latest),
    }
}

fn started(call: &ToolCall, summary: &str) -> Event {
    Event::ToolStarted { id: call.id.clone(), summary: summary.to_owned() }
}

fn finish(call: &ToolCall, summary: &str, result: &ToolResult) -> Event {
    Event::ToolFinished { id: call.id.clone(), summary: summary.to_owned(), result: result.clone() }
}

/// Runs `work` to the end, passing on the progress its Tool calls report while it runs.
#[expect(clippy::integer_division_remainder_used, reason = "tokio::select! expands to a remainder")]
async fn pump<T, F>(
    work: impl Future<Output = T>,
    reports: &mut UnboundedReceiver<ProgressReport>,
    on_event: &mut F,
) -> T
where
    F: FnMut(Event),
{
    tokio::pin!(work);
    let output = loop {
        select! {
            biased;
            output = &mut work => break output,
            Some(report) = reports.recv() => on_event(progress(report)),
        }
    };
    while let Ok(report) = reports.try_recv() {
        on_event(progress(report));
    }
    output
}

fn progress(report: ProgressReport) -> Event {
    Event::ToolProgress { id: report.call_id, status: report.status }
}

/// Runs `work` unless `cancelled` fires first.
#[expect(clippy::integer_division_remainder_used, reason = "tokio::select! expands to a remainder")]
async fn until<T>(cancelled: &CancellationToken, work: impl Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    select! {
        biased;
        () = cancelled.cancelled() => Err(Failure::Interrupted),
        result = work => result,
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::path::PathBuf;

    use super::*;
    use crate::agent::tools::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits};
    use crate::agent::tools::tool_set::ToolSet;
    use crate::llm::client::StreamedCompletion;
    use crate::llm::client::ToolChoice;
    use crate::llm::client::chat::{FunctionCall, Role, ToolCall, ToolCallKind};
    use crate::llm::client::fake;
    use crate::llm::client::fake::Client as FakeClient;
    use crate::llm::provider::{ModelSpec, Protocol, Provider};
    use crate::persona::Persona;
    use crate::test::assert_empty;
    use core::future;
    use core::sync::atomic::AtomicBool;
    use core::sync::atomic::Ordering;
    use core::time::Duration;
    use tokio::time;

    #[derive(Debug)]
    struct Echo;

    #[derive(Deserialize)]
    struct EchoArgs {
        text: String,
    }

    impl Tool for Echo {
        type Args = EchoArgs;

        fn description(&self) -> &'static str {
            "Echo the text back"
        }

        fn name(&self) -> &'static str {
            "echo"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
            future::ready(ToolResult::ok(format!("echo: {}", args.text)))
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        #[inline]
        fn traits(&self) -> ToolTraits {
            ToolTraits::default()
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    /// Like `Echo`, but it changes nothing, so calls to it may run together.
    #[derive(Debug)]
    struct Peek;

    impl Tool for Peek {
        type Args = EchoArgs;

        fn description(&self) -> &'static str {
            "Look at the text"
        }

        fn name(&self) -> &'static str {
            "peek"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
            future::ready(ToolResult::ok(format!("peek: {}", args.text)))
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        fn traits(&self) -> ToolTraits {
            ToolTraits::read_only()
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    /// Reports one status and says what its context holds: its call id and whether it is cancelled.
    #[derive(Debug)]
    struct Probe {
        read_only: bool,
    }

    impl Tool for Probe {
        type Args = EchoArgs;

        fn description(&self) -> &'static str {
            "Report what the call can see"
        }

        fn name(&self) -> &'static str {
            "probe"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        fn run(&self, args: EchoArgs, cx: &ToolContext) -> impl Future<Output = ToolResult> {
            cx.report("working");
            future::ready(ToolResult::ok(format!("{}:{}", args.text, cx.cancel.is_cancelled())))
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        fn traits(&self) -> ToolTraits {
            if self.read_only { ToolTraits::read_only() } else { ToolTraits::default() }
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    /// Cancels the Turn it runs in, then reports whether its own context saw that.
    #[derive(Debug)]
    struct CancelParent {
        parent: CancellationToken,
        saw_cancel: Arc<AtomicBool>,
    }

    impl Tool for CancelParent {
        type Args = EchoArgs;

        fn description(&self) -> &'static str {
            "Cancel the Turn"
        }

        fn name(&self) -> &'static str {
            "cancel_parent"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        fn run(&self, _args: EchoArgs, cx: &ToolContext) -> impl Future<Output = ToolResult> {
            self.parent.cancel();
            self.saw_cancel.store(cx.cancel.is_cancelled(), Ordering::SeqCst);
            future::ready(ToolResult::ok("cancelled"))
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        #[inline]
        fn traits(&self) -> ToolTraits {
            ToolTraits::default()
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    /// Spends tokens on a model of its own, as a sub-agent would.
    #[derive(Debug)]
    struct Spend;

    impl Tool for Spend {
        type Args = EchoArgs;

        fn description(&self) -> &'static str {
            "Use another model"
        }

        fn name(&self) -> &'static str {
            "spend"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        fn run(&self, _args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
            future::ready(ToolResult::ok("spent").with_usage(Usage {
                prompt_tokens: 5,
                completion_tokens: 2,
                total_tokens: 7,
                ..Default::default()
            }))
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        #[inline]
        fn traits(&self) -> ToolTraits {
            ToolTraits::default()
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    /// Waits far longer than any test, until it is cancelled.
    #[derive(Debug)]
    struct Sleepy;

    impl Tool for Sleepy {
        type Args = EchoArgs;

        fn description(&self) -> &'static str {
            "Wait"
        }

        fn name(&self) -> &'static str {
            "sleepy"
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        async fn run(&self, _args: EchoArgs, _cx: &ToolContext) -> ToolResult {
            time::sleep(Duration::from_secs(60)).await;
            ToolResult::ok("woke")
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        #[inline]
        fn traits(&self) -> ToolTraits {
            ToolTraits::default()
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    fn agent_with_tool(tool: impl Tool, script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
        let mut tools = ToolSet::new();
        tools.register(tool).unwrap();
        let fake = FakeClient::new(script);
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
            .with_model(test_model())
            .with_client(AnyClient::Fake(fake.clone()))
            .with_tools(tools);
        (agent, fake)
    }

    fn test_model() -> ModelSpec {
        ModelSpec::new(Provider::Ollama, Protocol::OpenAi, "test-model")
    }

    pub(super) fn build(script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
        let mut tools = ToolSet::new();
        tools.register(Echo).unwrap();
        let fake = FakeClient::new(script);
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
            .with_model(test_model())
            .with_client(AnyClient::Fake(fake.clone()))
            .with_tools(tools);
        (agent, fake)
    }

    fn agent(script: Vec<Result<StreamedCompletion, ClientError>>) -> Agent {
        build(script).0
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    pub(super) fn text(text: &str) -> Result<StreamedCompletion, ClientError> {
        Ok(StreamedCompletion { text: text.into(), usage: None, tool_calls: Vec::new(), ..Default::default() })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    fn call(id: &str, name: &str, arguments: &str) -> Result<StreamedCompletion, ClientError> {
        Ok(StreamedCompletion {
            text: String::new(),
            usage: None,
            tool_calls: vec![ToolCall {
                id: id.into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: name.into(), arguments: arguments.into() },
            }],
            ..Default::default()
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    fn calls(calls: &[(&str, &str, &str)]) -> Result<StreamedCompletion, ClientError> {
        Ok(StreamedCompletion {
            tool_calls: calls
                .iter()
                .map(|&(id, name, arguments)| ToolCall {
                    id: id.into(),
                    kind: ToolCallKind::Function,
                    function: FunctionCall { name: name.into(), arguments: arguments.into() },
                })
                .collect(),
            ..Default::default()
        })
    }

    fn with_peek(script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
        let mut tools = ToolSet::new();
        tools.register(Echo).unwrap();
        tools.register(Peek).unwrap();
        let fake = FakeClient::new(script);
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
            .with_model(test_model())
            .with_client(AnyClient::Fake(fake.clone()))
            .with_tools(tools);
        (agent, fake)
    }

    /// The tool events, each as its kind and the id of its call.
    fn tool_events(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match *event {
                Event::ToolStarted { ref id, .. } => Some(format!("start {id}")),
                Event::ToolProgress { ref id, ref status } => Some(format!("progress {id} {status}")),
                Event::ToolFinished { ref id, .. } => Some(format!("finish {id}")),
                Event::Text(_) | Event::Reasoning(_) | Event::StepStarted { .. } | Event::Notice(_) => None,
            })
            .collect()
    }

    fn big(tokens: usize) -> String {
        "x".repeat(tokens.saturating_mul(4))
    }

    async fn roles(agent: &Agent) -> Vec<Role> {
        roles_of(agent).await
    }

    async fn roles_of(agent: &Agent) -> Vec<Role> {
        agent.conversation.lock().await.messages().iter().map(|m| m.role).collect()
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn runs_a_tool_then_answers() {
        let agent = agent(vec![call("call_1", "echo", r#"{"text":"hi"}"#), text("done")]);

        let mut events = Vec::new();
        let response = agent.respond("say hi".into(), |e| events.push(e)).await.unwrap();

        assert_eq!(response.message.text.unwrap(), "done");
        let shown: Vec<Event> = events.into_iter().filter(|e| !matches!(e, Event::StepStarted { .. })).collect();
        assert!(matches!(&shown[0], Event::ToolStarted { summary, .. } if summary == "echo(hi)"));
        assert!(matches!(
            &shown[1],
            Event::ToolFinished { summary, result, .. }
                if summary == "echo(hi)" && *result == ToolResult::ok("echo: hi")
        ));
        assert!(matches!(&shown[2], Event::Text(t) if t == "done"));

        assert_eq!(roles(&agent).await, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        let conversation = agent.conversation.lock().await;
        let tool_message = &conversation.messages()[2];
        assert_eq!(tool_message.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(tool_message.content.as_deref(), Some("echo: hi"));
    }

    #[tokio::test]
    async fn offers_the_tool_set_and_sends_results_back() {
        let (agent, fake) = build(vec![call("c", "echo", r#"{"text":"x"}"#), text("ok")]);
        agent.respond("go".into(), |_| {}).await.unwrap();

        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|entry| entry.1.len() == 1 && entry.1[0].name == "echo"));
        assert_eq!(requests[0].0, 1, "first request: just the user message");
        assert_eq!(requests[1].0, 3, "second request: user, assistant tool call, tool result");
    }

    #[tokio::test]
    async fn an_agent_without_tools_offers_none() {
        let fake = FakeClient::new(vec![text("hi")]);
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
            .with_model(test_model())
            .with_client(AnyClient::Fake(fake.clone()));
        agent.respond("hello".into(), |_| {}).await.unwrap();
        assert_empty(&fake.requests()[0].1);
    }

    #[tokio::test]
    async fn unknown_tools_and_bad_arguments_reach_the_model_as_errors() {
        let agent = agent(vec![call("a", "launch_missiles", "{}"), call("b", "echo", "{not json"), text("sorry")]);
        let mut events = Vec::new();
        agent.respond("hi".into(), |e| events.push(e)).await.unwrap();

        let errors: Vec<&ToolResult> = events
            .iter()
            .filter_map(|e| match *e {
                Event::ToolFinished { ref result, .. } if result.is_error => Some(result),
                Event::Text(_)
                | Event::Reasoning(_)
                | Event::StepStarted { .. }
                | Event::ToolStarted { .. }
                | Event::ToolProgress { .. }
                | Event::Notice(_)
                | Event::ToolFinished { .. } => None,
            })
            .collect();
        assert_eq!(errors.len(), 2);
        assert!(errors[0].content.contains("unknown tool"));
        assert!(errors[1].content.contains("invalid arguments"));
    }

    #[tokio::test]
    async fn a_failed_turn_that_only_read_leaves_the_conversation_unchanged() {
        let (agent, _) = with_peek(vec![text("first"), call("c", "peek", r#"{"text":"x"}"#), Err(fake::failure())]);
        agent.respond("one".into(), |_| {}).await.unwrap();
        let before = roles(&agent).await;

        let err = agent.respond("two".into(), |_| {}).await.err().unwrap();
        assert!(matches!(err, Failure::Client(_)));
        assert_eq!(roles(&agent).await, before);
        assert_eq!(before, [Role::User, Role::Assistant]);
    }

    #[tokio::test]
    async fn a_failed_turn_that_changed_something_keeps_its_tool_steps() {
        let agent = agent(vec![call("c", "echo", r#"{"text":"x"}"#), Err(fake::failure())]);
        let err = agent.respond("edit".into(), |_| {}).await.err().unwrap();
        assert!(matches!(err, Failure::Client(_)));
        assert_eq!(roles(&agent).await, [Role::User, Role::Assistant, Role::Tool]);
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn an_interrupt_between_calls_still_records_the_calls_that_ran() {
        let parent = CancellationToken::new();
        let saw_cancel = Arc::new(AtomicBool::new(false));
        let mut tools = ToolSet::new();
        tools.register(CancelParent { parent: parent.clone(), saw_cancel }).unwrap();
        tools.register(Echo).unwrap();
        let fake = FakeClient::new(vec![calls(&[
            ("a", "cancel_parent", r#"{"text":"x"}"#),
            ("b", "echo", r#"{"text":"later"}"#),
        ])]);
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
            .with_model(test_model())
            .with_client(AnyClient::Fake(fake))
            .with_tools(tools);
        let result = agent.respond_under(&parent, "go".into(), |_| {}).await;
        assert!(matches!(result, Err(Failure::Interrupted)));

        let conversation = agent.conversation.lock().await;
        let results: Vec<_> = conversation
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.clone().unwrap())
            .collect();
        assert_eq!(results.len(), 2, "every call has a result");
        assert_eq!(results[0], "cancelled");
        assert!(results[1].contains("interrupted by the user"), "{}", results[1]);
    }

    #[tokio::test]
    async fn stopping_at_the_step_limit_keeps_the_tool_steps() {
        let script = (0..DEFAULT_MAX_STEPS).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
        let agent = agent(script);
        let result = agent.respond("loop".into(), |_| {}).await;
        assert!(matches!(result, Err(Failure::StepLimit(DEFAULT_MAX_STEPS))));
        let roles = roles(&agent).await;
        assert_eq!(roles.len(), 1 + 2 * DEFAULT_MAX_STEPS);
        assert_eq!(roles[0], Role::User);
    }

    #[tokio::test]
    async fn an_interrupt_stops_the_running_turn_and_the_ones_queued_behind_it() {
        let (agent, fake) = agent_with_tool(Sleepy, vec![call("c", "sleepy", r#"{"text":"x"}"#), text("never")]);
        let both = async {
            tokio::join!(agent.respond("one".into(), |_| {}), agent.respond("two".into(), |_| {}), async {
                time::sleep(Duration::from_millis(50)).await;
                agent.interrupt();
            })
        };
        let (first, second, ()) =
            time::timeout(Duration::from_secs(10), both).await.expect("the interrupt reached the running Turn");
        assert!(matches!(first, Err(Failure::Interrupted)));
        assert!(matches!(second, Err(Failure::Interrupted)));
        assert_eq!(fake.requests().len(), 1, "the queued Turn never asked the model");
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn a_failed_tool_call_is_stored_as_an_error_result() {
        let agent = agent(vec![call("c", "echo", "{not json"), text("done")]);
        agent.respond("go".into(), |_| {}).await.unwrap();
        let conversation = agent.conversation.lock().await;
        let tool = conversation.messages().iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(tool.is_error);
    }

    #[tokio::test]
    async fn a_large_prompt_counts_toward_compaction() {
        let budget = ContextBudget {
            elision: None,
            compaction: Some(context::Compaction { budget: 2_000, limit_share: 1.0, keep_recent: 0 }),
        };
        let (plain, fake) = build(vec![text("a"), text("b"), text("summary"), text("c")]);
        let agent = plain.with_context(&AgentContext::new(PathBuf::new().as_path()).with_budget(budget));
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        assert_eq!(fake.requests().len(), 2, "small turns do not compact");
        agent.respond(big(3_000), |_| {}).await.unwrap();
        assert_eq!(fake.requests().len(), 4, "the prompt alone is over the budget");
    }

    #[tokio::test]
    async fn the_estimate_is_corrected_by_the_tokens_the_provider_counted() {
        let mut reply = text("ok").unwrap();
        reply.usage =
            Some(Usage { prompt_tokens: 100_000, completion_tokens: 1, total_tokens: 100_001, ..Default::default() });
        let agent = agent(vec![Ok(reply)]);
        assert_eq!(agent.scaled(100), 100, "no correction before any request");
        agent.respond("hi".into(), |_| {}).await.unwrap();
        assert_eq!(agent.scaled(100), 300, "the ratio is capped at 3");
        agent.set_model(test_model());
        assert_eq!(agent.scaled(100), 100, "another model starts afresh");
    }

    #[tokio::test]
    async fn the_last_step_must_answer_in_text_and_ends_the_turn_with_a_reply() {
        let mut script: Vec<_> = (0i32..2i32).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
        script.push(text("ran out of steps; here is what is done"));
        let (agent, fake) = build(script);
        let response = agent.with_max_steps(3).respond("loop".into(), |_| {}).await.unwrap();
        assert_eq!(response.message.text.as_deref(), Some("ran out of steps; here is what is done"));
        assert_eq!(fake.tool_choices(), [ToolChoice::Auto, ToolChoice::Auto, ToolChoice::None]);
    }

    #[tokio::test]
    async fn a_turn_of_one_step_keeps_its_tools() {
        let (agent, fake) = build(vec![text("hi")]);
        agent.with_max_steps(1).respond("go".into(), |_| {}).await.unwrap();
        assert_eq!(fake.tool_choices(), [ToolChoice::Auto]);
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn the_model_is_told_when_few_steps_are_left() {
        let script = (0i32..6i32).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
        let agent = agent(script).with_max_steps(6);
        let _unused = agent.respond("loop".into(), |_| {}).await;
        let conversation = agent.conversation.lock().await;
        let results: Vec<&str> = conversation
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.as_deref().unwrap())
            .collect();
        assert!(!results[1].contains("left in this turn"), "{}", results[1]);
        assert!(results[2].contains("3 model steps left"), "{}", results[2]);
        assert!(results[3].contains("2 model steps left"), "{}", results[3]);
        assert!(results[4].contains("1 model step left"), "{}", results[4]);
        assert!(!results[5].contains("left in this turn"), "{}", results[5]);
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn repeating_a_failed_call_gets_a_hint() {
        let agent = agent(vec![call("a", "echo", "{not json"), call("b", "echo", "{not json"), text("done")]);
        agent.respond("go".into(), |_| {}).await.unwrap();
        let conversation = agent.conversation.lock().await;
        let results: Vec<&str> = conversation
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.as_deref().unwrap())
            .collect();
        assert!(!results[0].contains("already failed"), "{}", results[0]);
        assert!(results[1].contains("already failed"), "{}", results[1]);
    }

    #[tokio::test]
    async fn the_step_limit_is_each_agents_own() {
        let script = (0i32..3i32).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
        let agent = agent(script).with_max_steps(2);
        let result = agent.respond("loop".into(), |_| {}).await;
        assert!(matches!(result, Err(Failure::StepLimit(2))));
        assert_eq!(roles(&agent).await.len(), 1 + 2 * 2);
    }

    #[tokio::test]
    async fn each_model_call_starts_a_numbered_step() {
        let (agent, _) =
            agent_with_tool(Probe { read_only: false }, vec![call("c", "probe", r#"{"text":"x"}"#), text("done")]);
        let mut steps = Vec::new();
        agent
            .respond("go".into(), |e| {
                if let Event::StepStarted { step } = e {
                    steps.push(step);
                }
            })
            .await
            .unwrap();
        assert_eq!(steps, [1, 2]);
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn read_only_calls_start_together_and_finish_in_order() {
        let (agent, _) =
            with_peek(vec![calls(&[("a", "peek", r#"{"text":"1"}"#), ("b", "peek", r#"{"text":"2"}"#)]), text("done")]);
        let mut events = Vec::new();
        agent.respond("go".into(), |e| events.push(e)).await.unwrap();
        assert_eq!(tool_events(&events), ["start a", "start b", "finish a", "finish b"]);
        let conversation = agent.conversation.lock().await;
        let results: Vec<_> =
            conversation.messages()[2..4].iter().map(|m| (m.tool_call_id.as_deref(), m.content.as_deref())).collect();
        assert_eq!(results, [(Some("a"), Some("peek: 1")), (Some("b"), Some("peek: 2"))]);
    }

    #[tokio::test]
    async fn a_step_with_a_tool_that_changes_things_runs_one_call_at_a_time() {
        let (agent, _) =
            with_peek(vec![calls(&[("a", "peek", r#"{"text":"1"}"#), ("b", "echo", r#"{"text":"2"}"#)]), text("done")]);
        let mut events = Vec::new();
        agent.respond("go".into(), |e| events.push(e)).await.unwrap();
        assert_eq!(tool_events(&events), ["start a", "finish a", "start b", "finish b"]);
    }

    #[tokio::test]
    async fn a_reply_cut_off_after_some_text_is_kept_with_a_notice() {
        let cut = Ok(StreamedCompletion {
            text: "half an ans".into(),
            cut_off: Some("max_tokens".into()),
            ..Default::default()
        });
        let agent = agent(vec![cut]);
        let mut events = Vec::new();
        let response = agent.respond("hi".into(), |e| events.push(e)).await.unwrap();
        assert_eq!(response.message.text.as_deref(), Some("half an ans"));
        assert!(matches!(events.last(), Some(Event::Notice(n)) if n.contains("continue")));
        assert_eq!(roles(&agent).await, [Role::User, Role::Assistant]);
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn a_tool_call_cut_off_mid_argument_is_not_run_and_the_model_is_told() {
        let cut = Ok(StreamedCompletion {
            tool_calls: calls(&[("a", "echo", r#"{"text":"fine"}"#), ("b", "echo", r#"{"text":"cut of"#)])
                .unwrap()
                .tool_calls,
            cut_off: Some("max_tokens".into()),
            ..Default::default()
        });
        let agent = agent(vec![cut, text("smaller now")]);
        let response = agent.respond("go".into(), |_| {}).await.unwrap();
        assert_eq!(response.message.text.as_deref(), Some("smaller now"));

        let conversation = agent.conversation.lock().await;
        assert_eq!(conversation.messages()[2].content.as_deref(), Some("echo: fine"));
        let told = conversation.messages()[3].content.as_deref().unwrap();
        assert!(told.starts_with("error:") && told.contains("cut off"), "{told}");
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn a_request_that_is_too_long_is_retried_once_with_old_tool_output_elided() {
        let overflow = || Err(ClientError::BadRequest("prompt is too long: 300000 tokens".into()));
        let (agent, fake) = build(vec![
            call("c", "echo", &json!({ "text": big(5_000) }).to_string()),
            text("one"),
            overflow(),
            text("two"),
        ]);
        agent.respond("first".into(), |_| {}).await.unwrap();
        let mut events = Vec::new();
        agent.respond("second".into(), |e| events.push(e)).await.unwrap();

        assert_eq!(fake.requests().len(), 4);
        assert!(events.iter().any(|e| matches!(e, Event::Notice(_))));
        let conversation = agent.conversation.lock().await;
        let result = conversation.messages()[2].content.as_deref().unwrap();
        assert!(result.starts_with(r#"{"elided""#), "{result}");
    }

    #[tokio::test]
    async fn too_long_with_nothing_to_elide_is_an_error_not_a_retry() {
        let overflow = Err(ClientError::BadRequest("prompt is too long".into()));
        let (agent, fake) = build(vec![overflow]);
        let err = agent.respond("hi".into(), |_| {}).await.err().unwrap();
        assert!(matches!(err, Failure::Client(_)));
        assert_eq!(fake.requests().len(), 1);
    }

    #[tokio::test]
    async fn old_tool_output_is_elided_between_steps_but_what_the_model_has_not_read_is_not() {
        let budget = ContextBudget {
            elision: Some(Elision { trigger: 100, target: 100, smallest: 10 }),
            compaction: Some(context::Compaction { budget: 1_000, limit_share: 1.0, keep_recent: 0 }),
        };
        let arguments = json!({ "text": big(2_000) }).to_string();
        let (plain, _) = build(vec![call("a", "echo", &arguments), call("b", "echo", &arguments), text("done")]);
        let agent = plain.with_context(&AgentContext::new(PathBuf::new().as_path()).with_budget(budget));
        agent.respond("go".into(), |_| {}).await.unwrap();

        let conversation = agent.conversation.lock().await;
        let elided =
            |index: usize| conversation.messages()[index].content.as_deref().unwrap().starts_with(r#"{"elided""#);
        // system, user, [assistant, tool] for a, [assistant, tool] for b, assistant
        assert!(elided(3), "the first result is old by the last request");
        assert!(!elided(5), "the last result was only just read");
    }

    /// Two turns; the first leaves about 32k tokens of tool arguments and output behind.
    async fn second_turn_after_bulky_first(agent: Agent) -> Agent {
        agent.respond("first".into(), |_| {}).await.unwrap();
        agent.respond("second".into(), |_| {}).await.unwrap();
        agent
    }

    fn bulky_script() -> Vec<Result<StreamedCompletion, ClientError>> {
        let arguments = json!({ "text": big(8_000) }).to_string();
        vec![calls(&[("a", "echo", &arguments), ("b", "echo", &arguments)]), text("one"), text("two")]
    }

    fn first_result_elided(conversation: &Conversation) -> bool {
        let first = conversation.messages().iter().find(|m| m.role == Role::Tool).unwrap();
        first.content.as_deref().unwrap().starts_with(r#"{"elided""#)
    }

    #[tokio::test]
    async fn old_tool_output_is_kept_between_turns_while_the_context_has_room() {
        let (fresh, _) = build(bulky_script());
        let agent = second_turn_after_bulky_first(fresh).await;
        assert!(!first_result_elided(&*agent.conversation.lock().await));
    }

    #[tokio::test]
    async fn old_tool_output_is_elided_between_turns_once_the_context_is_half_full() {
        let budget = ContextBudget {
            compaction: Some(context::Compaction { budget: 40_000, limit_share: 1.0, keep_recent: 20_000 }),
            ..ContextBudget::default()
        };
        let (fresh, _) = build(bulky_script());
        let configured = fresh.with_context(&AgentContext::new(PathBuf::new().as_path()).with_budget(budget));
        let agent = second_turn_after_bulky_first(configured).await;
        assert!(first_result_elided(&*agent.conversation.lock().await));
    }

    #[tokio::test]
    async fn a_tool_sees_its_call_id_and_reports_progress_on_its_own_card() {
        let (agent, _) =
            agent_with_tool(Probe { read_only: false }, vec![call("call_9", "probe", r#"{"text":"x"}"#), text("done")]);
        let mut events = Vec::new();
        agent.respond("go".into(), |e| events.push(e)).await.unwrap();

        assert_eq!(tool_events(&events), ["start call_9", "progress call_9 working", "finish call_9"]);
        let conversation = agent.conversation.lock().await;
        assert_eq!(
            conversation.messages()[2].content.as_deref(),
            Some("x:false"),
            "the tool saw its own context and no cancellation"
        );
    }

    #[tokio::test]
    async fn read_only_calls_report_progress_on_their_own_cards_while_they_run_together() {
        let (agent, _) = agent_with_tool(
            Probe { read_only: true },
            vec![calls(&[("a", "probe", r#"{"text":"1"}"#), ("b", "probe", r#"{"text":"2"}"#)]), text("done")],
        );
        let mut events = Vec::new();
        agent.respond("go".into(), |e| events.push(e)).await.unwrap();

        assert_eq!(
            tool_events(&events),
            ["start a", "start b", "progress a working", "progress b working", "finish a", "finish b",]
        );
    }

    #[tokio::test]
    async fn cancelling_the_parent_cancels_the_turn_and_the_tool_sees_it() {
        let parent = CancellationToken::new();
        let saw_cancel = Arc::new(AtomicBool::new(false));
        let (agent, fake) = agent_with_tool(
            CancelParent { parent: parent.clone(), saw_cancel: Arc::clone(&saw_cancel) },
            vec![call("c", "cancel_parent", r#"{"text":"x"}"#), text("never")],
        );
        let result = agent.respond_under(&parent, "go".into(), |_| {}).await;

        assert!(matches!(result, Err(Failure::Interrupted)));
        assert!(saw_cancel.load(Ordering::SeqCst));
        assert_eq!(fake.requests().len(), 1, "no model call after the cancel");
    }

    #[tokio::test]
    async fn a_turn_whose_parent_is_already_cancelled_makes_no_model_call() {
        let parent = CancellationToken::new();
        parent.cancel();
        let (agent, fake) = build(vec![text("never")]);
        let result = agent.respond_under(&parent, "go".into(), |_| {}).await;
        assert!(matches!(result, Err(Failure::Interrupted)));
        assert_empty(fake.requests());
        assert_empty(&roles_of(&agent).await);
    }

    #[tokio::test]
    async fn a_tools_usage_counts_toward_the_turn_with_the_models_own() {
        let (agent, _) = agent_with_tool(Spend, {
            let mut first = call("c", "spend", r#"{"text":"x"}"#).unwrap();
            first.usage =
                Some(Usage { prompt_tokens: 10, completion_tokens: 1, total_tokens: 11, ..Default::default() });
            vec![Ok(first), text("done")]
        });
        let response = agent.respond("go".into(), |_| {}).await.unwrap();
        let total = response.usage.unwrap();
        assert_eq!((total.prompt_tokens, total.completion_tokens, total.total_tokens), (15, 3, 18));
    }

    #[tokio::test]
    async fn a_turns_usage_is_the_tools_when_the_model_reports_none() {
        let (agent, _) = agent_with_tool(Spend, vec![call("c", "spend", r#"{"text":"x"}"#), text("done")]);
        let response = agent.respond("go".into(), |_| {}).await.unwrap();
        assert_eq!(response.usage.map(|u| u.total_tokens), Some(7));
    }

    #[tokio::test]
    async fn an_empty_reply_leaves_the_conversation_unchanged() {
        let agent = agent(vec![text("")]);
        let response = agent.respond("hi".into(), |_| {}).await.unwrap();
        assert_eq!(response.message.text.unwrap(), "");
        assert_empty(&roles(&agent).await);
    }

    #[tokio::test]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    async fn an_empty_final_reply_keeps_prior_tool_steps_in_conversation() {
        let (agent, fake) = build(vec![call("c", "echo", r#"{"text":"x"}"#), text(""), text("next reply")]);

        let response = agent.respond("run tool".into(), |_| {}).await.unwrap();
        assert_eq!(response.message.text.as_deref(), Some(""));
        assert_eq!(roles(&agent).await, [Role::User, Role::Assistant, Role::Tool]);
        {
            let conversation = agent.conversation.lock().await;
            let tool_message = &conversation.messages()[2];
            assert_eq!(tool_message.tool_call_id.as_deref(), Some("c"));
            assert_eq!(tool_message.content.as_deref(), Some("echo: x"));
        }

        agent.respond("continue".into(), |_| {}).await.unwrap();
        assert_eq!(fake.requests()[2].0, 4, "next turn includes the prior tool step");
    }

    #[tokio::test]
    async fn turns_never_interleave() {
        let agent = agent(vec![
            call("a", "echo", r#"{"text":"1"}"#),
            text("reply one"),
            call("b", "echo", r#"{"text":"2"}"#),
            text("reply two"),
        ]);
        let (first, second) = tokio::join!(agent.respond("one".into(), |_| {}), agent.respond("two".into(), |_| {}),);
        first.unwrap();
        second.unwrap();

        let expected = [Role::User, Role::Assistant, Role::Tool, Role::Assistant];
        let all = roles(&agent).await;
        assert_eq!(&all[..4], &expected);
        assert_eq!(&all[4..], &expected);
    }

    #[tokio::test]
    async fn usage_is_summed_across_steps() {
        let usage = |p, c| Usage { prompt_tokens: p, completion_tokens: c, total_tokens: p + c, ..Default::default() };
        let mut first = call("c", "echo", r#"{"text":"x"}"#).unwrap();
        first.usage = Some(usage(10, 2));
        let mut second = text("done").unwrap();
        second.usage = Some(usage(30, 5));
        let agent = agent(vec![Ok(first), Ok(second)]);

        let total = agent.respond("hi".into(), |_| {}).await.unwrap().usage.unwrap();
        assert_eq!((total.prompt_tokens, total.completion_tokens, total.total_tokens), (40, 7, 47));
    }

    #[tokio::test]
    async fn the_system_prompt_is_kept_across_turns() {
        let agent = agent(vec![text("a"), text("b")]).with_context(&AgentContext::new(PathBuf::new().as_path()));
        agent.respond("1".into(), |_| {}).await.unwrap();
        agent.respond("2".into(), |_| {}).await.unwrap();
        assert_eq!(roles(&agent).await, [Role::System, Role::User, Role::Assistant, Role::User, Role::Assistant]);
    }

    #[test]
    fn the_model_can_be_switched() {
        let agent = agent(vec![]);
        assert_eq!(agent.model(), Some(test_model()));
        let other = ModelSpec::new(Provider::OpenAi, Protocol::OpenAi, "gpt-4.1");
        agent.set_model(other.clone());
        assert_eq!(agent.model(), Some(other));
    }

    #[tokio::test]
    async fn an_agent_without_a_model_says_how_to_get_one_and_keeps_the_conversation_clean() {
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()));
        assert_eq!(agent.model(), None);
        let err = agent.respond("hi".into(), |_| {}).await.err().unwrap();
        assert!(matches!(err, Failure::NoModel));
        assert_eq!(err.to_string(), "No model selected. Run /refresh-models, then /model.");
        assert_empty(&roles_of(&agent).await);
    }

    #[tokio::test]
    async fn settings_are_read_and_changed_together() {
        let agent = agent(vec![text("a")]);
        assert_eq!(agent.settings(), Settings { model: Some(test_model()), thinking: Thinking::Medium });
        agent.apply(Change::SetThinking(Thinking::Max));
        let other = ModelSpec::new(Provider::Ollama, Protocol::OpenAi, "other");
        agent.apply(Change::SetModel(other.clone()));
        assert_eq!(agent.settings(), Settings { model: Some(other), thinking: Thinking::Max });
    }

    #[test]
    fn clearing_the_conversation_ends_its_sub_agents() {
        let agent = agent(vec![])
            .with_sub_agents(Arc::new(SubAgents::new(Delegation { max_per_session: 1, ..Delegation::default() })));
        let spawn = || {
            agent
                .sub_agents()
                .spawn(&Persona::new("tester", "Tester", ""), || Agent::new(Arc::new(CredentialStore::in_memory())))
        };
        assert_eq!(spawn().unwrap(), "tester-1");
        assert!(spawn().is_err(), "the Root agent's limit applies");

        assert!(agent.clear_conversation());
        assert_empty(agent.sub_agents().infos());
        assert_eq!(spawn().unwrap(), "tester-2");
    }

    #[tokio::test]
    async fn clearing_keeps_the_system_prompt_and_drops_the_conversation() {
        let agent = agent(vec![text("a")]).with_context(&AgentContext::new(PathBuf::new().as_path()));
        agent.respond("1".into(), |_| {}).await.unwrap();
        assert!(agent.clear_conversation());
        assert_eq!(roles(&agent).await, [Role::System]);
    }
}
