//! Fetching each provider's models and storing them.

use crate::application::model_refresh::Event;
use crate::clock::now_ms;
use crate::domain::provider::Provider;

use super::fetch::{Fetched, ModelFetcher};
use super::store::Models as ModelsStore;

/// Fetches each provider in turn and replaces its models in the store. A provider that
/// fails or has no credentials keeps whatever it had before.
#[inline]
pub async fn run<Callback>(providers: &[Provider], fetcher: &ModelFetcher, store: &dyn ModelsStore, emit: Callback)
where
    Callback: Fn(Event) + Sync,
{
    for provider in providers {
        emit(match fetcher.fetch(provider).await {
            Ok(Fetched::Models(models)) => {
                let count = models.len();
                let updated = Provider { models, checked_at: now_ms(), ..provider.clone() };
                match store.save(&updated) {
                    Ok(()) => Event::Fetched { provider: provider.clone(), count },
                    Err(e) => {
                        Event::Failed { provider: provider.clone(), error: format!("cannot save the models: {e}") }
                    },
                }
            },
            Ok(Fetched::NoCredentials) => Event::Skipped(provider.clone()),
            Err(e) => Event::Failed { provider: provider.clone(), error: e.to_string() },
        });
    }
    emit(Event::Finished);
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use std::sync::Mutex;

    use super::*;
    use crate::adapters::auth::credential::Credential;
    use crate::adapters::auth::store::Credentials as CredentialStore;
    use crate::adapters::auth::testing::MockServer;
    use crate::adapters::models::store::InMemory as InMemoryModelsStore;
    use crate::adapters::models::testing::store_with;
    use crate::domain::provider::Protocol;

    async fn refresh(
        server: &MockServer,
        credentials: CredentialStore,
        store: &dyn ModelsStore,
        providers: &[Provider],
    ) -> Vec<Event> {
        let fetcher = ModelFetcher::new(Arc::new(credentials)).with_base_url(&server.url);
        let events = Mutex::new(Vec::new());
        run(providers, &fetcher, store, |event| events.lock().unwrap().push(event)).await;
        events.into_inner().unwrap()
    }

    #[tokio::test]
    async fn each_provider_is_fetched_and_saved_then_the_refresh_finishes() {
        let server = MockServer::start(vec![
            ("/models", vec![(200, r#"{"data":[{"id":"llama3"},{"id":"mistral"}]}"#.into())]),
            ("/v1/models", vec![(200, r#"{"data":[{"id":"claude-x","display_name":"Claude X"}]}"#.into())]),
        ])
        .await;
        let credentials = CredentialStore::in_memory();
        credentials.write(&Provider::builtin("anthropic").unwrap(), Credential::ApiKey { key: "k".into() }).unwrap();
        let store = InMemoryModelsStore::default();

        let events = refresh(
            &server,
            credentials,
            &store,
            &[Provider::builtin("ollama").unwrap(), Provider::builtin("anthropic").unwrap()],
        )
        .await;
        assert_eq!(
            events,
            [
                Event::Fetched { provider: Provider::builtin("ollama").unwrap(), count: 2 },
                Event::Fetched { provider: Provider::builtin("anthropic").unwrap(), count: 1 },
                Event::Finished,
            ]
        );
        assert_eq!(store.provider("ollama").unwrap().models.len(), 2);
        let anthropic = store.provider("anthropic").unwrap();
        assert_eq!(anthropic.models[0].name, "Claude X");
        assert!(anthropic.checked_at > 0);
    }

    #[tokio::test]
    async fn a_failed_provider_keeps_its_old_models_and_does_not_stop_the_others() {
        let server = MockServer::start(vec![
            ("/models", vec![(500, "{}".into())]),
            ("/v1/models", vec![(200, r#"{"data":[{"id":"claude-x"}]}"#.into())]),
        ])
        .await;
        let credentials = CredentialStore::in_memory();
        credentials.write(&Provider::builtin("anthropic").unwrap(), Credential::ApiKey { key: "k".into() }).unwrap();
        let store = store_with(&[("ollama", &[("old-model", Some(Protocol::OpenAi))])]);

        let events = refresh(
            &server,
            credentials,
            store.as_ref(),
            &[Provider::builtin("ollama").unwrap(), Provider::builtin("anthropic").unwrap()],
        )
        .await;
        assert!(matches!(&events[0], Event::Failed { provider, .. } if provider.id == "ollama"));
        assert_eq!(events[1], Event::Fetched { provider: Provider::builtin("anthropic").unwrap(), count: 1 });
        assert_eq!(store.provider("ollama").unwrap().models[0].id, "old-model");
    }

    #[tokio::test]
    async fn a_provider_without_credentials_is_skipped_and_untouched() {
        let server = MockServer::start(vec![]).await;
        let store = store_with(&[("openai", &[("cached", Some(Protocol::OpenAi))])]);
        let events =
            refresh(&server, CredentialStore::in_memory(), store.as_ref(), &[Provider::builtin("openai").unwrap()])
                .await;
        assert_eq!(events, [Event::Skipped(Provider::builtin("openai").unwrap()), Event::Finished]);
        assert_eq!(store.provider("openai").unwrap().models[0].id, "cached");
        assert!(server.requests().is_empty());
    }
}
