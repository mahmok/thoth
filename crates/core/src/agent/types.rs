use crate::llm::client::chat::{Reasoning, Usage};

#[derive(Debug, Default, Clone)]
/// A model message: text and reasoning.
pub struct AgentMessage {
    pub reasoning: Vec<Reasoning>,
    pub text: Option<String>,
}

impl AgentMessage {
    #[must_use]
    #[inline]
    pub const fn new(text: Option<String>, reasoning: Vec<Reasoning>) -> Self {
        Self { reasoning, text }
    }
}

impl From<&str> for AgentMessage {
    #[inline]
    fn from(text: &str) -> Self {
        Self::new(Some(text.to_owned()), Vec::new())
    }
}

#[derive(Clone, Debug)]
/// The result of a Turn.
pub struct AgentResponse {
    pub message: AgentMessage,
    pub usage: Option<Usage>,
}
