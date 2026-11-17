use alloc::sync::Arc;

use crate::domain::provider::Provider;
use crate::runtime::background::Background;

use super::fetch::ModelFetcher;
use super::model_refresh;
use super::store::Models as ModelsStore;

#[derive(Debug)]
/// A refresh worker on tokio.
pub struct TokioRefresh {
    fetcher: Arc<ModelFetcher>,
    refresh: Background<model_refresh::Event>,
    store: Arc<dyn ModelsStore>,
}

impl TokioRefresh {
    #[inline]
    pub fn new(fetcher: ModelFetcher, store: Arc<dyn ModelsStore>) -> Self {
        Self { fetcher: Arc::new(fetcher), store, refresh: Background::default() }
    }
}

impl model_refresh::Worker for TokioRefresh {
    #[inline]
    fn start(&self, providers: Vec<Provider>) {
        let (fetcher, store) = (Arc::clone(&self.fetcher), Arc::clone(&self.store));
        self.refresh.start(|events| async move {
            model_refresh::run(&providers, &fetcher, store.as_ref(), |event| {
                let _unused = events.send(event);
            })
            .await;
        });
    }

    #[inline]
    fn try_next(&self) -> Option<model_refresh::Event> {
        self.refresh.try_next()
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::adapters::auth::store::Credentials as CredentialStore;
    use crate::adapters::auth::testing::MockServer;
    use crate::adapters::models::model_refresh;
    use crate::adapters::models::model_refresh::Worker as _;
    use crate::adapters::models::store::InMemory as InMemoryModelsStore;
    use tokio::time;

    #[tokio::test]
    async fn a_refresh_started_on_the_worker_reports_back_and_fills_the_store() {
        let server = MockServer::start(vec![("/models", vec![(200, r#"{"data":[{"id":"llama3"}]}"#.into())])]).await;
        let store = Arc::new(InMemoryModelsStore::default());
        let fetcher = ModelFetcher::new(Arc::new(CredentialStore::in_memory())).with_base_url(&server.url);
        let worker = TokioRefresh::new(fetcher, Arc::<InMemoryModelsStore>::clone(&store));
        worker.start(vec![Provider::builtin("ollama").unwrap()]);

        let mut events = Vec::new();
        time::timeout(Duration::from_secs(5), async {
            while !events.contains(&model_refresh::Event::Finished) {
                match worker.try_next() {
                    Some(event) => events.push(event),
                    None => time::sleep(Duration::from_millis(1)).await,
                }
            }
        })
        .await
        .expect("the refresh should finish");

        assert_eq!(
            events[0],
            model_refresh::Event::Fetched { provider: Provider::builtin("ollama").unwrap(), count: 1 }
        );
        assert_eq!(store.provider("ollama").unwrap().models[0].id, "llama3");
    }
}
