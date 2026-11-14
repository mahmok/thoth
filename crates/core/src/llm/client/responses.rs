use alloc::collections::{BTreeMap, BTreeSet};

use reqwest::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Auth;
use crate::llm::thinking::Thinking;

use super::EventStream;
use super::chat::{FunctionCall, Message, Reasoning, Role, ToolCall, ToolCallKind, Usage};

use super::{
    ClientError, CreateCompletionRequest, Delta, Llm as LlmClient, Negotiation, StreamedCompletion, ToolChoice,
    ToolSpec,
};

#[derive(Debug)]
/// Settings of the Responses client.
pub struct Config {
    pub auth: Auth,
    pub model: String,
}

#[derive(Debug)]
pub struct Client {
    /// See [`new_cache_key`](super::new_cache_key).
    cache_key: String,
    config: Config,
    negotiation: Negotiation,
}

impl Client {
    /// The request body for `prompt` with a reasoning setting.
    fn body<'request>(
        &'request self,
        prompt: &'request Prompt<'request>,
        tools: &'request [ToolSpec],
        choice: ToolChoice,
        reasoning: Setting,
    ) -> Body<'request> {
        let field = reasoning.field();
        Body {
            model: &self.config.model,
            stream: true,
            store: false,
            prompt_cache_key: &self.cache_key,
            input: &prompt.input,
            // Stateless mode: the server keeps nothing, so reasoning comes back to us
            // (encrypted) and is sent again with the next request.
            include: field.is_some().then_some(["reasoning.encrypted_content"]),
            reasoning: field,
            instructions: prompt.instructions.as_deref(),
            tools: tools.iter().map(function_tool).collect(),
            tool_choice: choice.forbids(tools).then_some("none"),
        }
    }

    #[cfg(test)]
    fn body_json(&self, request: &CreateCompletionRequest<'_>, reasoning: Setting) -> Value {
        let prompt = Prompt::new(request.messages);
        serde_json::to_value(self.body(&prompt, request.tools, request.tool_choice, reasoning)).unwrap()
    }

    #[must_use]
    #[inline]
    pub fn new(config: Config) -> Self {
        Self { config, negotiation: Negotiation::default(), cache_key: super::new_cache_key() }
    }

    /// POSTs the request, trying reasoning settings until the model accepts one.
    async fn post(&self, request: &CreateCompletionRequest<'_>) -> Result<Response, ClientError> {
        let settings = Setting::to_try(request.thinking);
        let owned_prompt = Prompt::new(request.messages);
        let prompt = &owned_prompt;
        self.negotiation
            .send(request.thinking, &settings, |chosen| {
                let reasoning = *chosen;
                async move {
                    self.config
                        .auth
                        .post_json(
                            "/responses",
                            &[],
                            request.messages,
                            &self.body(prompt, request.tools, request.tool_choice, reasoning),
                        )
                        .await
                }
            })
            .await
    }
}

/// The history translated once per request: only the reasoning setting changes between the
/// attempts of a negotiation.
struct Prompt<'request> {
    input: Vec<Input<'request>>,
    instructions: Option<String>,
}

impl<'request> Prompt<'request> {
    fn new(history: &'request [Message]) -> Self {
        let (instructions, input) = translate(history);
        Prompt { input, instructions }
    }
}

/// The `/responses` request. It borrows the history, so sending it copies nothing first.
#[derive(Serialize)]
struct Body<'request> {
    #[serde(skip_serializing_if = "Option::is_none")]
    include: Option<[&'static str; 1]>,
    input: &'request [Input<'request>],
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'request str>,
    model: &'request str,
    prompt_cache_key: &'request str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
    store: bool,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool<'request>>,
}

#[derive(Serialize)]
#[serde(untagged)]
/// An input item.
enum Input<'request> {
    Call {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: &'request str,
        name: &'request str,
        arguments: &'request str,
    },
    /// A reasoning item, sent back exactly as received.
    Kept(&'request Value),
    Message {
        role: &'static str,
        content: &'request str,
    },
    Output {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: Option<&'request str>,
        output: &'request str,
    },
}

#[derive(Serialize)]
/// A Tool in the wire format.
struct WireTool<'request> {
    description: &'request str,
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'request str,
    parameters: &'request Value,
}

impl LlmClient for Client {
    #[inline]
    async fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        mut on_delta: F,
    ) -> Result<StreamedCompletion, ClientError>
    where
        F: FnMut(Delta<'_>) + Send,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A reasoning setting to try.
enum Setting {
    Effort(&'static str),
    Nothing,
    SummaryOnly,
}

impl Setting {
    fn field(self) -> Option<Value> {
        match self {
            Self::Effort(effort) => Some(json!({ "effort": effort, "summary": "auto" })),
            Self::SummaryOnly => Some(json!({ "summary": "auto" })),
            Self::Nothing => None,
        }
    }

    fn to_try(thinking: Thinking) -> Vec<Self> {
        super::openai::effort_words(thinking)
            .iter()
            .map(|effort| Self::Effort(effort))
            .chain([Self::SummaryOnly, Self::Nothing])
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
/// A streamed event.
enum Event {
    #[serde(rename = "response.function_call_arguments.delta")]
    ArgumentsDelta { output_index: usize, delta: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "response.failed")]
    Failed { response: Failed },
    #[serde(rename = "response.completed", alias = "response.incomplete")]
    Finished { response: Finished },
    #[serde(rename = "response.output_item.added")]
    ItemAdded { output_index: usize, item: Item },
    #[serde(rename = "response.output_item.done")]
    ItemDone { output_index: usize, item: Value },
    #[serde(rename = "response.reasoning_summary_text.delta", alias = "response.reasoning_text.delta")]
    ReasoningDelta { delta: String },
    #[serde(rename = "response.reasoning_summary_part.added")]
    SummaryPartAdded {
        #[serde(default)]
        summary_index: usize,
    },
    #[serde(rename = "response.output_text.delta")]
    TextDelta {
        #[serde(default)]
        output_index: usize,
        delta: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
/// An output item.
enum Item {
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: String,
    },
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        content: Vec<Part>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
/// A part of an item.
enum Part {
    #[serde(rename = "output_text")]
    Text { text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
/// A finished response.
struct Finished {
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
/// Why a response is incomplete.
struct IncompleteDetails {
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
/// Token counts in the wire format.
struct WireUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    input_tokens_details: InputDetails,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

#[derive(Default, Deserialize)]
/// Details of the input tokens.
struct InputDetails {
    #[serde(default)]
    cached_tokens: u32,
}

#[derive(Deserialize)]
/// A failed response.
struct Failed {
    #[serde(default)]
    error: Option<FailedError>,
}

#[derive(Deserialize)]
/// The error of a failed response.
struct FailedError {
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
    calls: BTreeMap<usize, PartialCall>,
    cut_off: Option<String>,
    reasoning: Vec<Reasoning>,
    /// Output items whose text already arrived as deltas.
    streamed: BTreeSet<usize>,
    text: String,
    usage: Option<Usage>,
}

impl StreamState {
    /// Applies one streamed event to the state, reporting text and reasoning as it arrives.
    fn apply(&mut self, data: &str, on_delta: &mut impl FnMut(Delta<'_>)) -> Result<(), ClientError> {
        match serde_json::from_str::<Event>(data)? {
            Event::ItemAdded { output_index, item: Item::FunctionCall { call_id, name, arguments } } => {
                drop(self.calls.insert(output_index, PartialCall { id: call_id, name, arguments }));
            },
            Event::ItemDone { output_index, item } => self.item_done(output_index, item, on_delta),
            Event::ReasoningDelta { delta } => {
                if !delta.is_empty() {
                    on_delta(Delta::Reasoning(&delta));
                }
            },
            // Summary parts are separate paragraphs.
            Event::SummaryPartAdded { summary_index } if summary_index > 0 => {
                on_delta(Delta::Reasoning("\n\n"));
            },
            Event::TextDelta { output_index, delta } => {
                if !delta.is_empty() {
                    let _first_time = self.streamed.insert(output_index);
                    on_delta(Delta::Text(&delta));
                    self.text.push_str(&delta);
                }
            },
            Event::ArgumentsDelta { output_index, delta } => {
                if let Some(call) = self.calls.get_mut(&output_index) {
                    call.arguments.push_str(&delta);
                }
            },
            Event::Finished { response } => {
                if response.status.as_deref() == Some("incomplete") {
                    let reason = response.incomplete_details.map(|details| details.reason).unwrap_or_default();
                    self.cut_off = Some(reason);
                }
                self.usage = response.usage.map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: if u.total_tokens > 0 {
                        u.total_tokens
                    } else {
                        u.input_tokens.saturating_add(u.output_tokens)
                    },
                    cached_tokens: u.input_tokens_details.cached_tokens,
                });
            },
            Event::Failed { response } => {
                let message = response.error.map_or_else(|| "the response failed".to_owned(), |e| e.message);
                return Err(ClientError::Api(message));
            },
            Event::Error { message } => return Err(ClientError::Api(message)),
            Event::ItemAdded { .. } | Event::SummaryPartAdded { .. } | Event::Unknown => {},
        }
        Ok(())
    }

    /// The completion the stream produced.
    fn finish(self) -> StreamedCompletion {
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
            usage: self.usage,
            tool_calls,
            reasoning: self.reasoning,
            cut_off: self.cut_off,
        }
    }

    /// A finished output item. Function calls carry their complete arguments, so they
    /// win over the streamed deltas. Messages are a negotiation for servers that send no
    /// text deltas (without it the reply would be empty and the turn dropped).
    /// Reasoning items are kept whole to be sent back next time.
    fn item_done(&mut self, output_index: usize, item: Value, on_delta: &mut impl FnMut(Delta<'_>)) {
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            self.reasoning.push(Reasoning::Responses(item));
            return;
        }
        match serde_json::from_value::<Item>(item) {
            Ok(Item::FunctionCall { call_id, name, arguments }) => {
                drop(self.calls.insert(output_index, PartialCall { id: call_id, name, arguments }));
            },
            Ok(Item::Message { content }) if !self.streamed.contains(&output_index) => {
                for part in content {
                    if let Part::Text { text } = part
                        && !text.is_empty()
                    {
                        on_delta(Delta::Text(&text));
                        self.text.push_str(&text);
                    }
                }
            },
            _ => {},
        }
    }
}

fn function_tool(spec: &ToolSpec) -> WireTool<'_> {
    WireTool { kind: "function", name: &spec.name, description: &spec.description, parameters: &spec.parameters }
}

/// Converts messages to instructions and input items.
fn translate(messages: &[Message]) -> (Option<String>, Vec<Input<'_>>) {
    let mut instructions: Vec<&str> = Vec::new();
    let mut input: Vec<Input<'_>> = Vec::new();

    for message in messages {
        let text = message.content.as_deref().filter(|t| !t.is_empty());
        match message.role {
            Role::System => instructions.extend(text),
            Role::User => {
                if let Some(content) = text {
                    input.push(Input::Message { role: "user", content });
                }
            },
            Role::Assistant => {
                input.extend(message.reasoning.iter().filter_map(|kept| match *kept {
                    Reasoning::Responses(ref item) => Some(Input::Kept(item)),
                    Reasoning::Anthropic(_) | Reasoning::Google(_) => None,
                }));
                if let Some(content) = text {
                    input.push(Input::Message { role: "assistant", content });
                }
                input.extend(message.tool_calls.iter().map(|call| Input::Call {
                    kind: "function_call",
                    call_id: &call.id,
                    name: &call.function.name,
                    arguments: &call.function.arguments,
                }));
            },
            Role::Tool => input.push(Input::Output {
                kind: "function_call_output",
                call_id: message.tool_call_id.as_deref(),
                output: message.content.as_deref().unwrap_or_default(),
            }),
        }
    }

    let combined = (!instructions.is_empty()).then(|| instructions.join("\n\n"));
    (combined, input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall { name: "read_file".into(), arguments: arguments.into() },
        }
    }

    fn stream(events: &[Value]) -> (StreamedCompletion, Vec<String>) {
        let mut state = StreamState::default();
        let mut deltas = Vec::new();
        for event in events {
            state
                .apply(&event.to_string(), &mut |d| {
                    if let Delta::Text(text) = d {
                        deltas.push(text.to_owned());
                    }
                })
                .unwrap();
        }
        (state.finish(), deltas)
    }

    #[test]
    fn tools_can_be_forbidden_for_one_request() {
        use alloc::sync::Arc;

        use crate::auth::store::Credentials as CredentialStore;
        use crate::llm::provider::{Protocol, Provider};

        let auth = Auth::new(Provider::ollama(), Protocol::Responses, Arc::new(CredentialStore::in_memory()));
        let client = Client::new(Config { model: "m".into(), auth });
        let tools = [ToolSpec { name: "t".into(), description: "d".into(), parameters: json!({ "type": "object" }) }];
        let ask = |listed, tool_choice| CreateCompletionRequest { tools: listed, tool_choice, ..Default::default() };
        let body = client.body_json(&ask(&tools, ToolChoice::None), Setting::Nothing);
        assert_eq!(body["tool_choice"], "none");
        assert_eq!(body["tools"][0]["name"], "t", "the definitions stay");
        let automatic = client.body_json(&ask(&tools, ToolChoice::Auto), Setting::Nothing);
        assert!(automatic.get("tool_choice").is_none());
        let bare = client.body_json(&ask(&[], ToolChoice::None), Setting::Nothing);
        assert!(bare.get("tool_choice").is_none());
    }

    #[test]
    fn every_request_of_a_client_carries_the_same_cache_key() {
        use alloc::sync::Arc;

        use crate::auth::store::Credentials as CredentialStore;
        use crate::llm::provider::{Protocol, Provider};

        let auth = Auth::new(Provider::ollama(), Protocol::Responses, Arc::new(CredentialStore::in_memory()));
        let client = Client::new(Config { model: "m".into(), auth });
        let request = CreateCompletionRequest::default();
        let first = client.body_json(&request, Setting::Nothing);
        let second = client.body_json(&request, Setting::Nothing);
        assert!(first["prompt_cache_key"].as_str().is_some_and(|key| key.starts_with(crate::APP_NAME)));
        assert_eq!(first["prompt_cache_key"], second["prompt_cache_key"]);
    }

    #[test]
    fn system_messages_become_instructions_and_the_rest_input_items() {
        let messages = vec![
            Message::system("be brief"),
            Message::user("hi"),
            Message::assistant(Some("looking".into()), vec![call("c1", "{\"path\":\"a\"}")]),
            Message::tool("c1", "contents"),
            Message::assistant(Some("done".into()), Vec::new()),
        ];
        let (instructions, input) = translate(&messages);
        assert_eq!(instructions.as_deref(), Some("be brief"));
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            json!([
                json!({ "role": "user", "content": "hi" }),
                json!({ "role": "assistant", "content": "looking" }),
                json!({ "type": "function_call", "call_id": "c1", "name": "read_file", "arguments": "{\"path\":\"a\"}" }),
                json!({ "type": "function_call_output", "call_id": "c1", "output": "contents" }),
                json!({ "role": "assistant", "content": "done" }),
            ])
        );
    }

    #[test]
    fn no_system_message_means_no_instructions() {
        let messages = [Message::user("hi")];
        let (instructions, input) = translate(&messages);
        assert_eq!(instructions, None);
        assert_eq!(input.len(), 1);
    }

    #[test]
    fn a_level_is_tried_as_effort_then_summary_only_then_nothing() {
        assert_eq!(
            Setting::to_try(Thinking::Max),
            [Setting::Effort("xhigh"), Setting::Effort("high"), Setting::SummaryOnly, Setting::Nothing]
        );
        assert_eq!(Setting::Effort("high").field(), Some(json!({ "effort": "high", "summary": "auto" })));
        assert_eq!(Setting::Nothing.field(), None);
    }

    #[test]
    fn describes_tools_in_the_flat_responses_shape() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({ "type": "object" }),
        };
        let value = serde_json::to_value(function_tool(&spec)).unwrap();
        assert_eq!(value["type"], "function");
        assert_eq!(value["name"], "read_file");
        assert_eq!(value["parameters"]["type"], "object");
    }

    #[test]
    fn streamed_text_and_usage_are_collected() {
        let (done, deltas) = stream(&[
            json!({ "type": "response.created", "response": {} }),
            json!({ "type": "response.output_text.delta", "delta": "Hel" }),
            json!({ "type": "response.output_text.delta", "delta": "lo" }),
            json!({ "type": "response.completed", "response": {
                "usage": { "input_tokens": 3i32, "output_tokens": 2i32, "total_tokens": 5i32,
                    "input_tokens_details": { "cached_tokens": 2i32 } } } }),
        ]);
        assert_eq!(done.text, "Hello");
        assert_eq!(deltas, ["Hel", "lo"]);
        let usage = done.usage.unwrap();
        assert_eq!((usage.total_tokens, usage.cached_tokens), (5, 2));
        assert!(done.tool_calls.is_empty());
    }

    #[test]
    fn function_calls_are_assembled_from_argument_deltas() {
        let (done, _) = stream(&[
            json!({ "type": "response.output_item.added", "output_index": 0i32,
                "item": { "type": "reasoning", "id": "rs_1" } }),
            json!({ "type": "response.output_item.added", "output_index": 1i32,
                "item": { "type": "function_call", "call_id": "call_1", "name": "read_file", "arguments": "" } }),
            json!({ "type": "response.function_call_arguments.delta", "output_index": 1i32, "delta": "{\"path\":" }),
            json!({ "type": "response.function_call_arguments.delta", "output_index": 1i32, "delta": "\"a\"}" }),
        ]);
        assert_eq!(done.tool_calls.len(), 1);
        assert_eq!(done.tool_calls[0].id, "call_1");
        assert_eq!(done.tool_calls[0].function.name, "read_file");
        assert_eq!(done.tool_calls[0].function.arguments, "{\"path\":\"a\"}");
    }

    #[test]
    fn the_finished_item_replaces_streamed_arguments() {
        let (done, _) = stream(&[
            json!({ "type": "response.output_item.added", "output_index": 0i32,
                "item": { "type": "function_call", "call_id": "c", "name": "t", "arguments": "" } }),
            json!({ "type": "response.function_call_arguments.delta", "output_index": 0i32, "delta": "{" }),
            json!({ "type": "response.output_item.done", "output_index": 0i32,
                "item": { "type": "function_call", "call_id": "c", "name": "t", "arguments": "{\"a\":1}" } }),
        ]);
        assert_eq!(done.tool_calls[0].function.arguments, "{\"a\":1}");
    }

    #[test]
    fn text_that_only_arrives_in_the_finished_message_is_kept() {
        let (done, deltas) = stream(&[json!({
            "type": "response.output_item.done", "output_index": 0i32,
            "item": { "type": "message", "content": [{ "type": "output_text", "text": "Hi there" }] }
        })]);
        assert_eq!(done.text, "Hi there");
        assert_eq!(deltas, ["Hi there"]);
    }

    #[test]
    fn streamed_text_is_not_repeated_by_the_finished_message() {
        let (done, deltas) = stream(&[
            json!({ "type": "response.output_text.delta", "output_index": 0i32, "delta": "Hi" }),
            json!({ "type": "response.output_item.done", "output_index": 0i32,
                "item": { "type": "message", "content": [{ "type": "output_text", "text": "Hi" }] } }),
        ]);
        assert_eq!(done.text, "Hi");
        assert_eq!(deltas, ["Hi"]);
    }

    #[test]
    fn reasoning_items_are_kept_and_sent_back_before_the_reply() {
        let reasoning = json!({ "type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "abc" });
        let (done, _) = stream(&[json!({
            "type": "response.output_item.done", "output_index": 0i32, "item": reasoning
        })]);
        assert_eq!(done.reasoning, vec![Reasoning::Responses(reasoning.clone())]);

        let messages = vec![
            Message::user("hi"),
            Message::assistant(Some("hello".into()), Vec::new()).with_reasoning(done.reasoning),
        ];
        let (_, translated) = translate(&messages);
        let input = serde_json::to_value(&translated).unwrap();
        assert_eq!(input[1], reasoning);
        assert_eq!(input[2], json!({ "role": "assistant", "content": "hello" }));
    }

    #[test]
    fn reasoning_summaries_stream_apart_from_the_reply() {
        let mut state = StreamState::default();
        let mut seen: Vec<(bool, String)> = Vec::new();
        for event in [
            json!({ "type": "response.reasoning_summary_part.added", "summary_index": 0i32 }),
            json!({ "type": "response.reasoning_summary_text.delta", "delta": "Look at" }),
            json!({ "type": "response.reasoning_summary_text.delta", "delta": " the file." }),
            json!({ "type": "response.reasoning_summary_part.added", "summary_index": 1i32 }),
            json!({ "type": "response.reasoning_summary_text.delta", "delta": "Then edit." }),
            json!({ "type": "response.output_text.delta", "delta": "Done" }),
        ] {
            state
                .apply(&event.to_string(), &mut |d| match d {
                    Delta::Reasoning(t) => seen.push((true, t.to_owned())),
                    Delta::Text(t) => seen.push((false, t.to_owned())),
                })
                .unwrap();
        }
        let thinking: String = seen.iter().filter(|entry| entry.0).map(|entry| entry.1.as_str()).collect();
        assert_eq!(thinking, "Look at the file.\n\nThen edit.");
        assert_eq!(state.finish().text, "Done", "reasoning is not part of the reply");
    }

    #[test]
    fn a_reply_cut_off_while_thinking_is_an_error_not_an_empty_reply() {
        let mut state = StreamState::default();
        let thinking = json!({ "type": "response.reasoning_summary_text.delta", "delta": "hmm" });
        state.apply(&thinking.to_string(), &mut |_| {}).unwrap();
        let incomplete = json!({
            "type": "response.incomplete",
            "response": { "status": "incomplete", "incomplete_details": { "reason": "max_output_tokens" } }
        });
        state.apply(&incomplete.to_string(), &mut |_| {}).unwrap();
        let err = state.finish().fail_if_cut_off_empty().err().unwrap();
        assert!(err.to_string().contains("max_output_tokens"), "{err}");
    }

    #[test]
    fn failures_surface_as_api_errors() {
        let mut state = StreamState::default();
        let failed = json!({ "type": "response.failed", "response": { "error": { "message": "boom" } } });
        let err = state.apply(&failed.to_string(), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
        let error = json!({ "type": "error", "message": "bad" });
        assert!(state.apply(&error.to_string(), &mut |_| {}).is_err());
    }
}
