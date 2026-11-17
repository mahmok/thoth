use reqwest::Response;
use serde::Serialize;
use serde_json::{Value, json};

use crate::adapters::auth::Auth;
use crate::domain::message::Message;
use crate::domain::shared::Usage;
use crate::domain::thinking::Thinking;

use super::body::Common;
use super::chat::{CompletionChunk as ChatCompletionChunk, ToolCallBuffer};
use super::stream;
use super::{ClientError, CreateCompletionRequest, Delta, Llm as LlmClient, Negotiation, StreamedCompletion, ToolSpec};

#[derive(Debug)]
pub struct OpenAiConfig {
    pub auth: Auth,
    pub model: String,
}

#[derive(Debug)]
pub struct OpenAiClient {
    config: OpenAiConfig,
    negotiation: Negotiation,
}

impl OpenAiClient {
    #[must_use]
    #[inline]
    pub fn new(config: OpenAiConfig) -> Self {
        Self { config, negotiation: Negotiation::default() }
    }

    async fn post(&self, request: &CreateCompletionRequest<'_>) -> Result<Response, ClientError> {
        let efforts = efforts(request.thinking);
        self.negotiation.send(request.thinking, &efforts, |effort| self.post_with(request, *effort)).await
    }

    /// POSTs the request with a reasoning effort.
    async fn post_with(
        &self,
        request: &CreateCompletionRequest<'_>,
        effort: Option<&'static str>,
    ) -> Result<Response, ClientError> {
        let body = Body {
            common: Common::streaming(
                &self.config.model,
                request.tools.iter().map(function_tool).collect(),
                request.tool_choice.forbids(request.tools).then_some("none"),
            ),
            messages: request.messages,
            prompt_cache_key: self.config.auth.session_id().map(|session| session.to_string()),
            reasoning_effort: effort,
            stream_options: StreamOptions { include_usage: true },
        };
        self.config.auth.post_json("/chat/completions", &[], request.messages, &body).await
    }
}

/// The `/chat/completions` request. It borrows the history, so sending it copies nothing first.
#[derive(Serialize)]
struct Body<'request> {
    #[serde(flatten)]
    common: Common<'request, Value, &'static str>,
    messages: &'request [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
    stream_options: StreamOptions,
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
        on_delta: F,
    ) -> Result<StreamedCompletion, ClientError>
    where
        F: FnMut(Delta<'_>) + Send,
    {
        stream::drive(self.post(&request), StreamState::default(), on_delta).await
    }
}

/// The sentinel a chat-completions stream sends after its last event.
const DONE: &str = "[DONE]";

#[derive(Default)]
/// What one chat-completions stream has produced.
struct StreamState {
    calls: ToolCallBuffer,
    cut_off: Option<String>,
    text: String,
    usage: Option<Usage>,
}

impl stream::Adapter for StreamState {
    fn is_end(&self, data: &str) -> bool {
        data == DONE
    }

    fn apply(&mut self, data: &str, on_delta: &mut impl FnMut(Delta<'_>)) -> Result<(), ClientError> {
        let chunk: ChatCompletionChunk = serde_json::from_str(data)?;
        if chunk.usage.is_some() {
            self.usage = chunk.usage;
        }
        for choice in chunk.choices {
            // Servers name the reasoning field differently (`reasoning_content`, `reasoning`).
            let reasoning = choice.delta.reasoning_content.or(choice.delta.reasoning);
            if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
                on_delta(Delta::Reasoning(&text));
            }
            if let Some(text) = choice.delta.content.filter(|t| !t.is_empty()) {
                on_delta(Delta::Text(&text));
                self.text.push_str(&text);
            }
            for delta in choice.delta.tool_calls {
                self.calls.append(delta);
            }
            if choice.finish_reason.as_deref() == Some("length") {
                self.cut_off = Some("length".to_owned());
            }
        }
        Ok(())
    }

    fn finish(self) -> StreamedCompletion {
        StreamedCompletion {
            text: self.text,
            usage: self.usage,
            tool_calls: self.calls.finish(),
            cut_off: self.cut_off,
            ..Default::default()
        }
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
    use crate::adapters::llm::client::ToolChoice;

    #[tokio::test]
    async fn the_done_sentinel_ends_the_stream_before_anything_after_it() {
        use alloc::sync::Arc;

        use crate::adapters::auth::store::Credentials as CredentialStore;
        use crate::adapters::auth::testing::MockServer;
        use crate::domain::provider::{Protocol, Provider};

        let reply = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
            "data: not json\n\n",
        );
        let server = MockServer::start(vec![("/chat/completions", vec![(200, reply.into())])]).await;
        let auth =
            Auth::new(Provider::builtin("ollama").unwrap(), Protocol::OpenAi, Arc::new(CredentialStore::in_memory()))
                .with_base_url(&server.url);
        let client = OpenAiClient::new(OpenAiConfig { model: "m".into(), auth });
        let messages = [Message::user("hi")];
        let done = client
            .create_completion_stream(CreateCompletionRequest { messages: &messages, ..Default::default() }, |_| {})
            .await
            .unwrap();
        assert_eq!(done.text, "hi");
        assert_eq!(done.usage.unwrap().total_tokens, 2);
    }

    #[tokio::test]
    async fn a_reply_that_hits_the_token_limit_is_an_error_not_an_empty_reply() {
        use alloc::sync::Arc;

        use crate::adapters::auth::store::Credentials as CredentialStore;
        use crate::adapters::auth::testing::MockServer;
        use crate::domain::provider::{Protocol, Provider};

        let reply = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
        let server = MockServer::start(vec![("/chat/completions", vec![(200, reply.into())])]).await;
        let auth =
            Auth::new(Provider::builtin("ollama").unwrap(), Protocol::OpenAi, Arc::new(CredentialStore::in_memory()))
                .with_base_url(&server.url);
        let client = OpenAiClient::new(OpenAiConfig { model: "m".into(), auth });
        let messages = [Message::user("hi")];
        let err = client
            .create_completion_stream(CreateCompletionRequest { messages: &messages, ..Default::default() }, |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("length"), "{err}");
    }

    #[tokio::test]
    async fn a_model_that_rejects_reasoning_effort_is_retried_without_it_once() {
        use alloc::sync::Arc;

        use crate::adapters::auth::store::Credentials as CredentialStore;
        use crate::adapters::auth::testing::MockServer;
        use crate::domain::provider::{Protocol, Provider};
        use crate::domain::shared::SessionId;

        let reply = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let server = MockServer::start(vec![(
            "/chat/completions",
            vec![(400, "unknown field reasoning_effort".into()), (200, reply.into())],
        )])
        .await;
        let session = SessionId::mint();
        let auth =
            Auth::new(Provider::builtin("ollama").unwrap(), Protocol::OpenAi, Arc::new(CredentialStore::in_memory()))
                .with_base_url(&server.url)
                .with_session(session);
        let client = OpenAiClient::new(OpenAiConfig { model: "plain".into(), auth });
        let messages = [Message::user("hi")];
        let ask = || CreateCompletionRequest { messages: &messages, ..Default::default() };

        let done = client.create_completion_stream(ask(), |_| {}).await.unwrap();
        assert_eq!(done.text, "hi");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].body.contains("\"reasoning_effort\":\"medium\""));
        assert!(requests.iter().all(|r| r.body.contains(&format!("\"prompt_cache_key\":\"{session}\""))));
        assert!(!requests[1].body.contains("reasoning_effort"));

        client.create_completion_stream(ask(), |_| {}).await.unwrap();
        let sent = server.requests();
        assert_eq!(sent.len(), 3, "the working setting is remembered");
        assert!(!sent[2].body.contains("reasoning_effort"));
    }

    #[tokio::test]
    async fn tools_can_be_forbidden_for_one_request() {
        use alloc::sync::Arc;

        use crate::adapters::auth::store::Credentials as CredentialStore;
        use crate::adapters::auth::testing::MockServer;
        use crate::domain::provider::{Protocol, Provider};

        let reply = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let server = MockServer::start(vec![("/chat/completions", vec![(200, reply.into())])]).await;
        let auth =
            Auth::new(Provider::builtin("ollama").unwrap(), Protocol::OpenAi, Arc::new(CredentialStore::in_memory()))
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
