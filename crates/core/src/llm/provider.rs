use serde::{Deserialize, Serialize};

/// Facts about Ollama.
const OLLAMA_INFO: Info = Info { id: "ollama", display_name: "Ollama", login: LoginMethod::None, env_var: None };

const OPENAI_INFO: Info =
    Info { id: "openai", display_name: "OpenAI", login: LoginMethod::ApiKey, env_var: Some("OPENAI_API_KEY") };

/// Facts about Anthropic.
const ANTHROPIC_INFO: Info =
    Info { id: "anthropic", display_name: "Anthropic", login: LoginMethod::ApiKey, env_var: Some("ANTHROPIC_API_KEY") };

/// Facts about Copilot.
const COPILOT_INFO: Info =
    Info { id: "github-copilot", display_name: "GitHub Copilot", login: LoginMethod::GithubDevice, env_var: None };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
/// A model provider.
pub enum Provider {
    Anthropic,
    Copilot,
    Ollama,
    OpenAi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How a provider is logged in to.
pub enum LoginMethod {
    ApiKey,
    GithubDevice,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How a provider takes a token.
pub enum AuthHeader {
    Bearer,
    XApiKey,
}

/// Facts about a provider.
struct Info {
    display_name: &'static str,
    env_var: Option<&'static str>,
    id: &'static str,
    login: LoginMethod,
}

impl Provider {
    /// Every provider.
    pub const ALL: [Self; 4] = [Self::Ollama, Self::OpenAi, Self::Anthropic, Self::Copilot];

    /// The providers that need a login.
    pub const LOGINS: [Self; 3] = [Self::Copilot, Self::OpenAi, Self::Anthropic];

    #[must_use]
    #[inline]
    /// How the provider takes a token for a protocol.
    pub const fn auth_header(self, protocol: Protocol) -> AuthHeader {
        match (protocol, self) {
            (Protocol::Anthropic, Self::Copilot) | (Protocol::OpenAi | Protocol::Responses, _) => AuthHeader::Bearer,
            (Protocol::Anthropic, _) => AuthHeader::XApiKey,
        }
    }

    #[must_use]
    #[inline]
    /// The base URL of the provider for a protocol.
    pub const fn base_url(self, protocol: Protocol) -> &'static str {
        match (self, protocol) {
            (Self::Ollama, Protocol::OpenAi | Protocol::Responses) => "http://localhost:11434/v1",
            (Self::Ollama, Protocol::Anthropic) => "http://localhost:11434",
            (Self::OpenAi, _) => "https://api.openai.com/v1",
            (Self::Anthropic, _) => "https://api.anthropic.com",
            (Self::Copilot, _) => "https://api.individual.githubcopilot.com",
        }
    }

    #[must_use]
    #[inline]
    pub const fn display_name(self) -> &'static str {
        self.info().display_name
    }

    #[must_use]
    #[inline]
    pub const fn env_var(self) -> Option<&'static str> {
        self.info().env_var
    }

    #[must_use]
    #[inline]
    pub const fn id(self) -> &'static str {
        self.info().id
    }

    const fn info(self) -> &'static Info {
        match self {
            Self::Ollama => &OLLAMA_INFO,
            Self::OpenAi => &OPENAI_INFO,
            Self::Anthropic => &ANTHROPIC_INFO,
            Self::Copilot => &COPILOT_INFO,
        }
    }

    #[must_use]
    #[inline]
    pub const fn login_method(self) -> LoginMethod {
        self.info().login
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// The wire protocol of a model.
pub enum Protocol {
    Anthropic,
    OpenAi,
    Responses,
}

impl Protocol {
    #[must_use]
    #[inline]
    /// A short name for the protocol.
    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Responses => "responses",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// A model of a provider.
pub struct ModelSpec {
    /// The most prompt tokens the model accepts, if its Provider says.
    pub context_limit: Option<u32>,
    /// The id the provider knows it by.
    pub id: String,
    /// How to talk to it.
    pub protocol: Protocol,
    /// Who serves it.
    pub provider: Provider,
}

impl ModelSpec {
    /// The most tokens one Anthropic reply may have. A whole-file `write_file` or a large
    /// `edit_file` is one reply, so 4096 cut them off mid-argument.
    pub const MAX_TOKENS: u32 = 0x4000;

    /// Names this model uniquely and stays the same across runs: `provider/protocol/id`.
    #[must_use]
    #[inline]
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.provider.id(), self.protocol.label(), self.id)
    }

    #[must_use]
    #[inline]
    /// The model id with its provider, for lists.
    pub fn label(&self) -> String {
        match self.provider {
            Provider::Ollama => format!("{} (ollama/{})", self.id, self.protocol.label()),
            provider @ (Provider::OpenAi | Provider::Anthropic | Provider::Copilot) => {
                format!("{} ({})", self.id, provider.id())
            },
        }
    }

    #[inline]
    pub fn new<IntoString>(provider: Provider, protocol: Protocol, id: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self { provider, protocol, id: id.into(), context_limit: None }
    }

    #[must_use]
    #[inline]
    pub const fn with_context_limit(mut self, context_limit: Option<u32>) -> Self {
        self.context_limit = context_limit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_name_the_model_and_provider() {
        let label = |provider, protocol, id| ModelSpec::new(provider, protocol, id).label();
        assert_eq!(label(Provider::Ollama, Protocol::OpenAi, "bonsai-2:2b"), "bonsai-2:2b (ollama/openai)");
        assert_eq!(label(Provider::Ollama, Protocol::Anthropic, "bonsai-2:2b"), "bonsai-2:2b (ollama/anthropic)");
        assert_eq!(label(Provider::OpenAi, Protocol::OpenAi, "gpt-4.1"), "gpt-4.1 (openai)");
        assert_eq!(
            label(Provider::Copilot, Protocol::Anthropic, "claude-sonnet-4.5"),
            "claude-sonnet-4.5 (github-copilot)"
        );
    }

    #[test]
    fn provider_ids_are_unique_and_every_provider_is_listed() {
        let ids = Provider::ALL.map(Provider::id);
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[index + 1..].contains(id), "{id}");
        }
        assert_eq!(Provider::ALL.len(), 4);
    }

    #[test]
    fn only_local_models_skip_login() {
        assert_eq!(Provider::Ollama.login_method(), LoginMethod::None);
        assert!(Provider::LOGINS.iter().all(|p| p.login_method() != LoginMethod::None));
    }

    #[test]
    fn each_provider_knows_how_it_logs_in_and_where_its_key_lives() {
        assert_eq!(Provider::Copilot.login_method(), LoginMethod::GithubDevice);
        assert_eq!(Provider::OpenAi.login_method(), LoginMethod::ApiKey);
        assert_eq!(Provider::OpenAi.env_var(), Some("OPENAI_API_KEY"));
        assert_eq!(Provider::Anthropic.env_var(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(Provider::Copilot.env_var(), None);
    }

    #[test]
    fn the_credential_header_depends_on_provider_and_protocol() {
        assert_eq!(Provider::Anthropic.auth_header(Protocol::Anthropic), AuthHeader::XApiKey);
        assert_eq!(Provider::Ollama.auth_header(Protocol::Anthropic), AuthHeader::XApiKey);
        assert_eq!(Provider::Copilot.auth_header(Protocol::Anthropic), AuthHeader::Bearer);
        assert_eq!(Provider::Copilot.auth_header(Protocol::OpenAi), AuthHeader::Bearer);
        assert_eq!(Provider::OpenAi.auth_header(Protocol::OpenAi), AuthHeader::Bearer);
    }

    #[test]
    fn each_provider_and_protocol_has_a_base_url() {
        assert_eq!(Provider::Ollama.base_url(Protocol::OpenAi), "http://localhost:11434/v1");
        assert_eq!(Provider::Ollama.base_url(Protocol::Anthropic), "http://localhost:11434");
        assert_eq!(Provider::OpenAi.base_url(Protocol::OpenAi), "https://api.openai.com/v1");
        assert_eq!(Provider::Anthropic.base_url(Protocol::Anthropic), "https://api.anthropic.com");
        assert_eq!(Provider::Copilot.base_url(Protocol::OpenAi), "https://api.individual.githubcopilot.com");
    }
}
