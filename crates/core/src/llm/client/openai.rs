use reqwest::Response;
use serde::Serialize;
use serde_json::{Value, json};

use crate::auth::Auth;
use crate::llm::thinking::Thinking;

use super::EventStream;
use super::chat::{CompletionChunk as ChatCompletionChunk, Message, ToolCallAssembler};
use super::{ClientError, CreateCompletionRequest, Delta, Llm as LlmClient, Negotiation, StreamedCompletion, ToolSpec};

#[derive(Debug)]
pub struct OpenAiConfig {
    pub auth: Auth,
    pub model: String,
}

#[derive(Debug)]
pub struct OpenAiClient {
    /// See [`new_cache_key`](super::new_cache_key).
    cache_key: String,
    config: OpenAiConfig,
    negotiation: Negotiation,
}

impl OpenAiClient {
    #[must_use]
    #[inline]
    pub fn new(config: OpenAiConfig) -> Self {
        Self { config, negotiation: Negotiation::default(), cache_key: super::new_cache_key() }
    }

    async fn post(&self, request: &CreateCompletionRequest<'_>, stream: bool) -> Result<Response, ClientError> {
        let efforts = efforts(request.thinking);
        self.negotiation.send(request.thinking, &efforts, |effort| self.post_with(request, stream, *effort)).await
    }

    /// POSTs the request with a reasoning effort, streaming or not.
    async fn post_with(
        &self,
        request: &CreateCompletionRequest<'_>,
        stream: bool,
        effort: Option<&'static str>,
    ) -> Result<Response, ClientError> {
        let body = Body {
            model: &self.config.model,
            messages: request.messages,
            stream,
            stream_options: StreamOptions { include_usage: stream },
            prompt_cache_key: &self.cache_key,
            reasoning_effort: effort,
            tools: request.tools.iter().map(function_tool).collect(),
            tool_choice: request.tool_choice.forbids(request.tools).then_some("none"),
        };
        self.config.auth.post_json("/chat/completions", &[], request.messages, &body).await
    }
}

/// The `/chat/completions` request. It borrows the history, so sending it copies nothing first.
#[derive(Serialize)]
struct Body<'request> {
    messages: &'request [Message],
    model: &'request str,
    prompt_cache_key: &'request str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
    stream: bool,
    stream_options: StreamOptions,
    /// Only sent with tools: a server may refuse a choice when there is nothing to choose from.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
}

#[derive(Serialize)]
/// Options for a streamed request.
struct StreamOptions {
    include_usage: bool,
}

impl LlmClient for OpenAiClient {
    #[inline]
    async fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        mut on_delta: F,
    ) -> Result<StreamedCompletion, ClientError>
    where
        F: FnMut(Delta<'_>) + Send,
    {
        let response = self.post(&request, true).await?;
        let mut events = EventStream::new(response);
        let mut full_text = String::new();
        let mut usage = None;
        let mut cut_off = None;
        let mut tool_calls = ToolCallAssembler::default();

        while let Some(data) = events.next().await? {
            if data == "[DONE]" {
                break;
            }
            let chunk: ChatCompletionChunk = serde_json::from_str(&data)?;
            if chunk.usage.is_some() {
                usage = chunk.usage;
            }
            for choice in chunk.choices {
                // Servers name the reasoning field differently (`reasoning_content`, `reasoning`).
                let reasoning = choice.delta.reasoning_content.or(choice.delta.reasoning);
                if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
                    on_delta(Delta::Reasoning(&text));
                }
                if let Some(text) = choice.delta.content.filter(|t| !t.is_empty()) {
                    on_delta(Delta::Text(&text));
                    full_text.push_str(&text);
                }
                for delta in choice.delta.tool_calls {
                    tool_calls.push(delta);
                }
                if choice.finish_reason.as_deref() == Some("length") {
                    cut_off = Some("length".to_owned());
                }
            }
        }

        StreamedCompletion { text: full_text, usage, tool_calls: tool_calls.finish(), cut_off, ..Default::default() }
            .fail_if_cut_off_empty()
    }
}

/// one is tried with the next. Shared with the Responses client, which speaks the same words.
pub(super) const fn effort_words(thinking: Thinking) -> &'static [&'static str] {
    match thinking {
        Thinking::Off => &["none", "minimal"],
        Thinking::Minimal => &["minimal", "low"],
        Thinking::Low => &["low"],
        Thinking::Medium => &["medium"],
        Thinking::High => &["high"],
        Thinking::Max => &["xhigh", "high"],
    }
}

/// The `reasoning_effort` settings to try for a level, ending with none at all.
fn efforts(thinking: Thinking) -> Vec<Option<&'static str>> {
    effort_words(thinking).iter().copied().map(Some).chain([None]).collect()
}

fn function_tool(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::client::ToolChoice;

    #[tokio::test]
    async fn a_model_that_rejects_reasoning_effort_is_retried_without_it_once() {
        use alloc::sync::Arc;

        use crate::auth::store::Credentials as CredentialStore;
        use crate::auth::testing::MockServer;
        use crate::llm::provider::{Protocol, Provider};

        let reply = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let server = MockServer::start(vec![(
            "/chat/completions",
            vec![(400, "unknown field reasoning_effort".into()), (200, reply.into())],
        )])
        .await;
        let auth = Auth::new(Provider::Ollama, Protocol::OpenAi, Arc::new(CredentialStore::in_memory()))
            .with_base_url(&server.url);
        let client = OpenAiClient::new(OpenAiConfig { model: "plain".into(), auth });
        let messages = [Message::user("hi")];
        let ask = || CreateCompletionRequest { messages: &messages, ..Default::default() };

        let done = client.create_completion_stream(ask(), |_| {}).await.unwrap();
        assert_eq!(done.text, "hi");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].body.contains("\"reasoning_effort\":\"medium\""));
        assert!(requests.iter().all(|r| { r.body.contains(&format!("\"prompt_cache_key\":\"{}-", crate::APP_NAME)) }));
        assert!(!requests[1].body.contains("reasoning_effort"));

        client.create_completion_stream(ask(), |_| {}).await.unwrap();
        let sent = server.requests();
        assert_eq!(sent.len(), 3, "the working setting is remembered");
        assert!(!sent[2].body.contains("reasoning_effort"));
    }

    #[tokio::test]
    async fn tools_can_be_forbidden_for_one_request() {
        use alloc::sync::Arc;

        use crate::auth::store::Credentials as CredentialStore;
        use crate::auth::testing::MockServer;
        use crate::llm::provider::{Protocol, Provider};

        let reply = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let server = MockServer::start(vec![("/chat/completions", vec![(200, reply.into())])]).await;
        let auth = Auth::new(Provider::Ollama, Protocol::OpenAi, Arc::new(CredentialStore::in_memory()))
            .with_base_url(&server.url);
        let client = OpenAiClient::new(OpenAiConfig { model: "m".into(), auth });
        let messages = [Message::user("hi")];
        let tools = [ToolSpec { name: "t".into(), description: "d".into(), parameters: json!({ "type": "object" }) }];
        client
            .create_completion_stream(
                CreateCompletionRequest {
                    messages: &messages,
                    tools: &tools,
                    tool_choice: ToolChoice::None,
                    ..Default::default()
                },
                |_| {},
            )
            .await
            .unwrap();
        let body = &server.requests()[0].body;
        assert!(body.contains("\"tool_choice\":\"none\""), "{body}");
        assert!(body.contains("\"name\":\"t\""), "the definitions stay: {body}");
    }

    #[test]
    fn each_level_ends_by_asking_for_nothing() {
        assert_eq!(efforts(Thinking::Medium), [Some("medium"), None]);
        assert_eq!(efforts(Thinking::Max), [Some("xhigh"), Some("high"), None]);
    }

    #[test]
    fn describes_tools_in_the_openai_function_shape() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({ "type": "object" }),
        };
        let value = function_tool(&spec);
        assert_eq!(value["type"], "function");
        assert_eq!(value["function"]["name"], "read_file");
        assert_eq!(value["function"]["parameters"]["type"], "object");
    }
}
