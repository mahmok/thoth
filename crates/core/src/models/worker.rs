use alloc::sync::Arc;

use crate::background::Background;
use crate::llm::provider::Provider;

use super::ModelsStore;
use super::fetch::ModelFetcher;
use super::refresh;
use super::refresh::{Event as RefreshEvent, Worker as RefreshWorker};

#[derive(Debug)]
/// A refresh worker on tokio.
pub struct TokioRefresh {
    fetcher: Arc<ModelFetcher>,
    refresh: Background<RefreshEvent>,
    store: Arc<dyn ModelsStore>,
}

impl TokioRefresh {
    #[inline]
    pub fn new(fetcher: ModelFetcher, store: Arc<dyn ModelsStore>) -> Self {
        Self { fetcher: Arc::new(fetcher), store, refresh: Background::default() }
    }
}

impl RefreshWorker for TokioRefresh {
    #[inline]
    fn start(&self, providers: Vec<Provider>) {
        let (fetcher, store) = (Arc::clone(&self.fetcher), Arc::clone(&self.store));
        self.refresh.start(|events| async move {
            refresh::run(&providers, &fetcher, store.as_ref(), |event| {
                let _unused = events.send(event);
            })
            .await;
        });
    }

    #[inline]
    fn try_next(&self) -> Option<RefreshEvent> {
        self.refresh.try_next()
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::auth::store::Credentials as CredentialStore;
    use crate::auth::testing::MockServer;
    use crate::models::store::InMemory as InMemoryModelsStore;
    use tokio::time;

    #[tokio::test]
    async fn a_refresh_started_on_the_worker_reports_back_and_fills_the_store() {
        let server = MockServer::start(vec![("/models", vec![(200, r#"{"data":[{"id":"llama3"}]}"#.into())])]).await;
        let store = Arc::new(InMemoryModelsStore::default());
        let fetcher = ModelFetcher::new(Arc::new(CredentialStore::in_memory())).with_base_url(&server.url);
        let worker = TokioRefresh::new(fetcher, Arc::<InMemoryModelsStore>::clone(&store));
        worker.start(vec![Provider::Ollama]);

        let mut events = Vec::new();
        time::timeout(Duration::from_secs(5), async {
            while !events.contains(&RefreshEvent::Finished) {
                match worker.try_next() {
                    Some(event) => events.push(event),
                    None => time::sleep(Duration::from_millis(1)).await,
                }
            }
        })
        .await
        .expect("the refresh should finish");

        assert_eq!(events[0], RefreshEvent::Fetched { provider: Provider::Ollama, count: 1 });
        assert_eq!(store.read(Provider::Ollama).unwrap().models[0].id, "llama3");
    }
}
