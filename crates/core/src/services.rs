use crate::auth::store::Credentials as CredentialStore;
use alloc::sync::Arc;

use crate::agent_builder::AgentBuilder;
use crate::auth::session::Login as LoginSession;
use crate::builtin::files::FileReferences;
use crate::models::refresh::Session as RefreshSession;
use crate::models::store::Models as ModelsStore;
use crate::persona::Repository as PersonaRepository;

#[derive(Debug)]
/// What a Harness is built from.
pub struct Services {
    pub builder: AgentBuilder,
    pub credentials: Arc<CredentialStore>,
    pub files: Arc<FileReferences>,
    pub logins: LoginSession,
    pub models: Arc<dyn ModelsStore>,
    pub personas: PersonaRepository,
    pub refresh: RefreshSession,
}
