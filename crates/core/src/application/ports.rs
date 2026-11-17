//! Ports the application uses to drive the runtime.

use alloc::sync::Arc;
use core::fmt::Debug;
use core::future::Future;
use core::pin::Pin;

use crate::adapters::llm::client::error::Failure as ClientError;
use crate::adapters::llm::client::{CreateCompletionRequest, Delta, Llm, StreamedCompletion};
use crate::adapters::skills::SkillSummary;
use crate::domain::provider::ModelSpec;
use crate::domain::shared::{SessionId, Usage};
use crate::runtime::agent::{AgentChange, AgentEvent, Settings};

/// The clock the application passes down. Tests supply a fixed one.
pub trait Clock: Debug + Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
}

#[derive(Debug, Default)]
/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    #[inline]
    fn now_ms(&self) -> u64 {
        crate::clock::now_ms()
    }
}

/// A model client the runtime can drive without knowing its protocol.
pub trait ModelClient: Debug + Send + Sync {
    /// Streams one reply, reporting each fragment through `on_delta`.
    fn create_completion_stream<'a>(
        &'a self,
        request: CreateCompletionRequest<'a>,
        on_delta: &'a mut (dyn for<'delta> FnMut(Delta<'delta>) + Send),
    ) -> Pin<Box<dyn Future<Output = Result<StreamedCompletion, ClientError>> + Send + 'a>>;
}

impl<T> ModelClient for T
where
    T: Llm + Debug + Send + Sync,
{
    #[inline]
    fn create_completion_stream<'a>(
        &'a self,
        request: CreateCompletionRequest<'a>,
        mut on_delta: &'a mut (dyn for<'delta> FnMut(Delta<'delta>) + Send),
    ) -> Pin<Box<dyn Future<Output = Result<StreamedCompletion, ClientError>> + Send + 'a>> {
        Box::pin(async move { Llm::create_completion_stream(self, request, &mut on_delta).await })
    }
}

/// Builds the client of a model. The production implementation is the one auth resolution point,
/// so header and base-url logic is not duplicated.
pub trait ClientFactory: Debug + Send + Sync {
    /// The client for `spec`, serving `session`.
    ///
    /// # Errors
    ///
    /// Fails with a message when the model's protocol cannot be spoken.
    fn client_for(&self, spec: &ModelSpec, session: SessionId) -> Result<Arc<dyn ModelClient>, String>;
}

#[derive(Debug)]
/// What the Agent task sends back.
pub enum Reply {
    Done { usage: Option<Usage> },
    Event(AgentEvent),
    Failed(String),
}

/// Runs a Turn and hands back what it reports.
pub trait TurnRunner: Debug {
    fn apply(&self, change: AgentChange);
    /// Compacts the Conversation, reporting through [`try_next`](Self::try_next) like a Turn.
    fn compact(&self, focus: Option<String>);
    fn settings(&self) -> Settings;
    fn skill_block(&self, name: &str) -> Option<String>;
    fn skills(&self) -> Vec<SkillSummary>;
    /// Starts a Turn for `prompt`. What it produces comes back through
    /// [`try_next`](Self::try_next).
    fn start(&self, prompt: String);
    /// The next thing the running Turn reported, if anything.
    fn try_next(&self) -> Option<Reply>;
}
