use crate::config;
use alloc::collections::BTreeMap;
use core::fmt::Debug;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::fsutil::write_private;
use crate::llm::provider::{Protocol, Provider};
use crate::sync::MutexExt as _;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredModel {
    #[serde(default, rename = "contextLimit", skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u32>,
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    #[serde(rename = "checkedAt")]
    pub checked_at: u64,
    pub models: Vec<StoredModel>,
}

pub trait Models: Debug + Send + Sync {
    fn read(&self, provider: Provider) -> Option<ModelEntry>;
    fn read_all(&self) -> Vec<(Provider, ModelEntry)>;
    fn write(&self, provider: Provider, entry: ModelEntry) -> io::Result<()>;
}

#[derive(Default, Debug)]
pub struct InMemory {
    entries: Mutex<BTreeMap<String, ModelEntry>>,
}

impl Models for InMemory {
    #[inline]
    fn read(&self, provider: Provider) -> Option<ModelEntry> {
        self.entries.lock_unpoisoned().get(provider.id()).cloned()
    }

    #[inline]
    fn read_all(&self) -> Vec<(Provider, ModelEntry)> {
        read_every(self)
    }

    #[inline]
    fn write(&self, provider: Provider, entry: ModelEntry) -> io::Result<()> {
        drop(self.entries.lock_unpoisoned().insert(provider.id().to_owned(), entry));
        Ok(())
    }
}

/// `models-store.json`: provider id → `{ checkedAt, models }`. An unreadable file counts as
/// an empty cache and is left alone until a refresh writes over it.
#[derive(Debug)]
pub struct Json {
    entries: Mutex<BTreeMap<String, ModelEntry>>,
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
                Ok(entries) => (entries, None),
                Err(e) => (
                    BTreeMap::new(),
                    Some(format!("Could not read {}: {e}. Run /refresh-models to rebuild it.", path.display())),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (BTreeMap::new(), None),
            Err(e) => (BTreeMap::new(), Some(format!("Could not read {}: {e}.", path.display()))),
        };
        (Self { path, entries: Mutex::new(entries) }, warning)
    }
}

impl Models for Json {
    #[inline]
    fn read(&self, provider: Provider) -> Option<ModelEntry> {
        self.entries.lock_unpoisoned().get(provider.id()).cloned()
    }

    #[inline]
    fn read_all(&self) -> Vec<(Provider, ModelEntry)> {
        read_every(self)
    }

    #[inline]
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    fn write(&self, provider: Provider, entry: ModelEntry) -> io::Result<()> {
        let mut entries = self.entries.lock_unpoisoned();
        drop(entries.insert(provider.id().to_owned(), entry));
        let json = serde_json::to_vec_pretty(&*entries).map_err(io::Error::other)?;
        write_private(&self.path, &json)
    }
}

/// Every provider's saved models, read one by one.
fn read_every(store: &dyn Models) -> Vec<(Provider, ModelEntry)> {
    Provider::ALL.into_iter().filter_map(|provider| Some((provider, store.read(provider)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::assert_empty;
    use std::env;
    use std::path::Path;
    use std::process;

    fn model(id: &str, protocol: Protocol) -> StoredModel {
        StoredModel { id: id.into(), name: format!("{id} name"), protocol, context_limit: None }
    }

    fn entry(models: Vec<StoredModel>) -> ModelEntry {
        ModelEntry { checked_at: 42, models }
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
        assert!(store.read(Provider::OpenAi).is_none() && store.read_all().is_empty());

        store.write(Provider::OpenAi, entry(vec![model("gpt-4.1", Protocol::OpenAi)])).unwrap();
        store.write(Provider::Ollama, entry(vec![model("llama3", Protocol::OpenAi)])).unwrap();
        assert_eq!(store.read(Provider::OpenAi).unwrap().models[0].id, "gpt-4.1");
        let order: Vec<Provider> = store.read_all().into_iter().map(|(p, _)| p).collect();
        assert_eq!(order, [Provider::Ollama, Provider::OpenAi], "read_all follows the provider order");

        store.write(Provider::OpenAi, entry(vec![model("gpt-4o", Protocol::OpenAi)])).unwrap();
        let models = store.read(Provider::OpenAi).unwrap().models;
        assert_eq!(models.len(), 1, "a write replaces the provider's entry");
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(store.read(Provider::Ollama).unwrap().models[0].id, "llama3", "other providers are untouched");
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
        store.write(Provider::Anthropic, entry(vec![model("claude-sonnet-4-5", Protocol::Anthropic)])).unwrap();

        let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["anthropic"]["checkedAt"], 42i32);
        assert_eq!(value["anthropic"]["models"][0]["id"], "claude-sonnet-4-5");
        assert_eq!(value["anthropic"]["models"][0]["protocol"], "anthropic");

        let reopened = Json::open(&path).0;
        assert_eq!(
            reopened.read(Provider::Anthropic),
            Some(entry(vec![model("claude-sonnet-4-5", Protocol::Anthropic)]))
        );
        cleanup(&path);
    }

    #[test]
    fn a_corrupt_file_is_an_empty_cache_with_a_warning_and_is_kept_until_a_write() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();

        let (store, warning) = Json::open(&path);
        assert!(warning.unwrap().contains("/refresh-models"));
        assert_empty(store.read_all());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{not json", "opening never rewrites the file");

        store.write(Provider::Ollama, entry(vec![model("llama3", Protocol::OpenAi)])).unwrap();
        assert!(Json::open(&path).1.is_none(), "a successful write replaces it");
        cleanup(&path);
    }

    #[test]
    fn a_missing_file_is_an_empty_cache_without_a_warning() {
        let path = temp_path("missing");
        let (store, warning) = Json::open(&path);
        assert!(warning.is_none() && store.read_all().is_empty());
    }
}
