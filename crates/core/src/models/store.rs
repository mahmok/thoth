use crate::config;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::fmt::Debug;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::fsutil::write_private;
use crate::llm::provider::{BUILTIN_IDS, Provider};
use crate::sync::MutexExt as _;

/// Where providers are kept between runs: one record per provider, keyed by id.
pub trait Models: Debug + Send + Sync {
    /// The provider saved under `id`.
    fn provider(&self, id: &str) -> Option<Arc<Provider>>;

    /// Every provider, in menu order: the built-ins first, then the ones the user added.
    fn providers(&self) -> Vec<Arc<Provider>>;

    /// Saves a provider record.
    ///
    /// # Errors
    ///
    /// Fails if the record cannot be saved.
    fn save(&self, provider: &Provider) -> io::Result<()>;
}

#[derive(Debug)]
/// A models store that lives in memory. It starts with the built-in providers.
pub struct InMemory {
    entries: Mutex<BTreeMap<String, Provider>>,
}

impl Default for InMemory {
    #[inline]
    fn default() -> Self {
        Self { entries: Mutex::new(builtins()) }
    }
}

impl Models for InMemory {
    #[inline]
    fn provider(&self, id: &str) -> Option<Arc<Provider>> {
        self.entries.lock_unpoisoned().get(id).map(|provider| Arc::new(provider.clone()))
    }

    #[inline]
    fn providers(&self) -> Vec<Arc<Provider>> {
        ordered(&self.entries.lock_unpoisoned())
    }

    #[inline]
    fn save(&self, provider: &Provider) -> io::Result<()> {
        drop(self.entries.lock_unpoisoned().insert(provider.id.clone(), provider.clone()));
        Ok(())
    }
}

/// `models-store.json`: provider id → the provider record. An unreadable file counts as the
/// built-ins and is left alone until a refresh writes over it.
#[derive(Debug)]
pub struct Json {
    entries: Mutex<BTreeMap<String, Provider>>,
    path: PathBuf,
}

impl Json {
    #[must_use]
    #[inline]
    pub fn default_path() -> Option<PathBuf> {
        config::dir().map(|dir| dir.join("models-store.json"))
    }

    /// Opens the file; the second value says why it could not be read, if it could not.
    #[inline]
    pub fn open<IntoPathbuf>(location: IntoPathbuf) -> (Self, Option<String>)
    where
        IntoPathbuf: Into<PathBuf>,
    {
        let path = location.into();
        let (entries, warning) = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(saved) => (merged(saved), None),
                Err(e) => (
                    builtins(),
                    Some(format!("Could not read {}: {e}. Run /refresh-models to rebuild it.", path.display())),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (builtins(), None),
            Err(e) => (builtins(), Some(format!("Could not read {}: {e}.", path.display()))),
        };
        (Self { path, entries: Mutex::new(entries) }, warning)
    }
}

impl Models for Json {
    #[inline]
    fn provider(&self, id: &str) -> Option<Arc<Provider>> {
        self.entries.lock_unpoisoned().get(id).map(|provider| Arc::new(provider.clone()))
    }

    #[inline]
    fn providers(&self) -> Vec<Arc<Provider>> {
        ordered(&self.entries.lock_unpoisoned())
    }

    #[inline]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    fn save(&self, provider: &Provider) -> io::Result<()> {
        let mut entries = self.entries.lock_unpoisoned();
        drop(entries.insert(provider.id.clone(), provider.clone()));
        let json = serde_json::to_vec_pretty(&*entries).map_err(io::Error::other)?;
        write_private(&self.path, &json)
    }
}

/// The built-in providers, keyed by id.
fn builtins() -> BTreeMap<String, Provider> {
    Provider::builtins().into_iter().map(|provider| (provider.id.clone(), provider)).collect()
}

/// Puts `saved` over the built-ins. The file does not repeat the id, so it is restored from the key.
fn merged(saved: BTreeMap<String, Provider>) -> BTreeMap<String, Provider> {
    let mut entries = builtins();
    for (id, mut provider) in saved {
        provider.id.clone_from(&id);
        drop(entries.insert(id, provider));
    }
    entries
}

/// The providers in menu order: the built-ins in [`BUILTIN_IDS`] order, then the rest by id.
fn ordered(entries: &BTreeMap<String, Provider>) -> Vec<Arc<Provider>> {
    let mut providers: Vec<Arc<Provider>> =
        BUILTIN_IDS.iter().filter_map(|id| entries.get(*id)).map(|provider| Arc::new(provider.clone())).collect();
    providers.extend(
        entries
            .iter()
            .filter(|(id, _)| !BUILTIN_IDS.contains(&id.as_str()))
            .map(|(_, provider)| Arc::new(provider.clone())),
    );
    providers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{Model, Protocol};
    use std::env;
    use std::path::Path;
    use std::process;

    fn model(id: &str, protocol: Protocol) -> Model {
        Model::new(id).with_name(format!("{id} name")).with_protocol(Some(protocol))
    }

    fn ids(providers: Vec<Arc<Provider>>) -> Vec<String> {
        providers.into_iter().map(|provider| provider.id.clone()).collect()
    }

    fn builtin_ids() -> Vec<String> {
        BUILTIN_IDS.iter().map(|id| (*id).to_owned()).collect()
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("{}-models-{name}-{}", crate::APP_NAME, process::id()));
        let _unused = fs::remove_dir_all(&dir);
        dir.join("nested").join("models-store.json")
    }

    fn cleanup(path: &Path) {
        fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    /// What every adapter has to do.
    fn behaves_like_a_models_store(store: &dyn Models) {
        assert_eq!(ids(store.providers()), builtin_ids(), "every built-in is there from the start");
        assert!(store.provider("gateway").is_none());
        assert_eq!(store.provider("openai").unwrap().models, Vec::new());

        let mut openai = (*store.provider("openai").unwrap()).clone();
        openai.models = vec![model("gpt-4.1", Protocol::OpenAi)];
        openai.checked_at = 42;
        store.save(&openai).unwrap();
        assert_eq!(store.provider("openai").unwrap().models[0].id, "gpt-4.1");

        let mut gateway = Provider::custom("gateway");
        gateway.models = vec![model("m", Protocol::OpenAi)];
        store.save(&gateway).unwrap();
        let mut order = builtin_ids();
        order.push("gateway".into());
        assert_eq!(ids(store.providers()), order, "custom providers follow the built-ins");

        let mut openai = (*store.provider("openai").unwrap()).clone();
        openai.models = vec![model("gpt-4o", Protocol::OpenAi)];
        store.save(&openai).unwrap();
        assert_eq!(store.provider("openai").unwrap().models.len(), 1, "a save replaces the provider's record");
        assert_eq!(store.provider("openai").unwrap().models[0].id, "gpt-4o");
        assert_eq!(store.provider("gateway").unwrap().models[0].id, "m", "other providers are untouched");
    }

    #[test]
    fn the_in_memory_store_is_a_models_store() {
        behaves_like_a_models_store(&InMemory::default());
    }

    #[test]
    fn the_json_store_is_a_models_store() {
        let path = temp_path("contract");
        behaves_like_a_models_store(&Json::open(&path).0);
        cleanup(&path);
    }

    #[test]
    fn the_json_store_survives_a_restart_and_uses_the_documented_format() {
        let path = temp_path("restart");
        let (store, warning) = Json::open(&path);
        assert!(warning.is_none());
        let mut anthropic = (*store.provider("anthropic").unwrap()).clone();
        anthropic.models = vec![model("claude-sonnet-4-5", Protocol::Anthropic)];
        anthropic.checked_at = 42;
        store.save(&anthropic).unwrap();

        let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["anthropic"]["checkedAt"], 42i32);
        assert_eq!(value["anthropic"]["displayName"], "Anthropic");
        assert_eq!(value["anthropic"]["protocol"], "anthropic");
        assert_eq!(value["anthropic"]["models"][0]["id"], "claude-sonnet-4-5");
        assert_eq!(value["anthropic"]["models"][0]["protocol"], "anthropic");

        let reopened = Json::open(&path).0;
        let saved = reopened.provider("anthropic").unwrap();
        assert_eq!(saved.checked_at, 42);
        assert_eq!(saved.models, vec![model("claude-sonnet-4-5", Protocol::Anthropic)]);
        cleanup(&path);
    }

    #[test]
    fn a_providers_session_header_survives_the_models_store() {
        let path = temp_path("session-header");
        let (store, _) = Json::open(&path);
        let gateway = (*store.provider("opencode-go").unwrap()).clone();
        assert_eq!(gateway.session_header.as_deref(), Some("x-opencode-session"));
        store.save(&gateway).unwrap();

        let written: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["opencode-go"]["sessionHeader"], "x-opencode-session");

        let reopened = Json::open(&path).0;
        let restored = reopened.provider("opencode-go").unwrap();
        assert_eq!(restored.session_header.as_deref(), Some("x-opencode-session"));
        cleanup(&path);
    }

    #[test]
    fn a_corrupt_file_leaves_the_builtins_with_a_warning_and_is_kept_until_a_write() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();

        let (store, warning) = Json::open(&path);
        assert!(warning.unwrap().contains("/refresh-models"));
        assert_eq!(ids(store.providers()), builtin_ids());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{not json", "opening never rewrites the file");

        let mut ollama = (*store.provider("ollama").unwrap()).clone();
        ollama.models = vec![model("llama3", Protocol::OpenAi)];
        store.save(&ollama).unwrap();
        assert!(Json::open(&path).1.is_none(), "a successful write replaces it");
        cleanup(&path);
    }

    #[test]
    fn a_missing_file_leaves_the_builtins_without_a_warning() {
        let path = temp_path("missing");
        let (store, warning) = Json::open(&path);
        assert!(warning.is_none());
        assert_eq!(ids(store.providers()), builtin_ids());
    }
}
