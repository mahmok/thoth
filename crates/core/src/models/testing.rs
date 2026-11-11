/// A scripted refresh worker.
pub mod scripted;

use alloc::sync::Arc;

use crate::llm::provider::{Protocol, Provider};

use super::{InMemoryModelsStore, ModelEntry, ModelsStore as _, StoredModel};

/// The models of one provider, as id and protocol.
pub type Listing<'listing> = &'listing [(&'listing str, Protocol)];

#[must_use]
#[inline]
pub fn stored(id: &str, protocol: Protocol) -> StoredModel {
    StoredModel { id: id.into(), name: id.into(), protocol, context_limit: None }
}

#[must_use]
#[inline]
/// A store holding the given models.
///
/// # Panics
///
/// Panics if a store write fails.
pub fn store_with(entries: &[(Provider, Listing<'_>)]) -> Arc<InMemoryModelsStore> {
    let store = Arc::new(InMemoryModelsStore::default());
    for &(provider, models) in entries {
        let stored_models = models.iter().map(|&(id, protocol)| stored(id, protocol)).collect();
        store.write(provider, ModelEntry { checked_at: 1, models: stored_models }).unwrap();
    }
    store
}

/// A few models per provider, enough for the menus.
#[must_use]
#[inline]
pub fn fixture() -> Arc<InMemoryModelsStore> {
    use Protocol::{Anthropic, OpenAi};
    store_with(&[
        (Provider::Ollama, &[("bonsai-2:2b", OpenAi), ("llama3.2:3b", OpenAi), ("mistral:latest", OpenAi)]),
        (Provider::OpenAi, &[("gpt-4.1", OpenAi), ("gpt-4o", OpenAi)]),
        (Provider::Anthropic, &[("claude-sonnet-4-5", Anthropic)]),
        (Provider::Copilot, &[("claude-sonnet-4.5", Anthropic), ("gpt-4.1", OpenAi)]),
    ])
}
