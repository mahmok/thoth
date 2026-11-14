/// GitHub Copilot tokens and headers.
pub mod copilot;
/// Stored credentials.
pub mod credential;
/// Events of a login.
pub mod events;
/// The login flow.
pub mod flow;
/// GitHub device login.
pub mod github;
/// A login in progress.
pub mod session;
/// Credential storage.
pub mod store;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::unwrap_used, clippy::indexing_slicing, reason = "test support: a failed setup should fail the test")]
/// Test doubles for login.
pub mod testing;
/// The login worker on tokio.
pub mod worker;

use alloc::fmt;
use alloc::sync::Arc;
use core::error;
use std::sync::LazyLock;

use reqwest::{Method, Response, StatusCode};
use serde::Serialize;

use crate::llm::client::chat::Message;
use crate::llm::client::error::Failure as ClientError;
use crate::llm::provider::{AuthHeader, LoginMethod, Model, Protocol, Provider};

use credential::Credential;
use github::{Endpoints, Github};
use store::{Credentials as CredentialStore, Source};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Why a request could not be authorised.
pub enum Failure {
    Flow(String),
    NotLoggedIn(Provider),
    Rejected(Provider),
    Store(String),
    Unsupported(Provider),
}

impl fmt::Display for Failure {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NotLoggedIn(p) => {
                write!(f, "not logged in to {}; run /login", p.display_name())
            },
            Self::Rejected(p) => {
                write!(f, "not logged in to {} (the credentials were rejected); run /login", p.display_name())
            },
            Self::Unsupported(p) => {
                write!(f, "the stored {} credential cannot be used here; run /login", p.display_name())
            },
            Self::Flow(ref message) => f.write_str(message),
            Self::Store(ref message) => write!(f, "cannot save credentials: {message}"),
        }
    }
}

impl error::Error for Failure {
    #[inline]
    fn cause(&self) -> Option<&dyn error::Error> {
        None
    }

    #[inline]
    fn description(&self) -> &'static str {
        "a request could not be authorised"
    }

    #[inline]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestAuth {
    base_url: Option<String>,
    headers: Vec<(&'static str, String)>,
    token: String,
}

impl RequestAuth {
    fn token(token: impl Into<String>) -> Self {
        Self { token: token.into(), base_url: None, headers: Vec::new() }
    }
}

#[derive(Clone, Debug)]
pub struct Auth {
    base_url: String,
    github: Github,
    http: reqwest::Client,
    provider: Provider,
    store: Arc<CredentialStore>,
}

impl Auth {
    #[inline]
    pub fn for_provider(provider: Provider, store: Arc<CredentialStore>) -> Self {
        Self::new(provider, store)
    }

    #[inline]
    /// GETs `path` with the provider's authorisation.
    ///
    /// # Errors
    ///
    /// Fails if the request cannot be authorised or sent, or the API rejects it.
    pub async fn get(&self, path: &str, extra_headers: &[(&str, &str)]) -> Result<Response, ClientError> {
        self.send::<()>(Method::GET, path, extra_headers, &[], None).await
    }

    #[inline]
    pub fn new(provider: Provider, store: Arc<CredentialStore>) -> Self {
        Self {
            provider,
            base_url: provider.base_url(provider.protocol).to_owned(),
            store,
            http: http_client(),
            github: Github::new(Endpoints::github()),
        }
    }

    #[inline]
    /// POSTs `body` as JSON to `path` with the provider's authorisation.
    ///
    /// # Errors
    ///
    /// Fails if the request cannot be authorised or sent, or the API rejects it.
    pub async fn post_json<Payload>(
        &self,
        path: &str,
        extra_headers: &[(&str, &str)],
        messages: &[Message],
        body: &Payload,
    ) -> Result<Response, ClientError>
    where
        Payload: Serialize + Sync,
    {
        self.send(Method::POST, path, extra_headers, messages, Some(body)).await
    }

    /// Renews the Copilot token, one caller at a time, and saves it.
    async fn refresh_copilot(&self) -> Result<String, Failure> {
        let _guard = self.store.refresh_lock().lock().await;
        let refresh = match self.store.read(Provider::Copilot) {
            Some(Credential::Oauth { access, expires, .. }) if !copilot::expires_soon(expires) => {
                return Ok(access);
            },
            Some(Credential::Oauth { refresh, .. }) => refresh,
            _ => return Err(Failure::NotLoggedIn(Provider::Copilot)),
        };
        let credential =
            self.github.exchange_token(&refresh).await.map_err(|e| Failure::Flow(format!("{e}; run /login")))?;
        let Credential::Oauth { ref access, .. } = credential else {
            return Err(Failure::Unsupported(Provider::Copilot));
        };
        let token = access.clone();
        self.store.write(Provider::Copilot, credential).map_err(|e| Failure::Store(e.to_string()))?;
        Ok(token)
    }

    async fn resolve(&self, messages: &[Message]) -> Result<RequestAuth, Failure> {
        let provider = self.provider;
        let method = provider.login_method();
        match self.store.source(provider).ok_or(Failure::NotLoggedIn(provider))? {
            Source::Local => Ok(RequestAuth::token("ollama")),
            Source::Environment(key) => Ok(RequestAuth::token(key)),
            Source::Stored(Credential::ApiKey { key }) if method == LoginMethod::ApiKey => Ok(RequestAuth::token(key)),
            Source::Stored(Credential::Oauth { access, expires, .. }) if method == LoginMethod::GithubDevice => {
                let current = if copilot::expires_soon(expires) { self.refresh_copilot().await? } else { access };
                Ok(RequestAuth {
                    base_url: copilot::base_url_from_token(&current),
                    token: current,
                    headers: copilot::headers(messages),
                })
            },
            Source::Stored(_) => Err(Failure::Unsupported(provider)),
        }
    }

    #[expect(clippy::too_many_arguments, reason = "the arguments are separate pieces of one call")]
    async fn send<B>(
        &self,
        method: Method,
        path: &str,
        extra_headers: &[(&str, &str)],
        messages: &[Message],
        body: Option<&B>,
    ) -> Result<Response, ClientError>
    where
        B: Serialize + Sync,
    {
        let auth = self.resolve(messages).await?;
        let base_url = auth.base_url.as_deref().unwrap_or(&self.base_url);
        let mut request = self.http.request(method, format!("{base_url}{path}"));
        if let Some(payload) = body {
            request = request.json(payload);
        }
        request = match AuthHeader::from_protocol(self.provider.protocol) {
            AuthHeader::Bearer => request.bearer_auth(&auth.token),
            AuthHeader::CustomHeader(header) => request.header(header, &auth.token),
        };
        for &(name, ref value) in &auth.headers {
            request = request.header(name, value);
        }
        for &(name, value) in extra_headers {
            request = request.header(name, value);
        }

        let response = request.send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(Failure::Rejected(self.provider).into());
        }
        if matches!(response.status(), StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY) {
            let detail = response.text().await.unwrap_or_default();
            return Err(ClientError::BadRequest(detail.trim().to_owned()));
        }
        Ok(response.error_for_status()?)
    }

    #[must_use]
    #[inline]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        base_url.clone_into(&mut self.base_url);
        self
    }

    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    #[inline]
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.github = Github::new(endpoints);
        self
    }
}

/// The program's one HTTP client. Clones share its connection pool and TLS setup, so
/// switching models or refreshing the model lists reuses connections instead of rebuilding them.
fn http_client() -> reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
    CLIENT.clone()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use credential::now_ms;
    use testing::MockServer;

    fn store() -> Arc<CredentialStore> {
        Arc::new(CredentialStore::in_memory())
    }

    fn key(value: &str) -> Credential {
        Credential::ApiKey { key: value.into() }
    }

    fn oauth(access: &str, expires: u64) -> Credential {
        Credential::Oauth { refresh: "gh-token".into(), access: access.into(), expires }
    }

    fn auth(provider: Provider, protocol: Protocol, store: Arc<CredentialStore>) -> Auth {
        Auth::new(provider, store)
    }

    async fn post(auth: &Auth, server: &MockServer, messages: &[Message]) -> Result<Response, ClientError> {
        let local = auth.clone().with_base_url(&server.url);
        local.post_json("/chat/completions", &[("x-extra", "1")], messages, &json!({ "a": 1i32 })).await
    }

    async fn ok_server() -> MockServer {
        MockServer::start(vec![("/chat/completions", vec![(200, "{}".into())])]).await
    }

    #[tokio::test]
    async fn ollama_needs_nothing() {
        let auth = auth(Provider::Ollama, Protocol::OpenAi, store());
        assert_eq!(auth.resolve(&[]).await.unwrap().token, "ollama");
    }

    #[tokio::test]
    async fn a_stored_key_is_used_and_beats_the_environment() {
        let store = Arc::new(CredentialStore::in_memory().with_env("OPENAI_API_KEY", "sk-env"));
        let auth = auth(Provider::OpenAi, Protocol::OpenAi, Arc::clone(&store));
        assert_eq!(auth.resolve(&[]).await.unwrap().token, "sk-env");
        store.write(Provider::OpenAi, key("sk-stored")).unwrap();
        assert_eq!(auth.resolve(&[]).await.unwrap().token, "sk-stored");
    }

    #[tokio::test]
    async fn without_a_credential_the_error_points_at_login() {
        let err = auth(Provider::Anthropic, Protocol::Anthropic, store()).resolve(&[]).await.unwrap_err();
        assert_eq!(err, Failure::NotLoggedIn(Provider::Anthropic));
        assert_eq!(err.to_string(), "not logged in to Anthropic; run /login");
    }

    #[tokio::test]
    async fn a_credential_of_the_wrong_kind_is_unsupported() {
        let store = store();
        store.write(Provider::Copilot, key("sk")).unwrap();
        store.write(Provider::OpenAi, oauth("a", 0)).unwrap();
        assert_eq!(
            auth(Provider::Copilot, Protocol::OpenAi, Arc::clone(&store)).resolve(&[]).await.unwrap_err(),
            Failure::Unsupported(Provider::Copilot)
        );
        assert_eq!(
            auth(Provider::OpenAi, Protocol::OpenAi, store).resolve(&[]).await.unwrap_err(),
            Failure::Unsupported(Provider::OpenAi)
        );
    }

    #[tokio::test]
    async fn a_valid_copilot_token_is_used_as_is_with_its_headers_and_base_url() {
        let store = store();
        let token = "tid=1;exp=2;proxy-ep=proxy.business.githubcopilot.com;x=y";
        store.write(Provider::Copilot, oauth(token, now_ms() + 3_600_000)).unwrap();
        let resolved = auth(Provider::Copilot, Protocol::OpenAi, store).resolve(&[Message::user("hi")]).await.unwrap();
        assert_eq!(resolved.token, token);
        assert_eq!(resolved.base_url.as_deref(), Some("https://api.business.githubcopilot.com"));
        assert!(resolved.headers.contains(&("X-Initiator", "user".to_owned())));
    }

    #[tokio::test]
    async fn an_expiring_copilot_token_is_refreshed_and_saved() {
        let server = MockServer::start(vec![(
            "/copilot_internal/v2/token",
            vec![(
                200,
                format!(r#"{{"token":"fresh","expires_at":{}}}"#, now_ms().div_euclid(1000).saturating_add(3600)),
            )],
        )])
        .await;
        let store = store();
        store.write(Provider::Copilot, oauth("stale", now_ms() + 60_000)).unwrap();
        let auth = auth(Provider::Copilot, Protocol::OpenAi, Arc::clone(&store)).with_endpoints(server.endpoints());

        assert_eq!(auth.resolve(&[]).await.unwrap().token, "fresh");
        assert!(matches!(store.read(Provider::Copilot), Some(Credential::Oauth { access, refresh, .. })
            if access == "fresh" && refresh == "gh-token"));
        assert_eq!(server.requests()[0].header("authorization"), Some("Bearer gh-token"));
    }

    #[tokio::test]
    async fn concurrent_requests_refresh_once() {
        let server = MockServer::start(vec![(
            "/copilot_internal/v2/token",
            vec![(
                200,
                format!(r#"{{"token":"fresh","expires_at":{}}}"#, now_ms().div_euclid(1000).saturating_add(3600)),
            )],
        )])
        .await;
        let store = store();
        store.write(Provider::Copilot, oauth("stale", 0)).unwrap();
        let auth = auth(Provider::Copilot, Protocol::OpenAi, store).with_endpoints(server.endpoints());

        let (a, b, c) = tokio::join!(auth.resolve(&[]), auth.resolve(&[]), auth.resolve(&[]));
        assert!(a.unwrap().token == "fresh" && b.unwrap().token == "fresh" && c.unwrap().token == "fresh");
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_failed_refresh_asks_for_a_new_login() {
        let server = MockServer::start(vec![("/copilot_internal/v2/token", vec![(401, "{}".into())])]).await;
        let store = store();
        store.write(Provider::Copilot, oauth("stale", 0)).unwrap();
        let auth = auth(Provider::Copilot, Protocol::OpenAi, store).with_endpoints(server.endpoints());
        let err = auth.resolve(&[]).await.unwrap_err();
        assert!(err.to_string().ends_with("run /login"), "{err}");
    }

    #[tokio::test]
    async fn openai_style_requests_carry_a_bearer_token_and_the_extra_headers() {
        let server = ok_server().await;
        let store = store();
        store.write(Provider::OpenAi, key("sk-1")).unwrap();
        post(&auth(Provider::OpenAi, Protocol::OpenAi, store), &server, &[]).await.unwrap();

        let request = &server.requests()[0];
        assert_eq!(request.header("authorization"), Some("Bearer sk-1"));
        assert_eq!(request.header("x-extra"), Some("1"));
        assert_eq!(request.body, r#"{"a":1}"#);
    }

    #[tokio::test]
    async fn anthropic_style_requests_use_x_api_key() {
        let server = ok_server().await;
        let store = store();
        store.write(Provider::Anthropic, key("sk-ant")).unwrap();
        post(&auth(Provider::Anthropic, Protocol::Anthropic, store), &server, &[]).await.unwrap();

        let request = &server.requests()[0];
        assert_eq!(request.header("x-api-key"), Some("sk-ant"));
        assert_eq!(request.header("authorization"), None);
    }

    #[tokio::test]
    async fn copilot_requests_are_bearer_even_over_the_anthropic_protocol_and_carry_copilot_headers() {
        let server = ok_server().await;
        let store = store();
        store.write(Provider::Copilot, oauth("copilot-tok", now_ms() + 3_600_000)).unwrap();
        post(&auth(Provider::Copilot, Protocol::Anthropic, store), &server, &[Message::user("hi")]).await.unwrap();

        let request = &server.requests()[0];
        assert_eq!(request.header("authorization"), Some("Bearer copilot-tok"));
        assert_eq!(request.header("x-api-key"), None);
        assert_eq!(request.header("copilot-integration-id"), Some("vscode-chat"));
        assert_eq!(request.header("x-initiator"), Some("user"));
    }

    #[tokio::test]
    async fn get_requests_are_authorized_without_a_body() {
        let server = MockServer::start(vec![("/models", vec![(200, "{}".into())])]).await;
        let store = store();
        store.write(Provider::OpenAi, key("sk-1")).unwrap();
        let auth = auth(Provider::OpenAi, Protocol::OpenAi, store).with_base_url(&server.url);
        auth.get("/models", &[("x-extra", "1")]).await.unwrap();

        let request = &server.requests()[0];
        assert_eq!(request.method, "GET");
        assert_eq!(request.header("authorization"), Some("Bearer sk-1"));
        assert_eq!(request.header("x-extra"), Some("1"));
        assert_eq!(request.body, "");
    }

    #[tokio::test]
    async fn a_rejected_credential_is_reported_as_a_login_problem() {
        let server = MockServer::start(vec![("/chat/completions", vec![(401, "{}".into())])]).await;
        let store = store();
        store.write(Provider::OpenAi, key("sk-bad")).unwrap();
        let err = post(&auth(Provider::OpenAi, Protocol::OpenAi, store), &server, &[]).await.unwrap_err();
        assert_eq!(err.to_string(), "not logged in to OpenAI (the credentials were rejected); run /login");
    }

    #[tokio::test]
    async fn other_http_errors_stay_http_errors() {
        let server = MockServer::start(vec![("/chat/completions", vec![(500, "{}".into())])]).await;
        let err = post(&auth(Provider::Ollama, Protocol::OpenAi, store()), &server, &[]).await.unwrap_err();
        assert!(matches!(err, ClientError::Http(_)), "{err}");
    }

    #[tokio::test]
    async fn a_missing_credential_fails_before_any_request_is_sent() {
        let server = ok_server().await;
        let err = post(&auth(Provider::OpenAi, Protocol::OpenAi, store()), &server, &[]).await.unwrap_err();
        assert!(matches!(err, ClientError::Auth(Failure::NotLoggedIn(Provider::OpenAi))), "{err}");
        assert!(server.requests().is_empty());
    }
}
