use crate::auth::credential::now_ms;
use crate::llm::provider::Provider;
use crate::notice::Notice;
use core::fmt::Debug;

use super::fetch::{Fetched, ModelFetcher};
use super::store::Models as ModelsStore;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Something a refresh reports.
pub enum Event {
    /// Fetching a provider's models failed.
    Failed {
        /// Which provider.
        provider: Provider,
        /// What went wrong.
        error: String,
    },
    /// A provider's models were stored.
    Fetched {
        /// Which provider.
        provider: Provider,
        /// How many models.
        count: usize,
    },
    /// All providers are done.
    Finished,
    /// A provider was skipped: not logged in.
    Skipped(Provider),
}

/// Runs a refresh in the background and reports what happened to each provider.
pub trait Worker: Debug {
    fn start(&self, providers: Vec<Provider>);

    /// The next thing the running refresh reported, if anything.
    fn try_next(&self) -> Option<Event>;
}

#[derive(Debug)]
/// A refresh in progress.
pub struct Session {
    running: bool,
    worker: Box<dyn Worker>,
}

impl Session {
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    #[inline]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    #[must_use]
    #[inline]
    pub fn new(worker: Box<dyn Worker>) -> Self {
        Self { worker, running: false }
    }

    /// Turns a refresh event into notices.
    fn on_event(&mut self, event: Event) -> Vec<Notice> {
        match event {
            Event::Fetched { provider, count } => {
                let noun = if count == 1 { "model" } else { "models" };
                vec![Notice::Say(format!("{}: {count} {noun}", provider.display_name()))]
            },
            Event::Skipped(provider) => {
                vec![Notice::Say(format!("{}: skipped (not logged in)", provider.display_name()))]
            },
            Event::Failed { provider, error } => {
                vec![Notice::Fail(format!("{}: failed ({error})", provider.display_name()))]
            },
            Event::Finished => {
                self.running = false;
                Vec::new()
            },
        }
    }

    /// Takes in what the running refresh has reported since the last call.
    #[inline]
    pub fn poll(&mut self) -> Vec<Notice> {
        let mut notices = Vec::new();
        while let Some(event) = self.worker.try_next() {
            notices.extend(self.on_event(event));
        }
        notices
    }

    /// Starts the refresh. What it reports comes back through [`poll`](Self::poll).
    #[inline]
    pub fn start(&mut self, providers: Vec<Provider>) -> Vec<Notice> {
        if self.running {
            return vec![Notice::Say("A refresh is already running.".into())];
        }
        self.running = true;
        self.worker.start(providers);
        vec![Notice::Say("Refreshing models...".into())]
    }
}

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
                match store.write(&updated) {
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
    use alloc::rc::Rc;
    use alloc::sync::Arc;
    use std::sync::Mutex;

    use super::*;
    use crate::auth::credential::Credential;
    use crate::auth::store::Credentials as CredentialStore;
    use crate::auth::testing::MockServer;
    use crate::llm::provider::Protocol;
    use crate::models::store::InMemory as InMemoryModelsStore;
    use crate::models::testing::scripted::Worker as ScriptedRefreshWorker;
    use crate::models::testing::store_with;

    struct Rig {
        session: Session,
        worker: Rc<ScriptedRefreshWorker>,
    }

    impl Rig {
        fn poll(&mut self) -> Vec<Notice> {
            self.session.poll()
        }

        fn start(&mut self, providers: Vec<Provider>) -> Vec<Notice> {
            self.session.start(providers)
        }
    }

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
        credentials.write(&Provider::anthropic(), Credential::ApiKey { key: "k".into() }).unwrap();
        let store = InMemoryModelsStore::default();

        let events = refresh(&server, credentials, &store, &[Provider::ollama(), Provider::anthropic()]).await;
        assert_eq!(
            events,
            [
                Event::Fetched { provider: Provider::ollama(), count: 2 },
                Event::Fetched { provider: Provider::anthropic(), count: 1 },
                Event::Finished,
            ]
        );
        assert_eq!(store.read("ollama").unwrap().models.len(), 2);
        let anthropic = store.read("anthropic").unwrap();
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
        credentials.write(&Provider::anthropic(), Credential::ApiKey { key: "k".into() }).unwrap();
        let store = store_with(&[("ollama", &[("old-model", Some(Protocol::OpenAi))])]);

        let events = refresh(&server, credentials, store.as_ref(), &[Provider::ollama(), Provider::anthropic()]).await;
        assert!(matches!(&events[0], Event::Failed { provider, .. } if provider.id == "ollama"));
        assert_eq!(events[1], Event::Fetched { provider: Provider::anthropic(), count: 1 });
        assert_eq!(store.read("ollama").unwrap().models[0].id, "old-model");
    }

    #[tokio::test]
    async fn a_provider_without_credentials_is_skipped_and_untouched() {
        let server = MockServer::start(vec![]).await;
        let store = store_with(&[("openai", &[("cached", Some(Protocol::OpenAi))])]);
        let events = refresh(&server, CredentialStore::in_memory(), store.as_ref(), &[Provider::openai()]).await;
        assert_eq!(events, [Event::Skipped(Provider::openai()), Event::Finished]);
        assert_eq!(store.read("openai").unwrap().models[0].id, "cached");
        assert!(server.requests().is_empty());
    }

    fn rig() -> Rig {
        let worker = ScriptedRefreshWorker::new();
        let session = Session::new(Box::new(Rc::clone(&worker)));
        Rig { session, worker }
    }

    #[test]
    fn a_refresh_announces_itself_and_reports_each_provider() {
        let mut r = rig();
        let providers = vec![Provider::ollama(), Provider::openai(), Provider::anthropic()];
        let updates = r.start(providers.clone());
        assert_eq!(updates, [Notice::Say("Refreshing models...".into())]);
        assert_eq!(r.worker.started(), [providers]);
        assert!(r.session.is_running());

        r.worker.event(Event::Fetched { provider: Provider::ollama(), count: 3 });
        r.worker.event(Event::Fetched { provider: Provider::openai(), count: 1 });
        r.worker.event(Event::Skipped(Provider::copilot()));
        r.worker.event(Event::Failed { provider: Provider::anthropic(), error: "http error".into() });
        r.worker.event(Event::Finished);
        assert_eq!(
            r.poll(),
            [
                Notice::Say("Ollama: 3 models".into()),
                Notice::Say("OpenAI: 1 model".into()),
                Notice::Say("GitHub Copilot: skipped (not logged in)".into()),
                Notice::Fail("Anthropic: failed (http error)".into()),
            ]
        );
        assert!(!r.session.is_running());
    }

    #[test]
    fn a_second_refresh_while_one_runs_is_ignored() {
        let mut r = rig();
        r.start(vec![Provider::ollama()]);
        assert_eq!(r.start(vec![Provider::openai()]), [Notice::Say("A refresh is already running.".into())]);
        assert_eq!(r.worker.started().len(), 1);

        r.worker.event(Event::Finished);
        r.poll();
        assert_eq!(r.start(vec![Provider::openai()]), [Notice::Say("Refreshing models...".into())]);
        assert_eq!(r.worker.started().len(), 2);
    }
}
