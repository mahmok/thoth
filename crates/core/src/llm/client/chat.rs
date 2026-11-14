use alloc::collections::BTreeMap;

use serde::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
/// A chat completion reply.
pub struct CompletionResponse {
    pub choices: Vec<Choice>,
    pub created: u64,
    pub id: String,
    pub model: String,
    pub object: String,
    #[serde(default)]
    pub system_fingerprint: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
/// One choice of a reply.
pub struct Choice {
    pub finish_reason: FinishReason,
    pub index: u32,
    pub message: Message,
}

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

impl Message {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Why a reply ended.
pub enum FinishReason {
    ContentFilter,
    FunctionCall,
    Length,
    Stop,
    ToolCalls,
    #[serde(other)]
    Unknown,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
/// Token counts of a request.
#[expect(clippy::struct_field_names, reason = "the names follow the provider's wire format or what the type wraps")]
pub struct Usage {
    /// How much of the prompt the provider served from its cache. Part of `prompt_tokens`.
    #[serde(default, rename = "prompt_tokens_details", deserialize_with = "cached_tokens")]
    pub cached_tokens: u32,
    /// Tokens in the reply.
    pub completion_tokens: u32,
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Prompt and reply together.
    pub total_tokens: u32,
}

impl Usage {
    /// Sums two usages, e.g. across the steps of one agent turn.
    #[must_use]
    #[inline]
    pub const fn merge(self, other: Self) -> Self {
        Self {
            prompt_tokens: self.prompt_tokens.saturating_add(other.prompt_tokens),
            completion_tokens: self.completion_tokens.saturating_add(other.completion_tokens),
            total_tokens: self.total_tokens.saturating_add(other.total_tokens),
            cached_tokens: self.cached_tokens.saturating_add(other.cached_tokens),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
/// A streamed piece of a reply.
pub struct CompletionChunk {
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
/// One choice of a chunk.
pub struct ChunkChoice {
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
/// A streamed change to a message.
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Clone, Default, Deserialize)]
/// A streamed change to a Tool call.
pub struct ToolCallDelta {
    #[serde(default)]
    pub function: Option<FunctionDelta>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub index: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
/// A streamed change to a function call.
pub struct FunctionDelta {
    #[serde(default)]
    pub arguments: Option<Value>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Default, Debug)]
/// A Tool call being assembled.
struct PartialCall {
    arguments: String,
    id: Option<String>,
    name: String,
}

#[derive(Default, Debug)]
/// Assembles Tool calls from deltas.
pub struct ToolCallAssembler {
    calls: BTreeMap<usize, PartialCall>,
}

impl ToolCallAssembler {
    #[must_use]
    #[inline]
    /// The assembled Tool calls, in order.
    pub fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .map(|(index, call)| ToolCall {
                id: call.id.unwrap_or_else(|| format!("call_{index}")),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: call.name, arguments: call.arguments },
            })
            .collect()
    }

    #[inline]
    /// Adds a streamed piece to the Tool call it belongs to.
    pub fn push(&mut self, delta: ToolCallDelta) {
        let call = self.calls.entry(delta.index).or_default();
        if let Some(id) = delta.id.filter(|id| !id.is_empty()) {
            call.id = Some(id);
        }
        let Some(function) = delta.function else {
            return;
        };
        if let Some(name) = function.name {
            call.name.push_str(&name);
        }
        match function.arguments {
            Some(Value::String(fragment)) => call.arguments.push_str(&fragment),
            Some(Value::Null) | None => {},
            Some(object) => call.arguments = object.to_string(),
        }
    }
}

/// Servers send arguments as a JSON-encoded string; a plain JSON object is
/// accepted too and kept as its JSON text.
fn arguments_as_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => text,
        Value::Null => String::new(),
        other @ (Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => other.to_string(),
    })
}

/// Chat completions report cached tokens inside `prompt_tokens_details`.
fn cached_tokens<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Details {
        #[serde(default)]
        cached_tokens: u32,
    }
    Ok(Option::<Details>::deserialize(deserializer)?.map_or(0, |details| details.cached_tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ollama_chat_completion() {
        let body = r#"{"id":"chatcmpl-710","object":"chat.completion","created":1792440437,
            "model":"llama3.2:3b","system_fingerprint":"fp_ollama",
            "choices":[{"index":0,"message":{"role":"assistant","content":"How are you today?"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":26,"completion_tokens":6,"total_tokens":32}}"#;
        let res: CompletionResponse = serde_json::from_str(body).unwrap();
        assert_eq!(res.choices[0].message.role, Role::Assistant);
        assert_eq!(res.choices[0].finish_reason, FinishReason::Stop);
        assert_eq!(res.choices[0].message.content.as_deref(), Some("How are you today?"));
        assert_eq!(res.usage.unwrap().total_tokens, 32);
    }

    #[test]
    fn parses_tool_calls() {
        let body = r#"{"id":"x","object":"chat.completion","created":1,"model":"m",
            "choices":[{"index":0,"message":{"role":"assistant","content":null,
              "tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]},
              "finish_reason":"tool_calls"}]}"#;
        let res: CompletionResponse = serde_json::from_str(body).unwrap();
        let choice = &res.choices[0];
        assert_eq!(choice.finish_reason, FinishReason::ToolCalls);
        assert!(choice.message.content.is_none());
        let function = &choice.message.tool_calls[0].function;
        assert_eq!(function.name, "read_file");
        assert_eq!(function.arguments, r#"{"path":"a"}"#);
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    #[test]
    fn parses_final_usage_chunk() {
        let body = r#"{"id":"c","choices":[],"usage":{"prompt_tokens":26,"completion_tokens":6,"total_tokens":32}}"#;
        let chunk: CompletionChunk = serde_json::from_str(body).unwrap();
        assert!(chunk.choices.is_empty());
        assert_eq!(chunk.usage.unwrap().total_tokens, 32);
    }

    #[test]
    fn cached_tokens_are_read_from_the_prompt_details() {
        let body = r#"{"choices":[],"usage":{"prompt_tokens":2006,"completion_tokens":6,"total_tokens":2012,"prompt_tokens_details":{"cached_tokens":1920}}}"#;
        let chunk: CompletionChunk = serde_json::from_str(body).unwrap();
        assert_eq!(chunk.usage.unwrap().cached_tokens, 1920);
    }

    #[test]
    fn content_chunk_has_no_usage() {
        let body = r#"{"choices":[{"index":0,"delta":{"content":"hi"}}]}"#;
        let chunk: CompletionChunk = serde_json::from_str(body).unwrap();
        assert!(chunk.usage.is_none());
    }
}

#[cfg(test)]
mod tool_stream_tests {
    use super::*;

    fn assemble(chunks: &[&str]) -> Vec<ToolCall> {
        let mut assembler = ToolCallAssembler::default();
        for body in chunks {
            let chunk: CompletionChunk = serde_json::from_str(body).unwrap();
            for choice in chunk.choices {
                for delta in choice.delta.tool_calls {
                    assembler.push(delta);
                }
            }
        }
        assembler.finish()
    }

    #[test]
    fn assembles_a_whole_call_as_sent_by_ollama() {
        let calls = assemble(&[
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","index":0,"type":"function","function":{"name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(
            calls[0].function,
            FunctionCall { name: "read_file".into(), arguments: r#"{"path":"Cargo.toml"}"#.into() }
        );
    }

    #[test]
    fn assembles_fragmented_arguments_as_sent_by_openai() {
        let calls = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"x\"}"}}]}}]}"#,
        ]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.arguments, r#"{"path":"x"}"#);
    }

    #[test]
    fn keeps_parallel_calls_separate_and_ordered() {
        let calls = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"list_directory","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{\"path\":\"p\"}"}}]}}]}"#,
        ]);
        assert_eq!(calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
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
}

#[cfg(test)]
mod function_call_tests {
    use super::*;

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

    #[test]
    fn reasoning_fields_are_read_from_a_chunk() {
        let chunk: CompletionChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"reasoning_content":"think"}},{"delta":{"reasoning":"also"}}]}"#,
        )
        .unwrap();
        assert_eq!(chunk.choices[0].delta.reasoning_content.as_deref(), Some("think"));
        assert_eq!(chunk.choices[1].delta.reasoning.as_deref(), Some("also"));
    }
}
