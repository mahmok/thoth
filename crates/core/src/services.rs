use crate::auth::store::Credentials as CredentialStore;
use alloc::sync::Arc;

use crate::auth::login::LoginAttempt;
use crate::builtin::files::FileReferences;
use crate::models::model_refresh::ModelRefreshRun;
use crate::models::store::Models as ModelsStore;
use crate::persona::Repository as PersonaRepository;
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
