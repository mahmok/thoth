use alloc::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::domain::message::{FunctionCall, ToolCall, ToolCallKind};
use crate::domain::shared::Usage;

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
/// Assembles Tool calls from streamed pieces.
pub struct ToolCallBuffer {
    calls: BTreeMap<usize, PartialCall>,
}

impl ToolCallBuffer {
    #[inline]
    /// Adds a streamed piece of a Tool call, appending each field as it arrives.
    pub fn append(&mut self, delta: ToolCallDelta) {
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

    #[inline]
    /// Starts (or replaces) a Tool call that arrived whole, such as at a content block start.
    pub fn start(&mut self, index: usize, id: &str, name: &str, arguments: &str) {
        drop(self.calls.insert(
            index,
            PartialCall { arguments: arguments.to_owned(), id: Some(id.to_owned()), name: name.to_owned() },
        ));
    }

    #[inline]
    /// Appends a fragment to the arguments of a started Tool call.
    pub fn append_arguments(&mut self, index: usize, fragment: &str) {
        if let Some(call) = self.calls.get_mut(&index) {
            call.arguments.push_str(fragment);
        }
    }

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

#[cfg(test)]
mod tool_stream_tests {
    use super::*;

    fn assemble(chunks: &[&str]) -> Vec<ToolCall> {
        let mut buffer = ToolCallBuffer::default();
        for body in chunks {
            let chunk: CompletionChunk = serde_json::from_str(body).unwrap();
            for choice in chunk.choices {
                for delta in choice.delta.tool_calls {
                    buffer.append(delta);
                }
            }
        }
        buffer.finish()
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
}
