/// Summarising old turns.
mod compaction;
pub mod delegation;
pub mod sub_agents;
/// Runs one Turn: its model steps, Tool calls, and events.
mod turn;

use crate::adapters::context::AgentContext;
use crate::adapters::llm::client::error::Failure as ClientError;
use crate::adapters::skills::SkillSet;
use crate::application::ports::{ClientFactory, ModelClient};
use crate::domain::conversation::Conversation;
use crate::domain::conversation::budget::{Budget, ContextBudget};
use crate::domain::provider::ModelSpec;
use crate::domain::shared::SessionId;
use crate::domain::shared::Usage;
use crate::domain::thinking::Thinking;
use crate::domain::tool::Outcome as ToolResult;
use crate::runtime::agent::delegation::Delegation;
use crate::runtime::agent::sub_agents::SubAgents;
use crate::runtime::tools::tool_set::ToolSet;
use crate::sync::{MutexExt as _, RwLockExt as _};
use crate::utils::numbers;
use alloc::fmt;
use alloc::sync::Arc;
use core::error;
use core::future::Future;
use std::sync;
use std::sync::RwLock;
use tokio::select;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// How many model steps a Turn may take before it stops with a step-limit error.
pub const DEFAULT_MAX_STEPS: usize = 20;

/// The id of the card that shows a compaction. It is not a model tool call, so it has no id from
/// the model.
pub const COMPACTION_CALL_ID: &str = "compaction";

#[derive(Debug, Clone)]
/// Something an Agent reports while it works.
pub enum AgentEvent {
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

#[derive(Debug, Clone)]
/// An Agent event, taken apart once. Everything that reads the event stream matches this rather
/// than the event enum, so a new event variant is read in one place.
pub enum Observed {
    /// A fragment of the reply.
    Text(String),
    /// A fragment of the model's thinking.
    Reasoning(String),
    /// Something the user should know about the Turn.
    Notice(String),
    /// A model call is starting; steps are numbered from 1 within a Turn.
    Step(usize),
    /// A Tool call is starting.
    ToolStarted {
        /// The id of the call.
        id: String,
        /// A short description of the call.
        summary: String,
    },
    /// A running Tool call reports a status line.
    ToolProgress {
        /// The id of the call.
        id: String,
        /// What the call is doing.
        status: String,
    },
    /// A Tool call finished.
    ToolFinished {
        /// The id of the call.
        id: String,
        /// A short description of the call.
        summary: String,
        /// What the Tool returned.
        result: ToolResult,
    },
}

impl Observed {
    /// Reads `event`. This is the one place the Agent's event enum is matched.
    #[must_use]
    pub fn of(event: AgentEvent) -> Self {
        match event {
            AgentEvent::Text(delta) => Self::Text(delta),
            AgentEvent::Reasoning(delta) => Self::Reasoning(delta),
            AgentEvent::Notice(text) => Self::Notice(text),
            AgentEvent::StepStarted { step } => Self::Step(step),
            AgentEvent::ToolStarted { id, summary } => Self::ToolStarted { id, summary },
            AgentEvent::ToolProgress { id, status } => Self::ToolProgress { id, status },
            AgentEvent::ToolFinished { id, summary, result } => Self::ToolFinished { id, summary, result },
        }
    }
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
pub enum AgentChange {
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

impl error::Error for Failure {}

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
/// A model and its client, or the reason its protocol has no client.
struct Current {
    client: Result<Arc<dyn ModelClient>, String>,
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
    pub(crate) conversation: Mutex<Conversation>,
    current: RwLock<Option<Current>>,
    description: String,
    factory: Arc<dyn ClientFactory>,
    max_steps: usize,
    name: String,
    session: SessionId,
    skills: SkillSet,
    sub_agents: Arc<SubAgents>,
    thinking: RwLock<Thinking>,
    tools: ToolSet,
}

impl Agent {
    #[inline]
    /// Applies a change that was queued for the Agent.
    pub fn apply(&self, change: AgentChange) {
        match change {
            AgentChange::SetModel(spec) => self.set_model(spec),
            AgentChange::SetThinking(thinking) => self.set_thinking(thinking),
            AgentChange::ClearConversation => {
                let _cleared = self.clear_conversation();
            },
            AgentChange::Interrupt => self.interrupt(),
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

    pub(crate) fn client(&self) -> Result<Arc<dyn ModelClient>, Failure> {
        let current = self.current.read_unpoisoned();
        let client = match current.as_ref().ok_or(Failure::NoModel)?.client {
            Ok(ref client) => Ok(Arc::clone(client)),
            Err(ref message) => Err(Failure::Client(ClientError::BadRequest(message.clone()))),
        };
        drop(current);
        client
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
        F: FnMut(AgentEvent) + Send,
    {
        if !self.budget.compacts() {
            return Err(Failure::Compaction("compaction is turned off"));
        }
        let mut conversation = self.conversation.lock().await;
        let (cancelled, _active) = self.new_cancellation(&CancellationToken::new());
        let compaction = self.compact_conversation(&mut conversation, 0, focus.as_deref(), &mut on_event);
        match until(&cancelled, compaction).await {
            Err(Failure::NothingToCompact) => {
                on_event(AgentEvent::ToolFinished {
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
    /// Creates an Agent with no model, no Tools and an empty Conversation. It mints an id of its
    /// own; an Agent built for a Session is given that Session's id instead.
    pub fn new(factory: Arc<dyn ClientFactory>) -> Self {
        Self {
            factory,
            current: RwLock::new(None),
            tools: ToolSet::new(),
            conversation: Mutex::new(Conversation::new()),
            budget: ContextBudget::new(Budget::default()),
            max_steps: DEFAULT_MAX_STEPS,
            sub_agents: Arc::new(SubAgents::new(Delegation::default())),
            skills: SkillSet::default(),
            thinking: RwLock::new(Thinking::default()),
            active: sync::Mutex::default(),
            calibration: sync::Mutex::new(1.0),
            name: String::new(),
            description: String::new(),
            session: SessionId::mint(),
        }
    }

    /// A token for the work about to start, child of `parent`, and the guard that keeps it
    /// reachable by `interrupt` until the work is done.
    ///
    /// Work that waits for the Conversation is registered too, so an interrupt stops the running
    /// Turn and the ones queued behind it.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    fn new_cancellation(&self, parent: &CancellationToken) -> (CancellationToken, ActiveGuard<'_>) {
        let token = parent.child_token();
        let mut active = self.active.lock_unpoisoned();
        active.next = active.next.saturating_add(1);
        let id = active.next;
        active.tokens.push((id, token.clone()));
        (token, ActiveGuard { agent: self, id })
    }

    /// How much real prompt tokens one estimated token turned out to be at the last request.
    fn calibration(&self) -> f32 {
        *self.calibration.lock_unpoisoned()
    }

    #[inline]
    /// Switches to `spec`, with a client of its own, and restarts token calibration.
    fn set_model(&self, spec: ModelSpec) {
        let client = self.factory.client_for(&spec, self.session);
        *self.current.write_unpoisoned() = Some(Current { client, spec });
        // Another model counts tokens its own way.
        *self.calibration.lock_unpoisoned() = 1.0;
    }

    /// Applies from the next model call on, including the rest of a turn in progress.
    #[inline]
    fn set_thinking(&self, thinking: Thinking) {
        *self.thinking.write_unpoisoned() = thinking;
    }

    #[inline]
    pub fn settings(&self) -> Settings {
        Settings { model: self.model(), thinking: self.thinking() }
    }

    #[inline]
    pub const fn skills(&self) -> &SkillSet {
        &self.skills
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

    #[inline]
    /// Sets the system prompt and the context budget from `context`.
    #[must_use]
    pub fn with_context(mut self, context: &AgentContext) -> Self {
        self.conversation.get_mut().set_system_prompt(context.get_combined_system_prompt());
        self.budget = ContextBudget::new(*context.budget());
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

    /// The Session this Agent belongs to, whose id every request of its clients carries.
    #[must_use]
    #[inline]
    pub const fn session_id(&self) -> SessionId {
        self.session
    }

    /// The Sub-agents this Agent may start and reach.
    #[inline]
    #[must_use]
    pub fn with_sub_agents(mut self, sub_agents: Arc<SubAgents>) -> Self {
        self.sub_agents = sub_agents;
        self
    }

    /// Gives the Agent the id of the Session it serves.
    #[inline]
    #[must_use]
    pub const fn with_session_id(mut self, session: SessionId) -> Self {
        self.session = session;
        self
    }

    #[inline]
    #[must_use]
    pub fn with_tools(mut self, tools: ToolSet) -> Self {
        self.tools = tools;
        self
    }
}

/// Runs `work` unless `cancelled` fires first.
async fn until<T>(cancelled: &CancellationToken, work: impl Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    select! {
        biased;
        () = cancelled.cancelled() => Err(Failure::Interrupted),
        result = work => result,
    }
}

#[cfg(test)]
pub(crate) mod tests;
