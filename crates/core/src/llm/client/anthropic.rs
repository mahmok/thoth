use alloc::collections::BTreeMap;

use reqwest::Response;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::auth::Auth;
use crate::llm::thinking::Thinking;

use super::EventStream;
use super::chat::{FunctionCall, Message, Reasoning, Role, ToolCall, ToolCallKind, Usage};

use super::{
    ClientError, CreateCompletionRequest, Delta as Streamed, Llm as LlmClient, Negotiation, StreamedCompletion,
    ToolChoice, ToolSpec,
};

/// The Anthropic API version sent.
pub const API_VERSION: &str = "2023-06-01";

/// The cache marker for a prompt prefix.
const EPHEMERAL: CacheControl = CacheControl { kind: "ephemeral" };

#[derive(Serialize)]
/// A prompt-cache marker.
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
/// The request body.
struct Body<'request> {
    max_tokens: u32,
    messages: &'request [WireMessage<'request>],
    model: &'request str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<[SystemBlock<'request>; 1]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<WireToolChoice>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool<'request>>,
}

/// `{"type": "none"}`: the model may not call a tool. Unlike a forced choice, it is allowed
/// together with extended thinking.
#[derive(Serialize)]
struct WireToolChoice {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
/// The thinking setting of a request.
struct ThinkingField {
    budget_tokens: u32,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
/// A block of the system prompt.
struct SystemBlock<'request> {
    cache_control: CacheControl,
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'request str,
}

#[derive(Serialize)]
/// A Tool in the wire format.
struct WireTool<'request> {
    description: &'request str,
    input_schema: &'request Value,
    name: &'request str,
}

#[derive(Serialize)]
/// A message in the wire format.
struct WireMessage<'request> {
    content: Vec<WireBlock<'request>>,
    role: &'static str,
}

#[derive(Serialize)]
#[serde(untagged)]
/// A content block in the wire format.
enum WireBlock<'request> {
    Kept(&'request Value),
    Text {
        #[serde(rename = "type")]
        kind: &'static str,
        text: &'request str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        #[serde(rename = "type")]
        kind: &'static str,
        tool_use_id: Option<&'request str>,
        content: &'request str,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        #[serde(rename = "type")]
        kind: &'static str,
        id: &'request str,
        name: &'request str,
        input: &'request RawValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl<'request> WireBlock<'request> {
    const fn mark_for_cache(&mut self) {
        match *self {
            WireBlock::Text { ref mut cache_control, .. }
            | WireBlock::ToolUse { ref mut cache_control, .. }
            | WireBlock::ToolResult { ref mut cache_control, .. } => {
                *cache_control = Some(EPHEMERAL);
            },
            WireBlock::Kept(_) => {},
        }
    }

    const fn text(text: &'request str) -> Self {
        WireBlock::Text { kind: "text", text, cache_control: None }
    }

    fn tool_result(message: &'request Message) -> Self {
        WireBlock::ToolResult {
            kind: "tool_result",
            tool_use_id: message.tool_call_id.as_deref(),
            content: message.content.as_deref().unwrap_or_default(),
            is_error: message.is_error,
            cache_control: None,
        }
    }

    fn tool_use(call: &'request ToolCall) -> Self {
        WireBlock::ToolUse {
            kind: "tool_use",
            id: &call.id,
            name: &call.function.name,
            input: input_of(&call.function.arguments),
            cache_control: None,
        }
    }
}

#[derive(Debug)]
/// Settings of the Anthropic client.
pub struct Config {
    pub auth: Auth,
    pub max_tokens: u32,
    pub model: String,
}

#[derive(Debug)]
/// The Anthropic Messages client.
pub struct Client {
    config: Config,
    negotiation: Negotiation,
}

/// A history translated once per request.
struct Prompt<'request> {
    messages: Vec<WireMessage<'request>>,
    system: Option<String>,
}

impl<'request> Prompt<'request> {
    fn new(history: &'request [Message]) -> Self {
        let (system, mut messages) = translate(history);
        cache_up_to_last_block(&mut messages);
        Prompt { messages, system }
    }
}

impl Client {
    /// The request body for `prompt`.
    fn body<'request>(
        &'request self,
        prompt: &'request Prompt<'request>,
        tools: &'request [ToolSpec],
        choice: ToolChoice,
        budget: Option<u32>,
    ) -> Body<'request> {
        Body {
            model: &self.config.model,
            max_tokens: self.config.max_tokens.saturating_add(budget.unwrap_or(0)),
            stream: true,
            messages: &prompt.messages,
            thinking: budget.map(|budget_tokens| ThinkingField { kind: "enabled", budget_tokens }),
            system: prompt.system.as_deref().map(|text| [SystemBlock { kind: "text", text, cache_control: EPHEMERAL }]),
            tools: tools.iter().map(tool_definition).collect(),
            tool_choice: choice.forbids(tools).then_some(WireToolChoice { kind: "none" }),
        }
    }

    #[cfg(test)]
    fn body_json(&self, request: &CreateCompletionRequest<'_>, budget: Option<u32>) -> Value {
        let prompt = Prompt::new(request.messages);
        serde_json::to_value(self.body(&prompt, request.tools, request.tool_choice, budget)).unwrap()
    }

    #[must_use]
    #[inline]
    pub fn new(config: Config) -> Self {
        Self { config, negotiation: Negotiation::default() }
    }

    /// POSTs the request, trying thinking budgets until the model accepts one.
    async fn post(&self, request: &CreateCompletionRequest<'_>) -> Result<Response, ClientError> {
        let budgets: Vec<Option<u32>> = budgets(request.thinking).iter().copied().map(Some).chain([None]).collect();
        let owned_prompt = Prompt::new(request.messages);
        let prompt = &owned_prompt;
        self.negotiation
            .send(request.thinking, &budgets, |chosen| {
                let budget = *chosen;
                async move {
                    self.config
                        .auth
                        .post_json(
                            "/v1/messages",
                            &[("anthropic-version", API_VERSION)],
                            request.messages,
                            &self.body(prompt, request.tools, request.tool_choice, budget),
                        )
                        .await
                }
            })
            .await
    }
}

impl LlmClient for Client {
    #[inline]
    async fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        mut on_delta: F,
    ) -> Result<StreamedCompletion, ClientError>
    where
        F: FnMut(Streamed<'_>) + Send,
    {
        let response = self.post(&request).await?;
        let mut events = EventStream::new(response);
        let mut stream = StreamState::default();
        while let Some(data) = events.next().await? {
            stream.apply(&data, &mut on_delta)?;
        }
        stream.finish().fail_if_cut_off_empty()
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// A streamed event.
enum Event {
    ContentBlockDelta {
        index: usize,
        delta: Delta,
    },
    ContentBlockStart {
        index: usize,
        content_block: Block,
    },
    Error {
        error: ApiError,
    },
    MessageDelta {
        #[serde(default)]
        delta: StopDelta,
        #[serde(default)]
        usage: WireUsage,
    },
    MessageStart {
        message: StartMessage,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
/// The start of a message.
struct StartMessage {
    #[serde(default)]
    usage: WireUsage,
}

#[derive(Default, Deserialize)]
/// Token counts in the wire format.
#[expect(clippy::struct_field_names, reason = "the names follow the provider's wire format or what the type wraps")]
struct WireUsage {
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

impl WireUsage {
    const fn prompt_tokens(&self) -> u32 {
        self.input_tokens.saturating_add(self.cache_read_input_tokens).saturating_add(self.cache_creation_input_tokens)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// A content block of a stream.
enum Block {
    RedactedThinking {
        #[serde(default)]
        data: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// A piece of a content block.
enum Delta {
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(other)]
    Unknown,
}

#[derive(Default, Deserialize)]
/// Why a message stopped.
struct StopDelta {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
/// An error from the API.
struct ApiError {
    message: String,
}

/// A Tool call being assembled.
struct PartialCall {
    arguments: String,
    id: String,
    name: String,
}

#[derive(Default)]
/// What a stream has produced so far.
struct StreamState {
    cached_tokens: u32,
    calls: BTreeMap<usize, PartialCall>,
    cut_off: Option<String>,
    input_tokens: u32,
    output_tokens: u32,
    text: String,
    thinking_blocks: BTreeMap<usize, Value>,
}

impl StreamState {
    /// Applies one streamed event to the state, reporting text and reasoning as it arrives.
    fn apply(&mut self, data: &str, on_delta: &mut impl FnMut(Streamed<'_>)) -> Result<(), ClientError> {
        match serde_json::from_str::<Event>(data)? {
            Event::MessageStart { message } => self.record_usage(&message.usage),
            Event::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason.filter(|r| r == "max_tokens") {
                    self.cut_off = Some(reason);
                }
                self.record_usage(&usage);
            },
            Event::ContentBlockStart { index, content_block: Block::ToolUse { id, name, input } } => {
                let arguments = match input {
                    Value::Object(ref map) if !map.is_empty() => input.to_string(),
                    Value::Null
                    | Value::Bool(_)
                    | Value::Number(_)
                    | Value::String(_)
                    | Value::Array(_)
                    | Value::Object(_) => String::new(),
                };
                drop(self.calls.insert(index, PartialCall { arguments, id, name }));
            },
            Event::ContentBlockDelta { delta: Delta::Text { text }, .. } => {
                if !text.is_empty() {
                    on_delta(Streamed::Text(&text));
                    self.text.push_str(&text);
                }
            },
            Event::ContentBlockStart { index, content_block: Block::Thinking { thinking, signature } } => {
                drop(
                    self.thinking_blocks
                        .insert(index, json!({ "type": "thinking", "thinking": thinking, "signature": signature })),
                );
            },
            Event::ContentBlockStart { index, content_block: Block::RedactedThinking { data: redacted } } => {
                drop(self.thinking_blocks.insert(index, json!({ "type": "redacted_thinking", "data": redacted })));
            },
            Event::ContentBlockDelta { index, delta: Delta::Thinking { thinking } } => {
                if !thinking.is_empty() {
                    on_delta(Streamed::Reasoning(&thinking));
                    if let Some(&mut Value::String(ref mut text)) =
                        self.thinking_blocks.get_mut(&index).map(|block| &mut block["thinking"])
                    {
                        text.push_str(&thinking);
                    }
                }
            },
            Event::ContentBlockDelta { index, delta: Delta::Signature { signature } } => {
                if let Some(block) = self.thinking_blocks.get_mut(&index) {
                    block["signature"] = Value::String(signature);
                }
            },
            Event::ContentBlockDelta { index, delta: Delta::InputJson { partial_json } } => {
                if let Some(call) = self.calls.get_mut(&index) {
                    call.arguments.push_str(&partial_json);
                }
            },
            Event::Error { error } => return Err(ClientError::Api(error.message)),
            Event::ContentBlockStart { .. } | Event::ContentBlockDelta { .. } | Event::Unknown => {},
        }
        Ok(())
    }

    /// The completion the stream produced.
    fn finish(self) -> StreamedCompletion {
        let usage = (self.input_tokens.saturating_add(self.output_tokens) > 0).then_some(Usage {
            prompt_tokens: self.input_tokens,
            completion_tokens: self.output_tokens,
            total_tokens: self.input_tokens.saturating_add(self.output_tokens),
            cached_tokens: self.cached_tokens,
        });
        let tool_calls = self
            .calls
            .into_values()
            .map(|call| ToolCall {
                id: call.id,
                kind: ToolCallKind::Function,
                function: FunctionCall { name: call.name, arguments: call.arguments },
            })
            .collect();
        StreamedCompletion {
            text: self.text,
            usage,
            tool_calls,
            reasoning: self.thinking_blocks.into_values().map(Reasoning::Anthropic).collect(),
            cut_off: self.cut_off,
        }
    }

    fn record_usage(&mut self, usage: &WireUsage) {
        self.input_tokens = self.input_tokens.max(usage.prompt_tokens());
        self.cached_tokens = self.cached_tokens.max(usage.cache_read_input_tokens);
        self.output_tokens = self.output_tokens.max(usage.output_tokens);
    }
}

#[expect(clippy::expect_used, reason = "`{}` is valid JSON")]
fn input_of(arguments: &str) -> &RawValue {
    serde_json::from_str::<&RawValue>(arguments)
        .unwrap_or_else(|_| serde_json::from_str("{}").expect("an empty object is JSON"))
}

fn cache_up_to_last_block(messages: &mut [WireMessage<'_>]) {
    let last_block = messages.last_mut().and_then(|message| message.content.last_mut());
    if let Some(block) = last_block {
        block.mark_for_cache();
    }
}

const fn budgets(thinking: Thinking) -> &'static [u32] {
    match thinking {
        Thinking::Off => &[],
        Thinking::Minimal => &[1024],
        Thinking::Low => &[2048],
        Thinking::Medium => &[8192],
        Thinking::High => &[0x4000],
        Thinking::Max => &[32_000, 0x4000],
    }
}

fn tool_definition(spec: &ToolSpec) -> WireTool<'_> {
    WireTool { name: &spec.name, description: &spec.description, input_schema: &spec.parameters }
}

/// Converts messages to the wire format: a system prompt and alternating turns.
fn translate(messages: &[Message]) -> (Option<String>, Vec<WireMessage<'_>>) {
    let mut system: Vec<&str> = Vec::new();
    let mut turns: Vec<WireMessage<'_>> = Vec::new();

    for message in messages {
        let text = message.content.as_deref().filter(|t| !t.is_empty());
        match message.role {
            Role::System => system.extend(text),
            Role::User => {
                let blocks = text.map(WireBlock::text).into_iter().collect();
                push_turn(&mut turns, "user", blocks);
            },
            Role::Assistant => {
                // Thinking blocks go back first, exactly as received: tool turns need them.
                let mut blocks: Vec<WireBlock<'_>> = message
                    .reasoning
                    .iter()
                    .filter_map(|kept| match *kept {
                        Reasoning::Anthropic(ref block) => Some(WireBlock::Kept(block)),
                        Reasoning::Responses(_) => None,
                    })
                    .collect();
                blocks.extend(text.map(WireBlock::text));
                blocks.extend(message.tool_calls.iter().map(WireBlock::tool_use));
                push_turn(&mut turns, "assistant", blocks);
            },
            Role::Tool => push_turn(&mut turns, "user", vec![WireBlock::tool_result(message)]),
        }
    }

    let system_prompt = (!system.is_empty()).then(|| system.join("\n\n"));
    (system_prompt, turns)
}

fn push_turn<'request>(turns: &mut Vec<WireMessage<'request>>, role: &'static str, blocks: Vec<WireBlock<'request>>) {
    if blocks.is_empty() {
        return;
    }
    match turns.last_mut() {
        Some(last) if last.role == role => last.content.extend(blocks),
        _ => turns.push(WireMessage { role, content: blocks }),
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::auth::store::Credentials as CredentialStore;
    use crate::llm::provider::{ModelSpec, Protocol, Provider};

    fn client() -> Client {
        Client::new(Config {
            model: "m".into(),
            max_tokens: 99,
            auth: Auth::for_model(
                &ModelSpec::new(Provider::Ollama, Protocol::Anthropic, "m"),
                Arc::new(CredentialStore::in_memory()),
            ),
        })
    }

    fn call(id: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall { name: "read_file".into(), arguments: arguments.into() },
        }
    }

    fn call_again() -> ToolCall {
        call("c2", "{}")
    }

    fn run(events: &[&str]) -> (StreamedCompletion, Vec<String>) {
        let mut state = StreamState::default();
        let mut deltas = Vec::new();
        for event in events {
            state
                .apply(event, &mut |d| {
                    if let Streamed::Text(text) = d {
                        deltas.push(text.to_owned());
                    }
                })
                .unwrap();
        }
        (state.finish(), deltas)
    }

    #[test]
    fn thinking_deltas_are_reasoning_not_reply_text() {
        let mut state = StreamState::default();
        let mut thinking = String::new();
        state
            .apply(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
                &mut |d| {
                    if let Streamed::Reasoning(text) = d {
                        thinking.push_str(text);
                    }
                },
            )
            .unwrap();
        assert_eq!(thinking, "hmm");
        assert_eq!(state.finish().text, "");
    }

    #[test]
    fn a_reply_that_hits_the_token_limit_is_an_error_not_an_empty_reply() {
        let mut state = StreamState::default();
        state
            .apply(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
                &mut |_| {},
            )
            .unwrap();
        state
            .apply(
                r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":5}}"#,
                &mut |_| {},
            )
            .unwrap();
        let err = state.finish().fail_if_cut_off_empty().err().unwrap();
        assert!(err.to_string().contains("max_tokens"), "{err}");
    }

    #[test]
    fn a_reply_that_hits_the_token_limit_keeps_the_text_it_had() {
        let (done, _) = run(&[
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":5}}"#,
        ]);
        let completed = done.fail_if_cut_off_empty().unwrap();
        assert_eq!((completed.text.as_str(), completed.cut_off.as_deref()), ("half", Some("max_tokens")));
    }

    #[test]
    fn a_failed_tool_result_is_marked_as_an_error() {
        let call = call("c1", "{}");
        let messages = [
            Message::user("go"),
            Message::assistant(None, vec![call]),
            Message::tool("c1", "error: boom").with_error(true),
        ];
        let request = CreateCompletionRequest { messages: &messages, ..Default::default() };
        let body = client().body_json(&request, None);
        let result = &body["messages"][2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["is_error"], true);

        let fine = [Message::user("go"), Message::assistant(None, vec![call_again()]), Message::tool("c2", "ok")];
        let fine_request = CreateCompletionRequest { messages: &fine, ..Default::default() };
        let fine_body = client().body_json(&fine_request, None);
        assert!(fine_body["messages"][2]["content"][0].get("is_error").is_none());
    }

    #[test]
    fn tools_can_be_forbidden_for_one_request() {
        let messages = [Message::user("hi")];
        let tools = [ToolSpec { name: "t".into(), description: "d".into(), parameters: json!({ "type": "object" }) }];
        let ask = |listed: &'static [ToolSpec], choice| CreateCompletionRequest {
            messages: &messages,
            tools: listed,
            tool_choice: choice,
            ..Default::default()
        };
        let leaked: &'static [ToolSpec] = Box::leak(Box::new(tools));
        let body = client().body_json(&ask(leaked, ToolChoice::None), None);
        assert_eq!(body["tool_choice"], json!({ "type": "none" }));
        assert_eq!(body["tools"][0]["name"], "t", "the definitions stay");
        let auto = client().body_json(&ask(leaked, ToolChoice::Auto), None);
        assert!(auto.get("tool_choice").is_none());
        let none_without_tools = client().body_json(&ask(&[], ToolChoice::None), None);
        assert!(none_without_tools.get("tool_choice").is_none());
    }

    #[test]
    fn a_thinking_budget_enables_thinking_and_makes_room_for_the_answer() {
        let messages = [Message::user("hi")];
        let request = CreateCompletionRequest { messages: &messages, ..Default::default() };
        let body = client().body_json(&request, Some(8192));
        assert_eq!(body["thinking"], json!({ "type": "enabled", "budget_tokens": 8_192i32 }));
        assert_eq!(body["max_tokens"], 99i32 + 8_192i32);
        assert!(client().body_json(&request, None).get("thinking").is_none());
    }

    #[test]
    fn thinking_blocks_are_kept_with_their_signature_and_sent_back_first() {
        let (done, _) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" see"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"xyz"}}"#,
        ]);
        assert_eq!(
            done.reasoning,
            vec![
                Reasoning::Anthropic(json!({ "type": "thinking", "thinking": "let me see", "signature": "sig" })),
                Reasoning::Anthropic(json!({ "type": "redacted_thinking", "data": "xyz" })),
            ]
        );

        let mut reply = Message::assistant(Some("ok".into()), vec![call("t1", "{}")]);
        reply.reasoning = done.reasoning;
        reply.reasoning.push(Reasoning::Responses(json!({ "type": "reasoning", "id": "other protocol" })));
        let messages = [Message::user("go"), reply];
        let (_, translated) = translate(&messages);
        let turns = serde_json::to_value(&translated).unwrap();
        let blocks = turns[1]["content"].as_array().unwrap();
        let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["thinking", "redacted_thinking", "text", "tool_use"]);
    }

    #[test]
    fn the_system_prompt_moves_to_its_own_field() {
        let messages = [Message::system("be brief"), Message::user("hi")];
        let request = CreateCompletionRequest { messages: &messages, ..Default::default() };
        let body = client().body_json(&request, None);
        assert_eq!(body["system"][0]["text"], "be brief");
        assert_eq!(body["max_tokens"], 99i32);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn tool_steps_become_tool_use_and_tool_result_blocks() {
        let messages = vec![
            Message::user("go"),
            Message::assistant(Some("looking".into()), vec![call("a", r#"{"path":"x"}"#), call("b", "")]),
            Message::tool("a", "ra"),
            Message::tool("b", "rb"),
            Message::assistant(Some("done".into()), Vec::new()),
        ];
        let (_, translated) = translate(&messages);
        let turns = serde_json::to_value(&translated).unwrap();
        let roles: Vec<&str> = turns.as_array().unwrap().iter().map(|t| t["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);

        let assistant = &turns[1]["content"];
        assert_eq!(assistant[0]["type"], "text");
        assert_eq!(assistant[1]["type"], "tool_use");
        assert_eq!(assistant[1]["input"]["path"], "x");
        assert_eq!(assistant[2]["input"], json!({}));

        let results = turns[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2, "consecutive results share one user message");
        assert_eq!(results[0]["tool_use_id"], "a");
        assert_eq!(results[1]["content"], "rb");
    }

    #[test]
    fn tools_are_described_with_an_input_schema() {
        let spec =
            ToolSpec { name: "read_file".into(), description: "Read".into(), parameters: json!({ "type": "object" }) };
        let value = serde_json::to_value(tool_definition(&spec)).unwrap();
        assert_eq!(value["name"], "read_file");
        assert_eq!(value["input_schema"]["type"], "object");
    }

    #[test]
    fn text_streams_through_and_usage_is_summed() {
        let (done, deltas) = run(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":0}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"he"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"llo"}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        assert_eq!(deltas, ["he", "llo"]);
        assert_eq!(done.text, "hello");
        let usage = done.usage.unwrap();
        assert_eq!((usage.prompt_tokens, usage.completion_tokens, usage.total_tokens), (12, 5, 17));
        assert!(done.tool_calls.is_empty());
    }

    #[test]
    fn the_system_prompt_and_the_last_block_are_cache_breakpoints() {
        let messages = [
            Message::system("be brief"),
            Message::user("go"),
            Message::assistant(Some("looking".into()), vec![call("a", "{}")]),
            Message::tool("a", "result"),
        ];
        let request = CreateCompletionRequest { messages: &messages, ..Default::default() };
        let body = client().body_json(&request, None);
        let ephemeral = json!({ "type": "ephemeral" });
        assert_eq!(body["system"][0]["cache_control"], ephemeral);

        let turns = body["messages"].as_array().unwrap();
        let marked = turns
            .iter()
            .flat_map(|turn| turn["content"].as_array().unwrap())
            .filter(|block| block.get("cache_control").is_some())
            .count();
        assert_eq!(marked, 1, "only the newest block moves the breakpoint");
        assert_eq!(turns[2]["content"][0]["cache_control"], ephemeral);
    }

    #[test]
    fn cached_input_still_counts_as_prompt_tokens() {
        let (done, _) = run(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":800,"cache_creation_input_tokens":90,"output_tokens":0}}}"#,
            r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":5}}"#,
        ]);
        let usage = done.usage.unwrap();
        assert_eq!((usage.prompt_tokens, usage.total_tokens), (900, 905));
        assert_eq!(usage.cached_tokens, 800, "only the part read from the cache");
    }

    #[test]
    fn tool_calls_are_assembled_from_json_fragments() {
        let (done, _) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"read_file","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#,
        ]);
        assert_eq!(done.tool_calls.len(), 1);
        assert_eq!(done.tool_calls[0].id, "t1");
        assert_eq!(done.tool_calls[0].function.name, "read_file");
        assert_eq!(done.tool_calls[0].function.arguments, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn a_complete_input_on_the_block_start_is_kept() {
        let (done, _) = run(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"n","input":{"a":1}}}"#,
        ]);
        assert_eq!(done.tool_calls[0].function.arguments, r#"{"a":1}"#);
    }

    #[test]
    fn api_errors_are_reported() {
        let mut state = StreamState::default();
        let err = state
            .apply(r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#, &mut |_| {})
            .unwrap_err();
        assert_eq!(err.to_string(), "api error: busy");
    }
}
