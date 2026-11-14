use alloc::sync::Arc;

use crate::auth::Auth;
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::provider::{Model, ModelSpec, Protocol};

use super::anthropic::{Client as AnthropicClient, Config as AnthropicConfig};
use super::gemini::{Client as GeminiClient, Config as GeminiConfig};
use super::openai::{OpenAiClient, OpenAiConfig};
use super::responses::{Client as ResponsesClient, Config as ResponsesConfig};
use super::{ClientError, CreateCompletionRequest, Delta, Llm as LlmClient, StreamedCompletion};

#[cfg(any(test, feature = "testing"))]
use super::fake::Client as FakeClient;

#[derive(Debug)]
/// A client of any protocol.
pub enum Client {
    Anthropic(AnthropicClient),
    #[cfg(any(test, feature = "testing"))]
    Fake(FakeClient),
    Gemini(GeminiClient),
    OpenAi(OpenAiClient),
    Responses(ResponsesClient),
}

impl Client {
    #[inline]
    pub fn for_model(spec: &ModelSpec, store: &Arc<CredentialStore>) -> Self {
        let auth = Auth::for_model(spec, Arc::clone(store));
        let model = spec.model.id.clone();
        match spec.protocol() {
            Protocol::OpenAi => Self::OpenAi(OpenAiClient::new(OpenAiConfig { auth, model })),
            Protocol::Anthropic => {
                Self::Anthropic(AnthropicClient::new(AnthropicConfig { auth, max_tokens: Model::MAX_TOKENS, model }))
            },
            Protocol::Google => {
                Self::Gemini(GeminiClient::new(GeminiConfig { auth, max_tokens: Model::MAX_TOKENS, model }))
            },
            Protocol::Responses => Self::Responses(ResponsesClient::new(ResponsesConfig { auth, model })),
        }
    }
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
        match *self {
            Self::OpenAi(ref client) => client.create_completion_stream(request, on_delta).await,
            Self::Anthropic(ref client) => client.create_completion_stream(request, on_delta).await,
            Self::Gemini(ref client) => client.create_completion_stream(request, on_delta).await,
            Self::Responses(ref client) => client.create_completion_stream(request, on_delta).await,
            #[cfg(any(test, feature = "testing"))]
            Self::Fake(ref client) => client.create_completion_stream(request, on_delta).await,
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::llm::client::chat::Message;

    async fn ask(spec: &ModelSpec) -> StreamedCompletion {
        let messages = [Message::system("Answer in one word."), Message::user("Say hi.")];
        let request = CreateCompletionRequest { messages: &messages, ..Default::default() };
        let store = Arc::new(CredentialStore::in_memory());
        Client::for_model(spec, &store).create_completion_stream(request, |_| {}).await.unwrap()
    }

    #[tokio::test]
    #[ignore = "needs a local Ollama server"]
    async fn both_providers_answer_from_a_local_ollama() {
        for protocol in [Protocol::OpenAi, Protocol::Anthropic] {
            let spec = ModelSpec::new(
                Arc::new(crate::llm::provider::Provider::ollama()),
                Model::new("llama3.2:3b").with_protocol(Some(protocol)),
            );
            let done = ask(&spec).await;
            assert!(!done.text.is_empty(), "{}", spec.label());
            assert!(done.usage.is_some(), "{}", spec.label());
        }
    }
}
