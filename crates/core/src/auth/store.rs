use crate::config;
use alloc::collections::BTreeMap;
use core::fmt;
use core::fmt::Debug;
use core::fmt::Formatter;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync;

use crate::fsutil::write_private;
use crate::llm::provider::{LoginMethod, Provider};
use crate::sync::MutexExt as _;

use super::credential::Credential;

#[derive(Clone, PartialEq, Eq)]
/// Where a credential came from.
pub enum Source {
    Environment(String),
    Local,
    Stored(Credential),
}

impl Debug for Source {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Local => f.write_str("Source::Local"),
            Self::Stored(ref credential) => write!(f, "Source::Stored({credential:?})"),
            Self::Environment(_) => f.write_str("Source::Environment(..)"),
        }
    }
}
#[derive(Debug)]

/// The environment credentials are read from.
enum Env {
    #[cfg(any(test, feature = "testing"))]
    Fixed(BTreeMap<String, String>),
    Process,
}

impl Env {
    fn get(&self, name: &str) -> Option<String> {
        match *self {
            Self::Process => env::var(name).ok(),
            #[cfg(any(test, feature = "testing"))]
            Self::Fixed(ref vars) => vars.get(name).cloned(),
        }
        .filter(|value| !value.is_empty())
    }
}

#[derive(Debug)]
/// Saved credentials.
pub struct Credentials {
    entries: Mutex<BTreeMap<String, Credential>>,
    env: Env,
    path: Option<PathBuf>,
    refresh: sync::Mutex<()>,
}

impl Credentials {
    #[inline]
    pub fn can_use(&self, provider: Provider) -> bool {
        self.source(provider).is_some()
    }

    #[must_use]
    #[inline]
    pub fn default_path() -> Option<PathBuf> {
        config::dir().map(|dir| dir.join("auth.json"))
    }

    #[inline]
    /// Removes the saved credential of `provider`.
    ///
    /// # Errors
    ///
    /// Fails if the credentials cannot be saved.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    pub fn delete(&self, provider: Provider) -> io::Result<()> {
        let mut credentials = self.entries.lock_unpoisoned();
        drop(credentials.remove(provider.id()));
        self.persist(&credentials)
    }

    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    #[inline]
    /// A store that is never saved, for tests.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            entries: Mutex::default(),
            refresh: sync::Mutex::default(),
            env: Env::Fixed(BTreeMap::new()),
        }
    }

    #[inline]
    /// Opens the credentials file, or starts empty if it does not exist.
    ///
    /// # Errors
    ///
    /// Fails if the file exists but cannot be read or parsed.
    pub fn open<IntoPathbuf>(location: IntoPathbuf) -> io::Result<Self>
    where
        IntoPathbuf: Into<PathBuf>,
    {
        let path = location.into();
        let credentials = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path),
            entries: Mutex::new(credentials),
            refresh: sync::Mutex::default(),
            env: Env::Process,
        })
    }

    fn persist(&self, credentials: &BTreeMap<String, Credential>) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(credentials).map_err(io::Error::other)?;
        write_private(path, &json)
    }

    #[inline]
    pub fn read(&self, provider: Provider) -> Option<Credential> {
        self.entries.lock_unpoisoned().get(provider.id()).cloned()
    }

    #[inline]
    pub const fn refresh_lock(&self) -> &sync::Mutex<()> {
        &self.refresh
    }

    #[inline]
    /// Where `provider`'s credential comes from: local, saved or the environment.
    pub fn source(&self, provider: Provider) -> Option<Source> {
        if provider.login_method() == LoginMethod::None {
            return Some(Source::Local);
        }
        if let Some(credential) = self.read(provider) {
            return Some(Source::Stored(credential));
        }
        provider.env_var().and_then(|name| self.env.get(name)).map(Source::Environment)
    }

    #[inline]
    pub fn stored(&self) -> Vec<Provider> {
        let credentials = self.entries.lock_unpoisoned();
        Provider::LOGINS.into_iter().filter(|p| credentials.contains_key(p.id())).collect()
    }

    /// A store that is never written to disk but still reads the process environment.
    #[must_use]
    #[inline]
    pub fn unsaved() -> Self {
        Self { path: None, entries: Mutex::default(), refresh: sync::Mutex::default(), env: Env::Process }
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    /// Sets an environment variable of an in-memory store.
    #[must_use]
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        if let Env::Fixed(ref mut vars) = self.env {
            drop(vars.insert(name.to_owned(), value.to_owned()));
        }
        self
    }

    #[inline]
    /// Saves `credential` for `provider`.
    ///
    /// # Errors
    ///
    /// Fails if the credentials cannot be saved.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    pub fn write(&self, provider: Provider, credential: Credential) -> io::Result<()> {
        let mut credentials = self.entries.lock_unpoisoned();
        drop(credentials.insert(provider.id().to_owned(), credential));
        self.persist(&credentials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::assert_empty;
    use std::env;
    use std::process;

    fn temp_path(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("{}-auth-{name}-{}", crate::APP_NAME, process::id()));
        let _unused = fs::remove_dir_all(&dir);
        dir.join("nested").join("auth.json")
    }

    fn key(value: &str) -> Credential {
        Credential::ApiKey { key: value.into() }
    }

    #[test]
    fn credentials_survive_a_restart() {
        let path = temp_path("restart");
        let store = Credentials::open(&path).unwrap();
        store.write(Provider::OpenAi, key("sk-1")).unwrap();
        store
            .write(Provider::Copilot, Credential::Oauth { refresh: "r".into(), access: "a".into(), expires: 5 })
            .unwrap();

        let reopened = Credentials::open(&path).unwrap();
        assert_eq!(reopened.read(Provider::OpenAi), Some(key("sk-1")));
        assert!(matches!(reopened.read(Provider::Copilot), Some(Credential::Oauth { expires: 5, .. })));
        assert_eq!(reopened.stored(), [Provider::Copilot, Provider::OpenAi]);
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn the_file_format_is_type_tagged_per_provider() {
        let path = temp_path("format");
        let store = Credentials::open(&path).unwrap();
        store.write(Provider::Anthropic, key("k")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["anthropic"]["type"], "api_key");
        assert_eq!(value["anthropic"]["key"], "k");
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_file_and_its_directory_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = temp_path("mode");
        Credentials::open(&path).unwrap().write(Provider::OpenAi, key("k")).unwrap();
        let file = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!((file, dir), (0o600, 0o700));
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn deleting_removes_the_credential_from_disk() {
        let path = temp_path("delete");
        let store = Credentials::open(&path).unwrap();
        store.write(Provider::OpenAi, key("k")).unwrap();
        store.delete(Provider::OpenAi).unwrap();
        assert_eq!(Credentials::open(&path).unwrap().read(Provider::OpenAi), None);
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_empty_store_and_a_corrupt_one_is_an_error() {
        let path = temp_path("corrupt");
        assert_empty(Credentials::open(&path).unwrap().stored());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        Credentials::open(&path).unwrap_err();
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn an_in_memory_store_keeps_values_without_touching_disk() {
        let store = Credentials::in_memory();
        store.write(Provider::OpenAi, key("k")).unwrap();
        assert_eq!(store.read(Provider::OpenAi), Some(key("k")));
    }

    #[test]
    fn a_saved_login_wins_over_the_environment_and_local_providers_need_nothing() {
        let store = Credentials::in_memory().with_env("OPENAI_API_KEY", "sk-env").with_env("ANTHROPIC_API_KEY", "sk-a");
        assert_eq!(store.source(Provider::Ollama), Some(Source::Local));
        assert_eq!(store.source(Provider::OpenAi), Some(Source::Environment("sk-env".into())));
        store.write(Provider::OpenAi, key("sk-stored")).unwrap();
        assert_eq!(store.source(Provider::OpenAi), Some(Source::Stored(key("sk-stored"))));
        assert_eq!(store.source(Provider::Copilot), None);
        assert!(store.can_use(Provider::Anthropic) && !store.can_use(Provider::Copilot));
    }

    #[test]
    fn an_empty_environment_variable_does_not_count() {
        let store = Credentials::in_memory().with_env("OPENAI_API_KEY", "");
        assert!(!store.can_use(Provider::OpenAi));
    }

    #[test]
    fn debug_output_hides_secrets() {
        assert!(!format!("{:?}", key("sk-secret")).contains("sk-secret"));
        let oauth = Credential::Oauth { refresh: "rrr".into(), access: "aaa".into(), expires: 1 };
        let shown = format!("{oauth:?}");
        assert!(!shown.contains("rrr") && !shown.contains("aaa"));
        assert!(!format!("{:?}", Source::Environment("sk-env".into())).contains("sk-env"));
    }
}
