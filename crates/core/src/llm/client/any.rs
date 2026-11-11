use alloc::sync::Arc;

use crate::auth::Auth;
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::provider::{ModelSpec, Protocol};

use super::anthropic::{Client as AnthropicClient, Config as AnthropicConfig};
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
    OpenAi(OpenAiClient),
    Responses(ResponsesClient),
}

impl Client {
    #[inline]
    /// Creates the client for a model's protocol.
    pub fn for_model(spec: &ModelSpec, store: &Arc<CredentialStore>) -> Self {
        let auth = Auth::for_model(spec, Arc::clone(store));
        let model = spec.id.clone();
        match spec.protocol {
            Protocol::OpenAi => Self::OpenAi(OpenAiClient::new(OpenAiConfig { auth, model })),
            Protocol::Anthropic => Self::Anthropic(AnthropicClient::new(AnthropicConfig {
                model,
                max_tokens: ModelSpec::MAX_TOKENS,
                auth,
            })),
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
    use crate::llm::provider::Provider;

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
            let spec = ModelSpec::new(Provider::Ollama, protocol, "llama3.2:3b");
            let done = ask(&spec).await;
            assert!(!done.text.is_empty(), "{}", spec.label());
            assert!(done.usage.is_some(), "{}", spec.label());
        }
    }
}
