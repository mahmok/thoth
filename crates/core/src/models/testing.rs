/// A scripted refresh worker.
pub mod scripted;

use alloc::sync::Arc;

use crate::llm::provider::{Model, Protocol};

use super::store::{InMemory as InMemoryModelsStore, Models as ModelsStore};

/// The models of one provider, as id and an optional protocol override.
pub type Listing<'listing> = &'listing [(&'listing str, Option<Protocol>)];

#[must_use]
#[inline]
pub fn stored(id: &str, protocol: Option<Protocol>) -> Model {
    Model::new(id).with_protocol(protocol)
}

#[must_use]
#[inline]
/// A store holding the given models. Providers left out keep the built-in defaults.
///
/// # Panics
///
/// Panics if a store write fails or an id is not a built-in provider.
pub fn store_with(entries: &[(&str, Listing<'_>)]) -> Arc<InMemoryModelsStore> {
    let store = Arc::new(InMemoryModelsStore::default());
    for &(id, models) in entries {
        let mut provider = store.read(id).expect("the provider is built in").as_ref().clone();
        provider.models = models.iter().map(|&(model, protocol)| stored(model, protocol)).collect();
        provider.checked_at = 1;
        store.write(&provider).unwrap();
    }
    store
}

/// A few models per provider, enough for the menus.
#[must_use]
#[inline]
pub fn fixture() -> Arc<InMemoryModelsStore> {
    use Protocol::{Anthropic, OpenAi};
    store_with(&[
        ("ollama", &[("bonsai-2:2b", Some(OpenAi)), ("llama3.2:3b", None), ("mistral:latest", None)]),
        ("openai", &[("gpt-4.1", None), ("gpt-4o", None)]),
        ("anthropic", &[("claude-sonnet-4-5", Some(Anthropic))]),
        ("github-copilot", &[("claude-sonnet-4.5", Some(Anthropic)), ("gpt-4.1", Some(OpenAi))]),
    ])
}
