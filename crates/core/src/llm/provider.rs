use alloc::sync::Arc;
use core::hash::{Hash, Hasher};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The built-in providers, in the order the menus show them.
pub const BUILTIN_IDS: [&str; 6] = ["ollama", "openai", "anthropic", "github-copilot", "google", "deepseek"];

/// The built-in providers that need a login, in the order the menus show them.
pub const LOGIN_IDS: [&str; 5] = ["github-copilot", "openai", "anthropic", "google", "deepseek"];

/// One provider of models: a built-in or a gateway the user configured.
///
/// A provider is identified by its `id`. Everything else may be changed in
/// `models-store.json`; the built-ins only supply the defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// The key in `models-store.json`. It is not stored inside the entry.
    #[serde(skip)]
    pub id: String,
    pub display_name: String,
    pub login: LoginMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    pub protocol: Protocol,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, rename = "checkedAt")]
    pub checked_at: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Model>,
}

// A Provider is a name for a service: two values with the same id are the same provider,
// whatever their cached model list or settings happen to be.
impl PartialEq for Provider {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Provider {}

impl Hash for Provider {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Provider {
    /// A provider the user configured. Everything it needs must be in the file.
    #[must_use]
    #[inline]
    pub fn custom<IntoId>(id: IntoId) -> Self
    where
        IntoId: Into<String>,
    {
        let id = id.into();
        Self {
            display_name: id.clone(),
            login: LoginMethod::ApiKey,
            env_var: None,
            protocol: Protocol::OpenAi,
            base_url: String::new(),
            headers: BTreeMap::new(),
            checked_at: 0,
            models: Vec::new(),
            id,
        }
    }

    #[must_use]
    #[inline]
    pub fn new<IntoId, IntoDisplayName>(
        id: IntoId,
        display_name: IntoDisplayName,
        login: LoginMethod,
        protocol: Protocol,
        base_url: impl Into<String>,
    ) -> Self
    where
        IntoId: Into<String>,
        IntoDisplayName: Into<String>,
    {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            login,
            env_var: None,
            protocol,
            base_url: base_url.into(),
            headers: BTreeMap::new(),
            checked_at: 0,
            models: Vec::new(),
        }
    }

    /// A built-in provider, by id.
    #[must_use]
    #[inline]
    pub fn builtin(id: &str) -> Option<Self> {
        Some(match id {
            "ollama" => Self::new("ollama", "Ollama", LoginMethod::None, Protocol::OpenAi, "http://localhost:11434/v1"),
            "openai" => {
                Self::new("openai", "OpenAI", LoginMethod::ApiKey, Protocol::Responses, "https://api.openai.com/v1")
                    .with_env_var(Some("OPENAI_API_KEY"))
            },
            "deepseek" => {
                Self::new("deepseek", "DeepSeek", LoginMethod::ApiKey, Protocol::OpenAi, "https://api.deepseek.com")
                    .with_env_var(Some("DEEPSEEK_API_KEY"))
            },
            "anthropic" => Self::new(
                "anthropic",
                "Anthropic",
                LoginMethod::ApiKey,
                Protocol::Anthropic,
                "https://api.anthropic.com",
            )
            .with_env_var(Some("ANTHROPIC_API_KEY")),
            "github-copilot" => Self::new(
                "github-copilot",
                "GitHub Copilot",
                LoginMethod::GithubDevice,
                Protocol::Anthropic,
                "https://api.individual.githubcopilot.com",
            ),
            "google" => Self::new(
                "google",
                "Google",
                LoginMethod::ApiKey,
                Protocol::Google,
                "https://generativelanguage.googleapis.com",
            )
            .with_env_var(Some("GOOGLE_API_KEY")),
            _ => return None,
        })
    }

    /// Every built-in provider, in menu order.
    #[must_use]
    #[inline]
    pub fn builtins() -> Vec<Self> {
        BUILTIN_IDS.iter().filter_map(|id| Self::builtin(id)).collect()
    }

    /// The built-in providers that need a login, in menu order.
    #[must_use]
    #[inline]
    pub fn logins() -> Vec<Self> {
        LOGIN_IDS.iter().filter_map(|id| Self::builtin(id)).collect()
    }

    /// How the provider takes a token over `protocol`. A GitHub Copilot token is always a bearer
    /// token, even when the model speaks the Anthropic protocol.
    #[must_use]
    #[inline]
    pub const fn auth_header(&self, protocol: Protocol) -> AuthHeader {
        match self.login {
            LoginMethod::GithubDevice => AuthHeader::Bearer,
            LoginMethod::ApiKey | LoginMethod::None => AuthHeader::from_protocol(protocol),
        }
    }

    /// The base URL for a protocol. Ollama serves the OpenAI-compatible API under `/v1`
    /// and its Anthropic-compatible API at the root, so the two differ.
    #[must_use]
    #[inline]
    pub fn base_url_for(&self, protocol: Protocol) -> &str {
        match (self.id.as_str(), protocol) {
            ("ollama", Protocol::Anthropic) => self.base_url.strip_suffix("/v1").unwrap_or(&self.base_url),
            _ => &self.base_url,
        }
    }

    #[must_use]
    #[inline]
    pub const fn login_method(&self) -> LoginMethod {
        self.login
    }

    #[must_use]
    #[inline]
    /// The key the provider is stored under.
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    #[inline]
    /// The name to show.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    #[inline]
    /// The environment variable its key may come from, if any.
    pub fn env_var(&self) -> Option<&str> {
        self.env_var.as_deref()
    }

    #[must_use]
    #[inline]
    pub fn with_env_var(mut self, env_var: Option<&str>) -> Self {
        self.env_var = env_var.map(str::to_owned);
        self
    }

    #[must_use]
    #[inline]
    pub fn with_models(mut self, models: Vec<Model>) -> Self {
        self.models = models;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// How a provider is logged in to.
pub enum LoginMethod {
    #[default]
    ApiKey,
    GithubDevice,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// The wire protocol of a model.
pub enum Protocol {
    Anthropic,
    Google,
    OpenAi,
    Responses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthHeader {
    Bearer,
    CustomHeader(&'static str),
}

impl AuthHeader {
    #[must_use]
    #[inline]
    pub const fn from_protocol(protocol: Protocol) -> Self {
        match protocol {
            Protocol::Anthropic => Self::CustomHeader("x-api-key"),
            Protocol::Google => Self::CustomHeader("x-goog-api-key"),
            Protocol::OpenAi | Protocol::Responses => Self::Bearer,
        }
    }
}

impl Protocol {
    #[must_use]
    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::OpenAi => "openai",
            Self::Responses => "responses",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[serde(default, alias = "context_limit", skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u32>,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    /// The provider that serves it, when a Persona file names the model before the models
    /// store has resolved it. A provider's own models leave this empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl Model {
    /// The most tokens one Anthropic reply may have. A whole-file `write_file` or a large
    /// `edit_file` is one reply, so 4096 cut them off mid-argument.
    pub const MAX_TOKENS: u32 = 0x4000;

    #[must_use]
    #[inline]
    pub fn new<IntoId>(id: IntoId) -> Self
    where
        IntoId: Into<String>,
    {
        let id = id.into();
        Self { context_limit: None, id: id.clone(), name: id, protocol: None, provider: None }
    }

    /// A model named in a Persona file, with the provider it belongs to.
    #[must_use]
    #[inline]
    pub fn reference<IntoProvider, IntoId>(provider: IntoProvider, id: IntoId) -> Self
    where
        IntoProvider: Into<String>,
        IntoId: Into<String>,
    {
        let id = id.into();
        Self { context_limit: None, id, name: String::new(), protocol: None, provider: Some(provider.into()) }
    }

    /// The name to show: the provider's name for it, or its id.
    #[must_use]
    #[inline]
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }

    #[must_use]
    #[inline]
    pub const fn with_context_limit(mut self, context_limit: Option<u32>) -> Self {
        self.context_limit = context_limit;
        self
    }

    #[must_use]
    #[inline]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    #[must_use]
    #[inline]
    pub const fn with_protocol(mut self, protocol: Option<Protocol>) -> Self {
        self.protocol = protocol;
        self
    }
}

/// A chosen model, together with the provider that serves it.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub provider: Arc<Provider>,
    pub model: Model,
}

impl PartialEq for ModelSpec {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.provider.id == other.provider.id && self.model.id == other.model.id
    }
}

impl Eq for ModelSpec {}

impl ModelSpec {
    /// The most tokens one reply may have.
    pub const MAX_TOKENS: u32 = Model::MAX_TOKENS;

    #[must_use]
    #[inline]
    pub const fn new(provider: Arc<Provider>, model: Model) -> Self {
        Self { provider, model }
    }

    /// The tokens its context window holds, if known.
    #[must_use]
    #[inline]
    pub const fn context_limit(&self) -> Option<u32> {
        self.model.context_limit
    }

    /// The protocol this model speaks: its own, or its provider's.
    #[must_use]
    #[inline]
    pub fn protocol(&self) -> Protocol {
        self.model.protocol.unwrap_or(self.provider.protocol)
    }

    #[must_use]
    #[inline]
    /// The model id with its provider, for lists.
    pub fn label(&self) -> String {
        if self.provider.login == LoginMethod::None {
            format!("{} ({}/{})", self.model.id, self.provider.id, self.protocol().label())
        } else {
            format!("{} ({})", self.model.id, self.provider.id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(id: &str) -> Provider {
        Provider::builtin(id).expect("the provider is built in")
    }

    #[test]
    fn labels_name_the_model_and_provider() {
        let label = |provider: Provider, protocol: Option<Protocol>, id: &str| {
            let model = Model::new(id).with_protocol(protocol);
            ModelSpec::new(Arc::new(provider), model).label()
        };
        assert_eq!(label(builtin("ollama"), None, "bonsai-2:2b"), "bonsai-2:2b (ollama/openai)");
        assert_eq!(
            label(builtin("ollama"), Some(Protocol::Anthropic), "bonsai-2:2b"),
            "bonsai-2:2b (ollama/anthropic)"
        );
        assert_eq!(label(builtin("openai"), None, "gpt-4.1"), "gpt-4.1 (openai)");
        assert_eq!(label(builtin("github-copilot"), None, "claude-sonnet-4.5"), "claude-sonnet-4.5 (github-copilot)");
        assert_eq!(label(Provider::custom("my-gateway"), None, "m"), "m (my-gateway)");
    }

    #[test]
    fn a_reference_names_the_model_and_the_provider_that_serves_it() {
        let reference = Model::reference("ollama", "qwen");
        assert_eq!(reference.provider.as_deref(), Some("ollama"));
        assert_eq!(reference.display_name(), "qwen");
        assert_eq!(Model::new("m").provider, None);
    }

    #[test]
    fn provider_ids_are_unique_and_every_builtin_is_listed() {
        let ids: Vec<String> = Provider::builtins().into_iter().map(|p| p.id).collect();
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[index + 1..].contains(id), "{id}");
        }
        assert_eq!(ids, BUILTIN_IDS);
    }

    #[test]
    fn only_local_builtins_skip_login_and_the_others_are_offered_for_login() {
        assert_eq!(builtin("ollama").login_method(), LoginMethod::None);
        assert!(Provider::logins().iter().all(|p| p.login_method() != LoginMethod::None));
        assert_eq!(Provider::logins().into_iter().map(|p| p.id).collect::<Vec<_>>(), LOGIN_IDS);
    }

    #[test]
    fn each_builtin_knows_how_it_logs_in_and_where_its_key_lives() {
        assert_eq!(builtin("github-copilot").login_method(), LoginMethod::GithubDevice);
        assert_eq!(builtin("openai").login_method(), LoginMethod::ApiKey);
        assert_eq!(builtin("openai").env_var.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(builtin("anthropic").env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(builtin("google").env_var.as_deref(), Some("GOOGLE_API_KEY"));
        assert_eq!(builtin("github-copilot").env_var, None);
        assert_eq!(Provider::custom("gw").login_method(), LoginMethod::ApiKey);
    }

    #[test]
    fn the_credential_header_depends_on_the_protocol() {
        assert_eq!(AuthHeader::from_protocol(Protocol::Anthropic), AuthHeader::CustomHeader("x-api-key"));
        assert_eq!(AuthHeader::from_protocol(Protocol::OpenAi), AuthHeader::Bearer);
        assert_eq!(AuthHeader::from_protocol(Protocol::Responses), AuthHeader::Bearer);
        assert_eq!(AuthHeader::from_protocol(Protocol::Google), AuthHeader::CustomHeader("x-goog-api-key"));
    }

    #[test]
    fn copilot_takes_a_bearer_token_over_every_protocol() {
        assert_eq!(builtin("github-copilot").auth_header(Protocol::Anthropic), AuthHeader::Bearer);
        assert_eq!(builtin("anthropic").auth_header(Protocol::Anthropic), AuthHeader::CustomHeader("x-api-key"));
        assert_eq!(builtin("ollama").auth_header(Protocol::Anthropic), AuthHeader::CustomHeader("x-api-key"));
    }

    #[test]
    fn each_provider_and_protocol_has_a_base_url() {
        assert_eq!(builtin("ollama").base_url_for(Protocol::OpenAi), "http://localhost:11434/v1");
        assert_eq!(builtin("ollama").base_url_for(Protocol::Anthropic), "http://localhost:11434");
        assert_eq!(builtin("openai").base_url_for(Protocol::OpenAi), "https://api.openai.com/v1");
        assert_eq!(builtin("anthropic").base_url_for(Protocol::Anthropic), "https://api.anthropic.com");
        assert_eq!(
            builtin("github-copilot").base_url_for(Protocol::OpenAi),
            "https://api.individual.githubcopilot.com"
        );
        assert_eq!(builtin("google").base_url_for(Protocol::Google), "https://generativelanguage.googleapis.com");
    }

    #[test]
    fn a_model_can_override_its_providers_protocol() {
        let spec = ModelSpec::new(
            Arc::new(builtin("ollama")),
            Model::new("m").with_protocol(Some(Protocol::Anthropic)).with_context_limit(Some(8192)),
        );
        assert_eq!(spec.protocol(), Protocol::Anthropic);
        assert_eq!(spec.context_limit(), Some(8192));
    }

    #[test]
    fn provider_equality_is_by_id_alone() {
        let mut other = builtin("ollama");
        other.display_name = "Elsewhere".into();
        other.models = vec![Model::new("m")];
        assert_eq!(builtin("ollama"), other);
    }
}
