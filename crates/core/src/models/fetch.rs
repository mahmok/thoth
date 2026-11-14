use alloc::sync::Arc;

use serde::Deserialize;

use crate::auth::Auth;
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::client::anthropic::API_VERSION;
use crate::llm::client::error::Failure as ClientError;
use crate::llm::provider::{LoginMethod, Model, Protocol, Provider};

/// Id fragments of models that cannot chat.
const NOT_CHAT: [&str; 11] = [
    "embedding",
    "whisper",
    "tts",
    "dall-e",
    "moderation",
    "audio",
    "image",
    "realtime",
    "transcribe",
    "davinci",
    "babbage",
];
/// The Copilot API version sent.
const COPILOT_API_VERSION: &str = "2026-06-01";

#[derive(Debug, PartialEq, Eq)]
/// The result of fetching models.
pub enum Fetched {
    Models(Vec<Model>),
    NoCredentials,
}

#[derive(Deserialize)]
/// A model list reply.
struct List<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
/// A model with only an id.
struct PlainModel {
    id: String,
}

#[derive(Deserialize)]
/// A model of Anthropic.
struct AnthropicModel {
    display_name: Option<String>,
    id: String,
    max_input_tokens: Option<u32>,
}

#[derive(Deserialize)]
/// A model of Copilot.
struct CopilotModel {
    #[serde(default)]
    capabilities: CopilotCapabilities,
    id: String,
    name: Option<String>,
    #[serde(default)]
    supported_endpoints: Vec<String>,
}

#[derive(Deserialize, Default)]
/// What a Copilot model supports.
struct CopilotCapabilities {
    #[serde(default)]
    limits: CopilotLimits,
}

/// Copilot also reports `max_context_window_tokens`, which includes the output; the prompt limit is what matters.
#[derive(Deserialize, Default)]
struct CopilotLimits {
    max_prompt_tokens: Option<u32>,
}

#[derive(Deserialize)]
/// A model list reply of Google.
struct GoogleList {
    #[serde(default)]
    models: Vec<GoogleModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// A model of Google.
struct GoogleModel {
    display_name: Option<String>,
    input_token_limit: Option<u32>,
    name: String,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

#[derive(Debug)]
/// Fetches the models a provider offers.
pub struct ModelFetcher {
    base_url: Option<String>,
    credentials: Arc<CredentialStore>,
}

impl ModelFetcher {
    async fn anthropic(&self, provider: &Provider) -> Result<Vec<Model>, ClientError> {
        let auth = self.auth(provider);
        let response = auth.get("/v1/models?limit=1000", &[("anthropic-version", API_VERSION)]).await?;
        let list: List<AnthropicModel> = response.json().await?;
        Ok(list
            .data
            .into_iter()
            .map(|model| {
                Model::new(model.id)
                    .with_name(model.display_name.unwrap_or_default())
                    .with_context_limit(model.max_input_tokens)
            })
            .collect())
    }

    fn auth(&self, provider: &Provider) -> Auth {
        let auth = Auth::for_provider(provider, Arc::clone(&self.credentials));
        match self.base_url.as_ref() {
            Some(base_url) => auth.with_base_url(base_url),
            None => auth,
        }
    }

    /// Fetches the Copilot models.
    async fn copilot(&self, provider: &Provider) -> Result<Vec<Model>, ClientError> {
        let headers = [("Accept", "application/json"), ("X-GitHub-Api-Version", COPILOT_API_VERSION)];
        let response = self.auth(provider).get("/models", &headers).await?;
        let list: List<CopilotModel> = response.json().await?;
        Ok(list
            .data
            .into_iter()
            .filter_map(|model| {
                let supports = |endpoint: &str| model.supported_endpoints.iter().any(|e| e == endpoint);
                let protocol = if supports("/chat/completions") {
                    Protocol::OpenAi
                } else if supports("/v1/messages") {
                    Protocol::Anthropic
                } else if supports("/responses") {
                    Protocol::Responses
                } else {
                    return None;
                };
                Some(
                    Model::new(model.id)
                        .with_name(model.name.unwrap_or_default())
                        .with_protocol(Some(protocol))
                        .with_context_limit(model.capabilities.limits.max_prompt_tokens),
                )
            })
            .collect())
    }

    #[inline]
    /// Fetches the models a provider offers, or says there are no credentials.
    ///
    /// # Errors
    ///
    /// Fails if the provider's model list cannot be fetched.
    pub async fn fetch(&self, provider: &Provider) -> Result<Fetched, ClientError> {
        if !self.credentials.can_use(provider) {
            return Ok(Fetched::NoCredentials);
        }
        let mut models = if provider.login == LoginMethod::GithubDevice {
            self.copilot(provider).await?
        } else {
            match provider.protocol {
                Protocol::Anthropic => self.anthropic(provider).await?,
                Protocol::Google => self.google(provider).await?,
                Protocol::OpenAi | Protocol::Responses => {
                    self.plain(provider, |id| !NOT_CHAT.iter().any(|word| id.contains(word))).await?
                },
            }
        };
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup_by(|a, b| a.id == b.id);
        Ok(Fetched::Models(models))
    }

    /// Fetches the Google models that can generate content.
    async fn google(&self, provider: &Provider) -> Result<Vec<Model>, ClientError> {
        let response = self.auth(provider).get("/v1beta/models", &[]).await?;
        let list: GoogleList = response.json().await?;
        Ok(list
            .models
            .into_iter()
            .filter(|model| {
                model.supported_generation_methods.is_empty()
                    || model.supported_generation_methods.iter().any(|method| method == "generateContent")
            })
            .map(|model| {
                Model::new(model.name.strip_prefix("models/").unwrap_or(&model.name).to_owned())
                    .with_name(model.display_name.unwrap_or_default())
                    .with_context_limit(model.input_token_limit)
            })
            .collect())
    }

    #[inline]
    pub const fn new(credentials: Arc<CredentialStore>) -> Self {
        Self { credentials, base_url: None }
    }

    async fn plain(&self, provider: &Provider, keep: impl Fn(&str) -> bool) -> Result<Vec<Model>, ClientError> {
        let response = self.auth(provider).get("/models", &[]).await?;
        let list: List<PlainModel> = response.json().await?;
        Ok(list.data.into_iter().filter(|model| keep(&model.id)).map(|model| Model::new(model.id)).collect())
    }

    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    #[inline]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = Some(base_url.to_owned());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::{Credential, now_ms};
    use crate::auth::testing::MockServer;
    use crate::test::assert_not_empty;

    fn credentials(provider: Option<(Provider, Credential)>) -> Arc<CredentialStore> {
        let store = CredentialStore::in_memory();
        if let Some((owner, credential)) = provider {
            store.write(&owner, credential).unwrap();
        }
        Arc::new(store)
    }

    fn key() -> Credential {
        Credential::ApiKey { key: "sk-test".into() }
    }

    async fn serve(path: &str, body: &str) -> MockServer {
        MockServer::start(vec![(path, vec![(200, body.to_owned())])]).await
    }

    fn ids(fetched: Fetched) -> Vec<(String, Protocol)> {
        match fetched {
            Fetched::Models(models) => {
                models.into_iter().map(|m| (m.id, m.protocol.unwrap_or(Protocol::OpenAi))).collect()
            },
            Fetched::NoCredentials => panic!("expected models"),
        }
    }

    #[tokio::test]
    async fn ollama_models_are_listed_once_as_openai_protocol_without_any_login() {
        let server = serve("/models", r#"{"data":[{"id":"mistral:latest"},{"id":"bonsai-2:2b"}]}"#).await;
        let fetcher = ModelFetcher::new(credentials(None)).with_base_url(&server.url);
        let models = ids(fetcher.fetch(&Provider::ollama()).await.unwrap());
        assert_eq!(models, [("bonsai-2:2b".into(), Protocol::OpenAi), ("mistral:latest".into(), Protocol::OpenAi)]);
    }

    #[tokio::test]
    async fn openai_models_drop_the_ones_that_are_not_chat_models() {
        let body = r#"{"data":[{"id":"gpt-4o"},{"id":"text-embedding-3-small"},{"id":"whisper-1"},
            {"id":"gpt-4o-realtime-preview"},{"id":"dall-e-3"},{"id":"gpt-4.1"},{"id":"tts-1"},
            {"id":"omni-moderation-latest"},{"id":"gpt-4o-audio-preview"},{"id":"gpt-image-1"},
            {"id":"o3-mini"},{"id":"gpt-4o-transcribe"},{"id":"davinci-002"},{"id":"babbage-002"}]}"#;
        let server = serve("/models", body).await;
        let fetcher = ModelFetcher::new(credentials(Some((Provider::openai(), key())))).with_base_url(&server.url);
        let models = ids(fetcher.fetch(&Provider::openai()).await.unwrap());
        assert_eq!(models.into_iter().map(|(id, _)| id).collect::<Vec<_>>(), ["gpt-4.1", "gpt-4o", "o3-mini"]);
        assert_eq!(server.requests()[0].header("authorization"), Some("Bearer sk-test"));
    }

    #[tokio::test]
    async fn anthropic_models_keep_their_display_names() {
        let body = r#"{"data":[{"id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5","type":"model","max_input_tokens":200000},{"id":"claude-x"}]}"#;
        let server = serve("/v1/models", body).await;
        let fetcher = ModelFetcher::new(credentials(Some((Provider::anthropic(), key())))).with_base_url(&server.url);
        let Fetched::Models(models) = fetcher.fetch(&Provider::anthropic()).await.unwrap() else {
            panic!("expected models")
        };
        assert_eq!(models[0].id, "claude-sonnet-4-5");
        assert_eq!(models[0].display_name(), "Claude Sonnet 4.5");
        assert_eq!(models[1].display_name(), "claude-x");
        let limits: Vec<_> = models.iter().map(|m| m.context_limit).collect();
        assert_eq!(limits, [Some(200_000), None]);
        assert!(models.iter().all(|m| m.protocol.is_none()));

        let request = &server.requests()[0];
        assert_eq!(request.path, "/v1/models?limit=1000");
        assert_eq!(request.header("x-api-key"), Some("sk-test"));
        assert_eq!(request.header("anthropic-version"), Some(API_VERSION));
    }

    #[tokio::test]
    async fn copilot_models_follow_the_endpoints_the_client_can_speak() {
        let body = r#"{"data":[
            {"id":"gpt-4.1","name":"GPT-4.1","supported_endpoints":["/chat/completions","/responses"]},
            {"id":"claude-sonnet-4.5","name":"Claude Sonnet 4.5","supported_endpoints":["/v1/messages"],
                "capabilities":{"limits":{"max_context_window_tokens":328000,"max_prompt_tokens":200000}}},
            {"id":"both","supported_endpoints":["/v1/messages","/chat/completions"]},
            {"id":"gpt-5-codex","supported_endpoints":["/responses"]},
            {"id":"no-endpoints"},
            {"id":"gpt-4.1","name":"duplicate","supported_endpoints":["/chat/completions"]}]}"#;
        let server = serve("/models", body).await;
        let token =
            Credential::Oauth { refresh: "gh".into(), access: "copilot-tok".into(), expires: now_ms() + 3_600_000 };
        let fetcher = ModelFetcher::new(credentials(Some((Provider::copilot(), token)))).with_base_url(&server.url);
        let Fetched::Models(models) = fetcher.fetch(&Provider::copilot()).await.unwrap() else {
            panic!("expected models")
        };

        let summary: Vec<(&str, Option<Protocol>)> = models.iter().map(|m| (m.id.as_str(), m.protocol)).collect();
        assert_eq!(
            summary,
            [
                ("both", Some(Protocol::OpenAi)),
                ("claude-sonnet-4.5", Some(Protocol::Anthropic)),
                ("gpt-4.1", Some(Protocol::OpenAi)),
                ("gpt-5-codex", Some(Protocol::Responses))
            ]
        );
        assert_eq!(models[1].display_name(), "Claude Sonnet 4.5");
        assert_eq!(models[1].context_limit, Some(200_000), "the prompt limit, not the window");
        assert_eq!(models[0].context_limit, None);

        let request = &server.requests()[0];
        assert_eq!(request.header("authorization"), Some("Bearer copilot-tok"));
        assert_eq!(request.header("copilot-integration-id"), Some("vscode-chat"));
        assert_eq!(request.header("x-github-api-version"), Some(COPILOT_API_VERSION));
    }

    #[tokio::test]
    async fn google_models_are_listed_with_their_input_limit() {
        let body = r#"{"models":[
            {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro","inputTokenLimit":1048576,
             "supportedGenerationMethods":["generateContent","countTokens"]},
            {"name":"models/text-embedding-004","displayName":"Embedding",
             "supportedGenerationMethods":["embedContent"]}]}"#;
        let server = serve("/v1beta/models", body).await;
        let fetcher = ModelFetcher::new(credentials(Some((Provider::google(), key())))).with_base_url(&server.url);
        let Fetched::Models(models) = fetcher.fetch(&Provider::google()).await.unwrap() else {
            panic!("expected models")
        };
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-2.5-pro");
        assert_eq!(models[0].display_name(), "Gemini 2.5 Pro");
        assert_eq!(models[0].context_limit, Some(1_048_576));

        let request = &server.requests()[0];
        assert_eq!(request.path, "/v1beta/models");
        assert_eq!(request.header("x-goog-api-key"), Some("sk-test"));
    }

    #[tokio::test]
    async fn a_provider_without_credentials_is_skipped_without_a_request() {
        let server = serve("/models", r#"{"data":[]}"#).await;
        let fetcher = ModelFetcher::new(credentials(None)).with_base_url(&server.url);
        for provider in Provider::logins() {
            assert_eq!(fetcher.fetch(&provider).await.unwrap(), Fetched::NoCredentials);
        }
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn http_errors_and_rejected_keys_are_errors() {
        let server = MockServer::start(vec![("/models", vec![(500, "{}".into())])]).await;
        let fetcher = ModelFetcher::new(credentials(None)).with_base_url(&server.url);
        assert!(matches!(fetcher.fetch(&Provider::ollama()).await, Err(ClientError::Http(_))));

        let rejecting = MockServer::start(vec![("/models", vec![(401, "{}".into())])]).await;
        let keyed = ModelFetcher::new(credentials(Some((Provider::openai(), key())))).with_base_url(&rejecting.url);
        let err = keyed.fetch(&Provider::openai()).await.unwrap_err();
        assert!(err.to_string().contains("rejected"), "{err}");
    }

    #[tokio::test]
    async fn an_unexpected_body_is_an_error() {
        let server = serve("/models", r#"{"surprise":true}"#).await;
        let fetcher = ModelFetcher::new(credentials(None)).with_base_url(&server.url);
        fetcher.fetch(&Provider::ollama()).await.unwrap_err();
    }

    #[tokio::test]
    #[ignore = "needs a local Ollama server"]
    async fn the_real_ollama_lists_its_models() {
        let fetcher = ModelFetcher::new(credentials(None));
        let models = ids(fetcher.fetch(&Provider::ollama()).await.unwrap());
        assert_not_empty(&models);
    }
}
