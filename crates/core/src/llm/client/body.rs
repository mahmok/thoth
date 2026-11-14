//! The fields every request body shares.

use serde::Serialize;

/// Request-body fields whose shape is the same in every dialect.
#[derive(Serialize)]
pub(super) struct Common<'request, Tool, Choice> {
    pub model: &'request str,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Choice>,
}

impl<'request, Tool, Choice> Common<'request, Tool, Choice> {
    /// The fields of a streamed request.
    #[must_use]
    #[inline]
    pub const fn streaming(model: &'request str, tools: Vec<Tool>, tool_choice: Option<Choice>) -> Self {
        Self { model, stream: true, tools, tool_choice }
    }
}
