/// The Anthropic Messages client.
pub mod anthropic;
/// A client of any protocol.
pub mod any;
/// The fields every request body shares.
mod body;
/// Chat messages and completions.
pub mod chat;
/// Client errors.
pub mod error;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::expect_used, reason = "test support: an exhausted script should fail the test")]
/// A scripted client for tests.
pub mod fake;
/// The Gemini `generateContent` client.
pub mod google;
/// Lowering a conversation for a dialect.
mod lowering;
/// Falling back to settings a model accepts.
mod negotiation;
pub mod openai;
pub mod responses;
/// Server-sent event framing.
mod sse;
/// Driving one streamed reply.
mod stream;

use core::future::Future;

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
    #[must_use]
    pub fn forbids(self, tools: &[ToolSpec]) -> bool {
        self == Self::None && !tools.is_empty()
    }
}

#[derive(Default, Debug)]
pub struct CreateCompletionRequest<'data> {
    pub messages: &'data [chat::Message],
    pub thinking: Thinking,
    pub tool_choice: ToolChoice,
    pub tools: &'data [ToolSpec],
}

#[derive(Default, Debug)]
pub struct StreamedCompletion {
    pub cut_off: Option<String>,
    pub reasoning: Vec<chat::Reasoning>,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

impl StreamedCompletion {
    /// # Errors
    ///
    /// Fails if the reply was cut off before producing any text or Tool calls.
    pub fn fail_if_cut_off_empty(self) -> Result<Self, ClientError> {
        match self.cut_off {
            Some(ref reason) if self.text.is_empty() && self.tool_calls.is_empty() => Err(error::cut_off(reason)),
            _ => Ok(self),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta<'data> {
    Reasoning(&'data str),
    Text(&'data str),
}

pub trait Llm {
    fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        on_delta: F,
    ) -> impl Future<Output = Result<StreamedCompletion, ClientError>> + Send
    where
        F: FnMut(Delta<'_>) + Send;
}
