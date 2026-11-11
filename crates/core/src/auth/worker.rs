use alloc::sync::Arc;
use std::sync::Mutex;

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::background::Background;
use crate::llm::provider::Provider;
use crate::sync::MutexExt as _;

use super::flow::{self, LoginEvent};
use super::github::{Endpoints, Github};
use super::session::LoginWorker;
use super::store::Credentials as CredentialStore;

#[derive(Debug)]
/// A login worker on tokio.
pub struct TokioLogin {
    answers: Mutex<Option<UnboundedSender<String>>>,
    github: Github,
    login: Background<LoginEvent>,
    store: Arc<CredentialStore>,
}

impl TokioLogin {
    #[inline]
    /// Creates a worker that saves to `store`.
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self {
            store,
            github: Github::new(Endpoints::github()),
            login: Background::default(),
            answers: Mutex::new(None),
        }
    }
}

impl LoginWorker for TokioLogin {
    #[inline]
    fn answer(&self, secret: String) {
        if let Some(answers) = self.answers.lock_unpoisoned().as_ref() {
            let _unused = answers.send(secret);
        }
    }

    #[inline]
    fn cancel(&self) {
        self.login.cancel();
        *self.answers.lock_unpoisoned() = None;
    }

    #[inline]
    fn start(&self, provider: Provider) {
        let (answers_tx, mut answers_rx) = unbounded_channel();
        *self.answers.lock_unpoisoned() = Some(answers_tx);
        let (store, github) = (Arc::clone(&self.store), self.github.clone());
        self.login.start(|events| async move {
            let emit = |event| {
                let _unused = events.send(event);
            };
            flow::run(provider, &github, &store, &mut answers_rx, emit).await;
        });
    }

    #[inline]
    fn try_next(&self) -> Option<LoginEvent> {
        self.login.try_next()
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::auth::credential::Credential;
    use tokio::time;

    async fn next_event(worker: &TokioLogin) -> LoginEvent {
        time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = worker.try_next() {
                    return event;
                }
                time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("an event should arrive")
    }

    #[tokio::test]
    async fn an_api_key_login_round_trips_through_the_worker() {
        let store = Arc::new(CredentialStore::in_memory());
        let worker = TokioLogin::new(Arc::clone(&store));
        worker.start(Provider::OpenAi);

        assert!(matches!(next_event(&worker).await, LoginEvent::PromptSecret { .. }));
        worker.answer("sk-worker".into());
        assert_eq!(next_event(&worker).await, LoginEvent::Done(Provider::OpenAi));
        assert_eq!(store.read(Provider::OpenAi), Some(Credential::ApiKey { key: "sk-worker".into() }));
    }

    #[tokio::test]
    async fn a_cancelled_login_reports_nothing_more_and_saves_nothing() {
        let store = Arc::new(CredentialStore::in_memory());
        let worker = TokioLogin::new(Arc::clone(&store));
        worker.start(Provider::Anthropic);
        assert!(matches!(next_event(&worker).await, LoginEvent::PromptSecret { .. }));
        worker.cancel();
        worker.answer("sk-late".into());
        time::sleep(Duration::from_millis(50)).await;
        assert!(worker.try_next().is_none());
        assert!(store.read(Provider::Anthropic).is_none());
    }

    #[tokio::test]
    async fn starting_again_replaces_the_running_login() {
        let store = Arc::new(CredentialStore::in_memory());
        let worker = TokioLogin::new(Arc::clone(&store));
        worker.start(Provider::OpenAi);
        assert!(matches!(next_event(&worker).await, LoginEvent::PromptSecret { .. }));

        worker.start(Provider::Anthropic);
        assert!(
            matches!(next_event(&worker).await, LoginEvent::PromptSecret { message } if message.contains("Anthropic"))
        );
        worker.answer("sk-ant".into());
        assert_eq!(next_event(&worker).await, LoginEvent::Done(Provider::Anthropic));
        assert!(store.read(Provider::OpenAi).is_none());
    }
}
