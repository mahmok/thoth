/// The Anthropic Messages client.
pub mod anthropic;
/// A client of any protocol.
pub mod any;
/// Chat messages and completions.
pub mod chat;
/// Client errors.
pub mod error;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::expect_used, reason = "test support: an exhausted script should fail the test")]
/// A scripted client for tests.
pub mod fake;
/// Falling back to settings a model accepts.
mod negotiation;
pub mod openai;
pub mod responses;
/// Server-sent event framing.
mod sse;

use core::future::Future;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use error::Failure as ClientError;
use negotiation::Negotiation;
use sse::EventStream;

use chat::{ToolCall, Usage};

use crate::llm::thinking::Thinking;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
/// A Tool as the model sees it.
pub struct ToolSpec {
    /// What it does, for the model.
    pub description: String,
    /// The name the model calls it by.
    pub name: String,
    /// The JSON schema of its arguments.
    pub parameters: Value,
}

/// Whether the model may call tools in one request. The tool definitions stay in the request
/// either way, so the provider's prompt cache for them is not lost.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice {
    /// The model decides.
    #[default]
    Auto,
    /// The model must answer in text.
    None,
}

impl ToolChoice {
    /// Whether the request must say that no tool may be called. With no tools there is nothing
    /// to forbid, and a server may refuse a choice then.
    pub fn forbids(self, tools: &[ToolSpec]) -> bool {
        self == Self::None && !tools.is_empty()
    }
}

#[derive(Default, Debug)]
/// A request for a completion.
pub struct CreateCompletionRequest<'data> {
    /// The history to send.
    pub messages: &'data [chat::Message],
    /// How hard it should think.
    pub thinking: Thinking,
    /// Whether it may, must or may not call Tools.
    pub tool_choice: ToolChoice,
    /// The Tools the model may call.
    pub tools: &'data [ToolSpec],
}

#[derive(Default, Debug)]
/// A completion assembled from a stream.
pub struct StreamedCompletion {
    /// Why the reply stopped early, if it did.
    pub cut_off: Option<String>,
    /// Reasoning blocks to send back with the reply.
    pub reasoning: Vec<chat::Reasoning>,
    /// The reply text.
    pub text: String,
    /// The Tool calls it asked for.
    pub tool_calls: Vec<ToolCall>,
    /// Tokens spent, if the provider says.
    pub usage: Option<Usage>,
}

impl StreamedCompletion {
    pub fn fail_if_cut_off_empty(self) -> Result<Self, ClientError> {
        match self.cut_off {
            Some(ref reason) if self.text.is_empty() && self.tool_calls.is_empty() => Err(error::cut_off(reason)),
            _ => Ok(self),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A piece of a streamed reply.
pub enum Delta<'data> {
    Reasoning(&'data str),
    Text(&'data str),
}

/// A client for one provider protocol.
pub trait Llm {
    fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        on_delta: F,
    ) -> impl Future<Output = Result<StreamedCompletion, ClientError>> + Send
    where
        F: FnMut(Delta<'_>) + Send;
}

fn new_cache_key() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    format!("{}-{}-{nanos:x}", crate::APP_NAME, process::id())
}
