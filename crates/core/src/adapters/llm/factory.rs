use alloc::sync::Arc;

use crate::adapters::auth::store::Credentials as CredentialStore;
use crate::adapters::llm::client::protocol::ProtocolClient;
use crate::application::ports::{ClientFactory, ModelClient};
use crate::domain::provider::ModelSpec;
use crate::domain::shared::SessionId;

#[derive(Debug)]
/// Builds a protocol client through the one auth resolution point.
pub struct Factory {
    store: Arc<CredentialStore>,
}

impl Factory {
    #[must_use]
    #[inline]
    pub const fn new(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }
}

impl ClientFactory for Factory {
    #[inline]
    fn client_for(&self, spec: &ModelSpec, session: SessionId) -> Result<Arc<dyn ModelClient>, String> {
        ProtocolClient::for_model(spec, &self.store, session)
            .map(|client| Arc::new(client) as Arc<dyn ModelClient>)
            .map_err(|error| error.to_string())
    }
}
