use crate::adapters::auth::store::Credentials as CredentialStore;
use alloc::sync::Arc;

use crate::adapters::files::FileReferences;
use crate::adapters::models::store::Models as ModelsStore;
use crate::adapters::persona::Repository as PersonaRepository;
use crate::application::login::LoginAttempt;
use crate::application::model_refresh::ModelRefreshRun;
use crate::runtime::agent_builder::AgentBuilder;

#[derive(Debug)]
pub struct Services {
    pub builder: AgentBuilder,
    pub credentials: Arc<CredentialStore>,
    pub files: Arc<FileReferences>,
    pub logins: LoginAttempt,
    pub models: Arc<dyn ModelsStore>,
    pub personas: PersonaRepository,
    pub refresh: ModelRefreshRun,
}
