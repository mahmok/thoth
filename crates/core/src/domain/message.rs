//! The messages a Conversation is made of: what goes to the model and what comes back.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
/// A chat message.
pub struct Message {
    #[serde(default)]
    pub content: Option<String>,
    /// For `Role::Tool` messages: the call failed. Only Anthropic has a field for it.
    #[serde(skip)]
    pub is_error: bool,
    /// The model's reasoning before this reply, sent back so it keeps its train of thought.
    #[serde(skip)]
    pub reasoning: Vec<Reasoning>,
    pub role: Role,
    /// For `Role::Tool` messages: the id of the call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

/// The model's own reasoning from one reply, kept whole in the form its protocol needs back.
/// Each protocol sends back only its own kind, so switching models mid-conversation is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reasoning {
    /// An Anthropic `thinking` or `redacted_thinking` block (with its signature).
    Anthropic(Value),
    /// A Gemini part to send back as received, such as a thought and its signature.
    Google(Value),
    /// A Responses API `reasoning` output item (with its encrypted content).
    Responses(Value),
}

impl Reasoning {
    /// The kept value, if this reasoning came from Anthropic.
    #[must_use]
    #[inline]
    pub const fn anthropic(&self) -> Option<&Value> {
        match *self {
            Self::Anthropic(ref value) => Some(value),
            Self::Google(_) | Self::Responses(_) => None,
        }
    }

    /// The kept value, if this reasoning came from Gemini.
    #[must_use]
    #[inline]
    pub const fn google(&self) -> Option<&Value> {
        match *self {
            Self::Google(ref value) => Some(value),
            Self::Anthropic(_) | Self::Responses(_) => None,
        }
    }

    /// The kept value, if this reasoning came from the Responses API.
    #[must_use]
    #[inline]
    pub const fn responses(&self) -> Option<&Value> {
        match *self {
            Self::Responses(ref value) => Some(value),
            Self::Anthropic(_) | Self::Google(_) => None,
        }
    }
}

impl Message {
    /// The text of the message, unless it is empty.
    #[must_use]
    #[inline]
    pub fn text(&self) -> Option<&str> {
        self.content.as_deref().filter(|text| !text.is_empty())
    }

    #[must_use]
    #[inline]
    pub const fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self { role: Role::Assistant, content, tool_calls, tool_call_id: None, reasoning: Vec::new(), is_error: false }
    }

    #[inline]
    /// A system message.
    pub fn system<Text>(content: Text) -> Self
    where
        Text: Into<String>,
    {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: Vec::new(),
            is_error: false,
        }
    }

    #[inline]
    /// A Tool result message for the call `tool_call_id`.
    pub fn tool<CallId, Text>(tool_call_id: CallId, content: Text) -> Self
    where
        CallId: Into<String>,
        Text: Into<String>,
    {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            reasoning: Vec::new(),
            is_error: false,
        }
    }

    #[inline]
    /// A user message.
    pub fn user<Text>(content: Text) -> Self
    where
        Text: Into<String>,
    {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: Vec::new(),
            is_error: false,
        }
    }

    #[must_use]
    #[inline]
    pub const fn with_error(mut self, is_error: bool) -> Self {
        self.is_error = is_error;
        self
    }

    #[must_use]
    #[inline]
    pub fn with_reasoning(mut self, reasoning: Vec<Reasoning>) -> Self {
        self.reasoning = reasoning;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
/// Who a message is from.
pub enum Role {
    Assistant,
    System,
    Tool,
    User,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
/// A call to a Tool.
pub struct ToolCall {
    pub function: FunctionCall,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ToolCallKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
/// The kind of a Tool call.
pub enum ToolCallKind {
    Function,
}

/// A function call exactly as the model produced it: the tool's name and its
/// arguments as JSON text. Turning that text into typed arguments is the tool's job.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FunctionCall {
    #[serde(default, deserialize_with = "arguments_as_text")]
    pub arguments: String,
    pub name: String,
}

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

/// Servers send arguments as a JSON-encoded string; a plain JSON object is
/// accepted too and kept as its JSON text.
fn arguments_as_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => text,
        Value::Null => String::new(),
        other @ (Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_message() {
        let body = r#"{"role":"assistant","content":"How are you today?"}"#;
        let message: Message = serde_json::from_str(body).unwrap();
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content.as_deref(), Some("How are you today?"));
    }

    #[test]
    fn message_serialization_omits_empty_tool_fields() {
        let value = serde_json::to_value(Message::user("hi")).unwrap();
        assert!(value.get("tool_calls").is_none());
        assert!(value.get("tool_call_id").is_none());
        let tool_value = serde_json::to_value(Message::tool("c1", "out")).unwrap();
        assert_eq!(tool_value["tool_call_id"], "c1");
        assert_eq!(tool_value["role"], "tool");
    }

    #[test]
    fn accepts_object_arguments_as_json_text() {
        let call: FunctionCall =
            serde_json::from_str(r#"{"name":"list_directory","arguments":{"path":"src"}}"#).unwrap();
        assert_eq!(call.arguments, r#"{"path":"src"}"#);
    }

    #[test]
    fn missing_arguments_become_empty_text() {
        let call: FunctionCall = serde_json::from_str(r#"{"name":"list_directory"}"#).unwrap();
        assert_eq!(call.arguments, "");
    }

    #[test]
    fn serializes_arguments_as_a_string() {
        let call = FunctionCall { name: "t".into(), arguments: "{}".into() };
        assert_eq!(serde_json::to_value(&call).unwrap()["arguments"], "{}");
    }
}
