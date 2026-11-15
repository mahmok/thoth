//! The Gemini `generateContent` client.
//!
//! Gemini has no request-level model field: the model is part of the path, and its
//! `generateContent` dialect shares nothing with the other three but the transport, the
//! stream driver, and the lowering skeleton. That is why [`Body`] does not flatten
//! [`Common`](super::body::Common): the fields it would add (`model`, `stream`) are either in
//! the path or meaningless here, and Gemini rejects what it does not know.

use alloc::collections::BTreeMap;

use reqwest::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::auth::Auth;
use crate::llm::thinking::Thinking;

use super::chat::{Message, Reasoning, ToolCall, ToolCallBuffer, Usage};
use super::{lowering, stream};

use super::{
    ClientError, CreateCompletionRequest, Delta, Llm as LlmClient, Negotiation, StreamedCompletion, ToolChoice,
    ToolSpec,
};

#[derive(Debug)]
/// Settings of the Gemini client.
pub struct Config {
    pub auth: Auth,
    pub model: String,
}

#[derive(Debug)]
/// The Gemini `generateContent` client.
pub struct Client {
    config: Config,
    negotiation: Negotiation,
}

impl Client {
    #[must_use]
    #[inline]
    pub fn new(config: Config) -> Self {
        Self { config, negotiation: Negotiation::default() }
    }

    /// POSTs the request, trying thinking levels until the model accepts one.
    async fn post(&self, request: &CreateCompletionRequest<'_>) -> Result<Response, ClientError> {
        let owned_prompt = Prompt::new(request.messages);
        let prompt = &owned_prompt;
        let include_thoughts = request.thinking != Thinking::Off;
        self.negotiation
            .send_refusing_status(request.thinking, levels(request.thinking), |level| {
                let level = *level;
                async move {
                    self.config
                        .auth
                        .post_json(
                            &self.path(),
                            &[],
                            request.messages,
                            &body(prompt, request.tools, request.tool_choice, level, include_thoughts),
                        )
                        .await
                }
            })
            .await
    }

    /// Where a streamed generation of one model lives.
    fn path(&self) -> String {
        format!("/v1beta/models/{}:streamGenerateContent?alt=sse", self.config.model)
    }
}

/// The request body for a lowered `prompt`.
fn body<'request>(
    prompt: &'request Prompt<'request>,
    tools: &'request [ToolSpec],
    choice: ToolChoice,
    level: Option<&'static str>,
    include_thoughts: bool,
) -> Body<'request> {
    Body {
        contents: &prompt.contents,
        generation_config: level.map(|thinking_level| GenerationConfig {
            thinking_config: ThinkingConfig { include_thoughts, thinking_level },
        }),
        system_instruction: prompt
            .system
            .as_deref()
            .map(|text| WireContent { parts: vec![WirePart::Text { text }], role: None }),
        tool_config: choice
            .forbids(tools)
            .then_some(WireToolConfig { function_calling_config: FunctionCallingConfig { mode: "NONE" } }),
        tools: (!tools.is_empty())
            .then(|| [WireTools { function_declarations: tools.iter().map(declaration).collect() }]),
    }
}

/// The thinking levels to try, best first. Gemini 4 cannot turn thinking off, and Gemini 2.5
/// ignores levels entirely, so even `Off` asks for the least it will do.
const fn levels(thinking: Thinking) -> &'static [Option<&'static str>] {
    match thinking {
        Thinking::Off | Thinking::Minimal | Thinking::Low => &[Some("low")],
        Thinking::Medium => &[Some("medium")],
        Thinking::High => &[Some("high")],
        Thinking::Max => &[Some("xhigh"), Some("high"), None],
    }
}

/// The history lowered once per request.
struct Prompt<'request> {
    contents: Vec<WireContent<'request>>,
    system: Option<String>,
}

impl<'request> Prompt<'request> {
    fn new(history: &'request [Message]) -> Self {
        let (system, contents) = lowering::lower(history, Lowering::default());
        Prompt { contents, system }
    }
}

/// The `:streamGenerateContent` request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'request> {
    contents: &'request [WireContent<'request>],
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<WireContent<'request>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<WireToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<[WireTools<'request>; 1]>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// How much the model may think.
struct GenerationConfig {
    thinking_config: ThinkingConfig,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// The thinking setting. `thinkingBudget` is never sent: Gemini 4 rejects it beside a level.
struct ThinkingConfig {
    include_thoughts: bool,
    thinking_level: &'static str,
}

#[derive(Serialize)]
/// One turn of the conversation.
struct WireContent<'request> {
    parts: Vec<WirePart<'request>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(untagged)]
/// One part of a turn.
enum WirePart<'request> {
    /// Reasoning sent back exactly as it arrived, signature and all.
    Kept(&'request Value),
    Text {
        text: &'request str,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: WireCall<'request>,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: WireResult<'request>,
    },
}

#[derive(Serialize)]
/// A call the model made.
struct WireCall<'request> {
    args: &'request RawValue,
    id: &'request str,
    name: &'request str,
}

#[derive(Serialize)]
/// What a Tool answered with.
struct WireResult<'request> {
    id: Option<&'request str>,
    name: &'request str,
    response: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// The Tool definitions of one request.
struct WireTools<'request> {
    function_declarations: Vec<WireTool<'request>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// One Tool as the model sees it.
struct WireTool<'request> {
    description: &'request str,
    name: &'request str,
    parameters: &'request Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// Whether the model may call Tools.
struct WireToolConfig {
    function_calling_config: FunctionCallingConfig,
}

#[derive(Serialize)]
/// The mode the model must honour.
struct FunctionCallingConfig {
    mode: &'static str,
}

fn declaration(spec: &ToolSpec) -> WireTool<'_> {
    WireTool { description: &spec.description, name: &spec.name, parameters: &spec.parameters }
}

impl LlmClient for Client {
    #[inline]
    async fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        on_delta: F,
    ) -> Result<StreamedCompletion, ClientError>
    where
        F: FnMut(Delta<'_>) + Send,
    {
        stream::drive(self.post(&request), StreamState::default(), on_delta).await
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// One event of a streamed generation.
struct Chunk {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    usage_metadata: Option<WireUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// One candidate of a chunk.
struct Candidate {
    #[serde(default)]
    content: Option<Content>,
    /// Absent on intermediate chunks, which is not a cut-off.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
/// The parts one candidate produced so far.
struct Content {
    #[serde(default)]
    parts: Vec<Value>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "the names follow the provider's wire format or what the type wraps")]
/// Token counts in the wire format. Every event repeats the running totals.
struct WireUsage {
    #[serde(default)]
    cached_content_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
    #[serde(default)]
    prompt_token_count: u32,
    #[serde(default)]
    total_token_count: u32,
}

impl WireUsage {
    /// Gemini reports the total itself, and leaves it out for some gateways.
    const fn total(&self) -> u32 {
        if self.total_token_count > 0 {
            self.total_token_count
        } else {
            self.prompt_token_count.saturating_add(self.candidates_token_count)
        }
    }

    const fn to_usage(&self) -> Usage {
        Usage {
            prompt_tokens: self.prompt_token_count,
            completion_tokens: self.candidates_token_count,
            total_tokens: self.total(),
            cached_tokens: self.cached_content_token_count,
        }
    }
}

#[derive(Default)]
/// What one streamed generation has produced.
struct StreamState {
    calls: ToolCallBuffer,
    cut_off: Option<String>,
    next_call: usize,
    reasoning: Vec<Reasoning>,
    text: String,
    usage: Option<Usage>,
}

impl stream::Adapter for StreamState {
    fn is_end(&self, data: &str) -> bool {
        // Gemini ends the stream by closing it. Some gateways still send the OpenAI sentinel.
        data == "[DONE]"
    }

    fn apply(&mut self, data: &str, on_delta: &mut impl FnMut(Delta<'_>)) -> Result<(), ClientError> {
        let chunk: Chunk = serde_json::from_str(data)?;
        if let Some(usage) = chunk.usage_metadata {
            self.usage = Some(usage.to_usage());
        }
        for candidate in chunk.candidates {
            // Absent is normal, `CONTINUATION` is a long decode still going, and only the token
            // limit means the reply we got is not the whole one.
            if let Some(reason) = candidate.finish_reason.filter(|reason| reason == "MAX_TOKENS") {
                self.cut_off = Some(reason);
            }
            let Some(content) = candidate.content else {
                continue;
            };
            for part in content.parts {
                // A signature makes the part opaque: it has to go back exactly as it arrived.
                if part.get("thoughtSignature").is_some() {
                    self.reasoning.push(Reasoning::Google(part.clone()));
                }
                // Gemini sends a call whole, never as a stream of argument fragments.
                if let Some(call) = part.get("functionCall") {
                    let index = self.next_call;
                    self.next_call = self.next_call.saturating_add(1);
                    let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
                    let arguments = call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(serde_json::Map::default()))
                        .to_string();
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map_or_else(|| format!("call_{index}"), str::to_owned);
                    self.calls.start(index, &id, name, &arguments);
                }
                let Some(text) = part.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()) else {
                    continue;
                };
                if part.get("thought").and_then(Value::as_bool).unwrap_or(false) {
                    on_delta(Delta::Reasoning(text));
                } else {
                    on_delta(Delta::Text(text));
                    self.text.push_str(text);
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> StreamedCompletion {
        StreamedCompletion {
            text: self.text,
            reasoning: self.reasoning,
            tool_calls: self.calls.finish(),
            usage: self.usage,
            cut_off: self.cut_off,
        }
    }
}

/// A call that arrived on a kept part, which must not be sent twice.
struct SignedCall<'request> {
    arguments: Value,
    id: Option<&'request str>,
    name: &'request str,
}

impl SignedCall<'_> {
    /// Whether `call` is the same call, already carried by the kept part.
    fn repeats(&self, call: &ToolCall) -> bool {
        self.id.map_or_else(
            || {
                self.name == call.function.name
                    && serde_json::from_str::<Value>(&call.function.arguments).unwrap_or_default() == self.arguments
            },
            |id| id == call.id,
        )
    }
}

/// The Gemini view of a conversation: a system instruction and alternating turns.
#[derive(Default)]
struct Lowering<'request> {
    contents: Vec<WireContent<'request>>,
    /// The names of the calls made so far, so a result can name the call it answers.
    names: BTreeMap<&'request str, &'request str>,
    system: Option<String>,
}

impl<'request> lowering::Dialect<'request> for Lowering<'request> {
    type Output = (Option<String>, Vec<WireContent<'request>>);

    fn system(&mut self, text: &str) {
        self.system = Some(text.to_owned());
    }

    fn user(&mut self, text: &'request str) {
        push_turn(&mut self.contents, "user", vec![WirePart::Text { text }]);
    }

    fn assistant(&mut self, message: &'request Message) {
        for call in &message.tool_calls {
            self.names.insert(&call.id, &call.function.name);
        }
        // Kept parts go back first, exactly as they arrived: tool turns need their signatures.
        let mut signed_text = false;
        let mut signed_calls: Vec<SignedCall<'request>> = Vec::new();
        let mut parts: Vec<WirePart<'request>> = Vec::new();
        for part in message.reasoning.iter().filter_map(Reasoning::google) {
            if part.get("text").and_then(Value::as_str).is_some_and(|text| !text.is_empty()) {
                signed_text = true;
            }
            if let Some(call) = part.get("functionCall") {
                signed_calls.push(SignedCall {
                    arguments: call.get("args").cloned().unwrap_or(Value::Null),
                    id: call.get("id").and_then(Value::as_str),
                    name: call.get("name").and_then(Value::as_str).unwrap_or_default(),
                });
            }
            parts.push(WirePart::Kept(part));
        }
        // A kept part that carries text owns it: text is never concatenated across a signature.
        if let Some(text) = message.text().filter(|_| !signed_text) {
            parts.push(WirePart::Text { text });
        }
        parts.extend(
            message.tool_calls.iter().filter(|call| !signed_calls.iter().any(|signed| signed.repeats(call))).map(
                |call| WirePart::FunctionCall {
                    function_call: WireCall {
                        args: input_of(&call.function.arguments),
                        id: &call.id,
                        name: &call.function.name,
                    },
                },
            ),
        );
        push_turn(&mut self.contents, "model", parts);
    }

    fn tool_result(&mut self, message: &'request Message) {
        let id = message.tool_call_id.as_deref();
        let name = id.and_then(|id| self.names.get(id).copied()).unwrap_or_default();
        let key = if message.is_error { "error" } else { "output" };
        let mut response = serde_json::Map::new();
        drop(response.insert(key.into(), Value::String(message.text().unwrap_or_default().to_owned())));
        let function_response = WireResult { id, name, response: Value::Object(response) };
        push_turn(&mut self.contents, "user", vec![WirePart::FunctionResponse { function_response }]);
    }

    fn finish(self) -> Self::Output {
        (self.system, self.contents)
    }
}

/// A call's arguments as JSON, or an empty object when they are not JSON.
#[expect(clippy::expect_used, reason = "`{}` is valid JSON")]
fn input_of(arguments: &str) -> &RawValue {
    serde_json::from_str::<&RawValue>(arguments)
        .unwrap_or_else(|_| serde_json::from_str("{}").expect("an empty object is JSON"))
}

/// Adds a turn, or extends the previous one when the roles match.
fn push_turn<'request>(turns: &mut Vec<WireContent<'request>>, role: &'static str, parts: Vec<WirePart<'request>>) {
    if parts.is_empty() {
        return;
    }
    match turns.last_mut() {
        Some(last) if last.role == Some(role) => last.parts.extend(parts),
        _ => turns.push(WireContent { parts, role: Some(role) }),
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::auth::store::Credentials as CredentialStore;
    use crate::auth::testing::MockServer;
    use crate::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind};
    use crate::llm::client::{ToolChoice, ToolSpec};
    use crate::llm::provider::{Protocol, Provider};
    use serde_json::json;

    /// The path a streamed request takes. The mock server matches on the path alone.
    const PATH: &str = "/v1beta/models/m:streamGenerateContent";

    /// Asks a real client, over HTTP, against a scripted event stream.
    async fn ask(reply: &str) -> (StreamedCompletion, Vec<String>) {
        let server = MockServer::start(vec![(PATH, vec![(200, reply.to_owned())])]).await;
        let auth = Auth::new(
            // Ollama needs no credential; the protocol still has to be Gemini's.
            Provider::builtin("ollama").unwrap(),
            Protocol::Google,
            Arc::new(CredentialStore::in_memory()),
        )
        .with_base_url(&server.url);
        let client = Client::new(Config { auth, model: "m".into() });
        let messages = [Message::user("hi")];
        let mut reasoning: Vec<String> = Vec::new();
        let done = client
            .create_completion_stream(CreateCompletionRequest { messages: &messages, ..Default::default() }, |delta| {
                if let Delta::Reasoning(text) = delta {
                    reasoning.push(text.to_owned());
                }
            })
            .await
            .unwrap();
        (done, reasoning)
    }

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall { arguments: arguments.into(), name: name.into() },
        }
    }

    /// The body a request would send, as JSON.
    fn body_of(
        messages: &[Message],
        tools: &[ToolSpec],
        choice: ToolChoice,
        level: Option<&'static str>,
        include_thoughts: bool,
    ) -> Value {
        let prompt = Prompt::new(messages);
        serde_json::to_value(body(&prompt, tools, choice, level, include_thoughts)).unwrap()
    }

    #[test]
    fn a_reply_is_lowered_reasoning_then_text_then_calls() {
        let messages = [
            Message::system("be brief"),
            Message::user("go"),
            Message::assistant(Some("hello".into()), vec![call("c1", "read", "{\"path\":\"a\"}")])
                .with_reasoning(vec![Reasoning::Google(json!({ "text": "", "thoughtSignature": "SIG" }))]),
        ];
        let body = body_of(&messages, &[], ToolChoice::Auto, None, true);

        assert_eq!(body["systemInstruction"], json!({ "parts": [{ "text": "be brief" }] }));
        assert_eq!(body["contents"][0], json!({ "role": "user", "parts": [{ "text": "go" }] }));
        assert_eq!(body["contents"][1]["role"], "model");
        assert_eq!(
            body["contents"][1]["parts"],
            json!([
                { "text": "", "thoughtSignature": "SIG" },
                { "text": "hello" },
                { "functionCall": { "name": "read", "id": "c1", "args": { "path": "a" } } },
            ])
        );
    }

    #[test]
    fn tool_results_are_one_turn_and_name_the_call_they_answer() {
        let messages = [
            Message::user("go"),
            Message::assistant(None, vec![call("c1", "read", "{}"), call("c2", "read", "{}")]),
            Message::tool("c1", "one"),
            Message::tool("c2", "two"),
        ];
        let body = body_of(&messages, &[], ToolChoice::Auto, None, true);

        assert_eq!(body["contents"].as_array().unwrap().len(), 3, "both results follow the call turn");
        assert_eq!(
            body["contents"][2],
            json!({ "role": "user", "parts": [
                { "functionResponse": { "name": "read", "id": "c1", "response": { "output": "one" } } },
                { "functionResponse": { "name": "read", "id": "c2", "response": { "output": "two" } } },
            ] })
        );
    }

    #[test]
    fn a_signed_part_is_not_sent_twice() {
        let signed = json!({ "functionCall": { "name": "read", "id": "c1", "args": {} }, "thoughtSignature": "SIG" });
        let messages = [
            Message::user("go"),
            Message::assistant(None, vec![call("c1", "read", "{}")])
                .with_reasoning(vec![Reasoning::Google(signed.clone())]),
        ];
        let body = body_of(&messages, &[], ToolChoice::Auto, None, true);

        assert_eq!(body["contents"][1]["parts"], json!([signed]), "the kept part already carries the call");
    }

    #[test]
    fn signed_text_is_not_repeated_as_a_plain_part() {
        let signed = json!({ "text": "hi", "thoughtSignature": "SIG" });
        let messages = [
            Message::user("go"),
            Message::assistant(Some("hi".into()), vec![]).with_reasoning(vec![Reasoning::Google(signed.clone())]),
        ];
        let body = body_of(&messages, &[], ToolChoice::Auto, None, true);

        assert_eq!(body["contents"][1]["parts"], json!([signed]));
    }

    #[test]
    fn a_signed_fragment_does_not_replay_the_whole_text() {
        let signed = json!({ "text": "lo", "thoughtSignature": "SIG" });
        let messages = [
            Message::user("go"),
            Message::assistant(Some("hello".into()), vec![]).with_reasoning(vec![Reasoning::Google(signed.clone())]),
        ];
        let body = body_of(&messages, &[], ToolChoice::Auto, None, true);

        assert_eq!(
            body["contents"][1]["parts"],
            json!([signed]),
            "the signed part carries its fragment, and the text is not concatenated across the boundary"
        );
    }

    #[test]
    fn an_empty_signature_part_leaves_the_plain_text_in_place() {
        let signed = json!({ "text": "", "thoughtSignature": "TAIL" });
        let messages = [
            Message::user("go"),
            Message::assistant(Some("hello".into()), vec![]).with_reasoning(vec![Reasoning::Google(signed.clone())]),
        ];
        let body = body_of(&messages, &[], ToolChoice::Auto, None, true);

        assert_eq!(body["contents"][1]["parts"], json!([signed, { "text": "hello" }]));
    }

    #[test]
    fn a_forbidden_tool_choice_sends_none_mode() {
        let tools = [ToolSpec {
            description: "Read a file".into(),
            name: "read".into(),
            parameters: json!({ "type": "object" }),
        }];
        let body = body_of(&[Message::user("go")], &tools, ToolChoice::None, None, true);

        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "read");
        assert_eq!(body["toolConfig"], json!({ "functionCallingConfig": { "mode": "NONE" } }));
    }

    #[test]
    fn the_body_carries_a_thinking_level_but_no_token_cap_or_budget() {
        let body = body_of(&[Message::user("go")], &[], ToolChoice::Auto, Some("low"), true);

        assert_eq!(
            body["generationConfig"]["thinkingConfig"],
            json!({ "thinkingLevel": "low", "includeThoughts": true })
        );
        assert!(body.get("maxOutputTokens").is_none());
        assert!(body["generationConfig"].get("maxOutputTokens").is_none());
        assert!(body["generationConfig"]["thinkingConfig"].get("thinkingBudget").is_none());
        assert!(
            body_of(&[Message::user("go")], &[], ToolChoice::Auto, None, true).get("generationConfig").is_none(),
            "a model that refused every level is asked without one"
        );
        assert_eq!(
            body_of(&[Message::user("go")], &[], ToolChoice::Auto, Some("low"), false)["generationConfig"]["thinkingConfig"]
                ["includeThoughts"],
            false,
            "thinking off asks for no summaries"
        );
    }

    #[tokio::test]
    async fn a_model_that_refuses_xhigh_falls_back_to_high() {
        const REPLY: &str =
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}\n\n";
        let refusal = json!({
            "error": { "code": 400, "message": "Request contains an invalid argument.", "status": "INVALID_ARGUMENT" }
        })
        .to_string();
        let server = MockServer::start(vec![(PATH, vec![(400, refusal), (200, REPLY.to_owned())])]).await;
        let auth =
            Auth::new(Provider::builtin("ollama").unwrap(), Protocol::Google, Arc::new(CredentialStore::in_memory()))
                .with_base_url(&server.url);
        let client = Client::new(Config { auth, model: "m".into() });
        let messages = [Message::user("hi")];

        let done = client
            .create_completion_stream(
                CreateCompletionRequest { messages: &messages, thinking: Thinking::Max, ..Default::default() },
                |_| {},
            )
            .await
            .unwrap();

        assert_eq!(done.text, "ok");
        let levels: Vec<Value> = server
            .requests()
            .into_iter()
            .map(|request| {
                let body: Value = serde_json::from_str(&request.body).unwrap();
                body["generationConfig"]["thinkingConfig"]["thinkingLevel"].clone()
            })
            .collect();
        assert_eq!(levels, [json!("xhigh"), json!("high")], "the refused level is not tried again");
    }

    #[tokio::test]
    async fn text_and_usage_arrive_from_a_streamed_reply() {
        let (done, _) = ask(concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"he\"}],\"role\":\"model\"},\"index\":0}],",
            "\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":1,\"totalTokenCount\":4}}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"llo\"}],\"role\":\"model\"},\"index\":0}],",
            "\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2,\"totalTokenCount\":5,",
            "\"cachedContentTokenCount\":1}}\n\n",
        ))
        .await;

        assert_eq!(done.text, "hello");
        let usage = done.usage.unwrap();
        assert_eq!((usage.prompt_tokens, usage.completion_tokens, usage.total_tokens), (3, 2, 5));
        assert_eq!(usage.cached_tokens, 1);
    }

    #[tokio::test]
    async fn thought_parts_are_reasoning_not_reply_text() {
        let (done, reasoning) = ask(concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hmm\",\"thought\":true}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n",
        ))
        .await;

        assert_eq!(reasoning, ["hmm"], "a thought is reported as it arrives");
        assert_eq!(done.text, "hi", "a thought is not part of the reply");
        assert!(done.reasoning.is_empty(), "an unsigned thought is not kept for the next turn");
    }

    #[tokio::test]
    async fn a_part_carrying_a_signature_is_kept_whole() {
        let (done, _) = ask(concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hmm\",\"thought\":true,",
            "\"thoughtSignature\":\"SIG\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\",\"thoughtSignature\":\"TAIL\"}]}}]}\n\n",
        ))
        .await;

        assert_eq!(
            done.reasoning,
            vec![
                Reasoning::Google(serde_json::json!({ "text": "hmm", "thought": true, "thoughtSignature": "SIG" })),
                Reasoning::Google(serde_json::json!({ "text": "", "thoughtSignature": "TAIL" })),
            ],
            "a signature is opaque and rides the part it arrived on, even an empty one"
        );
    }

    #[tokio::test]
    async fn a_missing_or_unremarkable_finish_reason_is_not_a_cut_off() {
        let (done, _) = ask(concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" there\"}]},\"finishReason\":\"STOP\"}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"!\"}]},\"finishReason\":\"CONTINUATION\",",
            "\"continuationToken\":\"T\"}]}\n\n",
        ))
        .await;

        assert_eq!(done.text, "hi there!", "a long decode continues past CONTINUATION");
        assert_eq!(done.cut_off, None);
    }

    #[tokio::test]
    async fn each_function_call_arrives_whole() {
        let (done, _) = ask(concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read\",",
            "\"id\":\"call-1\",\"args\":{\"path\":\"a.txt\"}}}]},\"finishReason\":\"STOP\"}]}\n\n",
        ))
        .await;

        let call = &done.tool_calls[0];
        assert_eq!(call.id, "call-1");
        assert_eq!(call.function.name, "read");
        assert_eq!(call.function.arguments, "{\"path\":\"a.txt\"}");
    }

    #[tokio::test]
    async fn a_function_call_without_an_id_gets_one() {
        let (done, _) = ask(concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"a\",\"args\":{}}},",
            "{\"functionCall\":{\"name\":\"b\",\"args\":{}}}]}}]}\n\n",
        ))
        .await;

        let ids: Vec<_> = done.tool_calls.iter().map(|call| call.id.as_str()).collect();
        assert_eq!(ids, ["call_0", "call_1"], "each part is one call, in arrival order");
    }

    #[tokio::test]
    async fn hitting_the_token_limit_is_reported_as_a_cut_off() {
        let (done, _) =
            ask("data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"half\"}]},\"finishReason\":\"MAX_TOKENS\"}]}\n\n")
                .await;

        assert_eq!((done.text.as_str(), done.cut_off.as_deref()), ("half", Some("MAX_TOKENS")));
    }

    #[tokio::test]
    #[ignore = "needs a real Gemini API key in GEMINI_API_KEY or GOOGLE_API_KEY"]
    async fn a_live_model_answers() {
        let Some(key) = ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|key| !key.is_empty()))
        else {
            return;
        };
        let store = Arc::new(CredentialStore::in_memory().with_env("GOOGLE_API_KEY", &key));
        let auth = Auth::new(Provider::builtin("google").unwrap(), Protocol::Google, store);
        let client = Client::new(Config { auth, model: "gemini-2.5-flash".into() });
        let messages = [Message::system("Answer in one word."), Message::user("Say hi.")];

        let done = client
            .create_completion_stream(CreateCompletionRequest { messages: &messages, ..Default::default() }, |_| {})
            .await
            .unwrap();

        assert!(!done.text.is_empty(), "{done:?}");
        assert!(done.usage.is_some(), "{done:?}");
    }
}
