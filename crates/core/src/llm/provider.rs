use alloc::sync::Arc;
use core::hash::{Hash, Hasher};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The built-in providers, in the order the menus show them.
pub const BUILTIN_IDS: [&str; 5] = ["ollama", "openai", "anthropic", "github-copilot", "google"];

/// The built-in providers that need a login, in the order the menus show them.
pub const LOGIN_IDS: [&str; 4] = ["github-copilot", "openai", "anthropic", "google"];

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

    #[must_use]
    #[inline]
    /// # Panics
    ///
    /// Panics only if the built-in table is edited to leave out `ollama`.
    pub fn ollama() -> Self {
        Self::builtin("ollama").expect("ollama is built in")
    }

    #[must_use]
    #[inline]
    /// # Panics
    ///
    /// Panics only if the built-in table is edited to leave out `openai`.
    pub fn openai() -> Self {
        Self::builtin("openai").expect("openai is built in")
    }

    #[must_use]
    #[inline]
    /// # Panics
    ///
    /// Panics only if the built-in table is edited to leave out `anthropic`.
    pub fn anthropic() -> Self {
        Self::builtin("anthropic").expect("anthropic is built in")
    }

    #[must_use]
    #[inline]
    /// # Panics
    ///
    /// Panics only if the built-in table is edited to leave out `github-copilot`.
    pub fn copilot() -> Self {
        Self::builtin("github-copilot").expect("github-copilot is built in")
    }

    #[must_use]
    #[inline]
    /// # Panics
    ///
    /// Panics only if the built-in table is edited to leave out `google`.
    pub fn google() -> Self {
        Self::builtin("google").expect("google is built in")
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

    #[must_use]
    #[inline]
    pub fn is_builtin(&self) -> bool {
        BUILTIN_IDS.contains(&self.id.as_str())
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u32>,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
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
        Self { context_limit: None, id: id.clone(), name: id, protocol: None }
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

    /// Names this model uniquely and stays the same across runs: `provider/protocol/id`.
    #[must_use]
    #[inline]
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.provider.id, self.protocol().label(), self.model.id)
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

/// A model named in a Persona file. It is resolved to a [`ModelSpec`] through the models store.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub context_limit: Option<u32>,
    #[serde(default)]
    pub protocol: Option<Protocol>,
}

impl ModelRef {
    #[must_use]
    #[inline]
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        Self { provider: provider.into(), id: id.into(), context_limit: None, protocol: None }
    }

    /// The model this names, served by `provider`.
    #[must_use]
    #[inline]
    pub fn resolve(self, provider: Arc<Provider>) -> ModelSpec {
        let model =
            Model { context_limit: self.context_limit, id: self.id, name: String::new(), protocol: self.protocol };
        ModelSpec::new(provider, model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_name_the_model_and_provider() {
        let label = |provider: Provider, protocol: Option<Protocol>, id: &str| {
            let model = Model::new(id).with_protocol(protocol);
            ModelSpec::new(Arc::new(provider), model).label()
        };
        assert_eq!(label(Provider::ollama(), None, "bonsai-2:2b"), "bonsai-2:2b (ollama/openai)");
        assert_eq!(
            label(Provider::ollama(), Some(Protocol::Anthropic), "bonsai-2:2b"),
            "bonsai-2:2b (ollama/anthropic)"
        );
        assert_eq!(label(Provider::openai(), None, "gpt-4.1"), "gpt-4.1 (openai)");
        assert_eq!(label(Provider::copilot(), None, "claude-sonnet-4.5"), "claude-sonnet-4.5 (github-copilot)");
        assert_eq!(label(Provider::custom("my-gateway"), None, "m"), "m (my-gateway)");
    }

    #[test]
    fn keys_are_stable_and_include_the_effective_protocol() {
        let spec = |provider: Provider, protocol: Option<Protocol>| {
            ModelSpec::new(Arc::new(provider), Model::new("m").with_protocol(protocol)).key()
        };
        assert_eq!(spec(Provider::ollama(), None), "ollama/openai/m");
        assert_eq!(spec(Provider::openai(), None), "openai/responses/m");
        assert_eq!(spec(Provider::ollama(), Some(Protocol::Anthropic)), "ollama/anthropic/m");
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
        assert_eq!(Provider::ollama().login_method(), LoginMethod::None);
        assert!(Provider::logins().iter().all(|p| p.login_method() != LoginMethod::None));
        assert_eq!(Provider::logins().into_iter().map(|p| p.id).collect::<Vec<_>>(), LOGIN_IDS);
    }

    #[test]
    fn each_builtin_knows_how_it_logs_in_and_where_its_key_lives() {
        assert_eq!(Provider::copilot().login_method(), LoginMethod::GithubDevice);
        assert_eq!(Provider::openai().login_method(), LoginMethod::ApiKey);
        assert_eq!(Provider::openai().env_var.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(Provider::anthropic().env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(Provider::google().env_var.as_deref(), Some("GOOGLE_API_KEY"));
        assert_eq!(Provider::copilot().env_var, None);
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
    fn each_provider_and_protocol_has_a_base_url() {
        assert_eq!(Provider::ollama().base_url_for(Protocol::OpenAi), "http://localhost:11434/v1");
        assert_eq!(Provider::ollama().base_url_for(Protocol::Anthropic), "http://localhost:11434");
        assert_eq!(Provider::openai().base_url_for(Protocol::OpenAi), "https://api.openai.com/v1");
        assert_eq!(Provider::anthropic().base_url_for(Protocol::Anthropic), "https://api.anthropic.com");
        assert_eq!(Provider::copilot().base_url_for(Protocol::OpenAi), "https://api.individual.githubcopilot.com");
        assert_eq!(Provider::google().base_url_for(Protocol::Google), "https://generativelanguage.googleapis.com");
    }

    #[test]
    fn a_model_can_override_its_providers_protocol() {
        let spec = ModelSpec::new(
            Arc::new(Provider::ollama()),
            Model::new("m").with_protocol(Some(Protocol::Anthropic)).with_context_limit(Some(8192)),
        );
        assert_eq!(spec.protocol(), Protocol::Anthropic);
        assert_eq!(spec.context_limit(), Some(8192));
    }

    #[test]
    fn provider_equality_is_by_id_alone() {
        let mut other = Provider::ollama();
        other.display_name = "Elsewhere".into();
        other.models = vec![Model::new("m")];
        assert_eq!(Provider::ollama(), other);
    }
}
