use alloc::rc::Rc;
use alloc::sync::Arc;

use crate::auth::credential::Credential;
use crate::auth::session::Login as LoginSession;
use crate::auth::store::Credentials as CredentialStore;
use crate::auth::testing::scripted::Worker as ScriptedLoginWorker;
use crate::llm::provider::Provider;
use crate::models::refresh::Session as RefreshSession;
use crate::models::store::InMemory as InMemoryModelsStore;
use crate::models::testing::scripted::Worker as ScriptedRefreshWorker;
use crate::workspace::Workspace;

use crate::agent::context::Context as AgentContext;
use crate::agent::controller::scripted::Controller as ScriptedAgent;
use crate::agent::skills::SkillSet;
use crate::agent_builder::AgentBuilder;
use crate::builtin::files::FileReferences;
use crate::persona::Repository as PersonaRepository;
use crate::services::Services;

/// Scripted collaborators for a conversation, with handles the test keeps to drive and inspect them.
#[derive(Debug)]
#[expect(clippy::partial_pub_fields, reason = "the private fields keep the invariants of the type")]
pub struct Scripted {
    pub agent: Rc<ScriptedAgent>,
    pub files: Arc<FileReferences>,
    pub refresh_worker: Rc<ScriptedRefreshWorker>,
    pub store: Arc<CredentialStore>,
    pub worker: Rc<ScriptedLoginWorker>,
    workspace: Workspace,
}

impl Scripted {
    #[inline]
    /// Creates the fixture: an Agent, a store with the given logins and the current directory.
    ///
    /// # Panics
    ///
    /// Panics if the setup fails.
    pub fn new(agent: Rc<ScriptedAgent>, logged_in: &[Provider]) -> Self {
        let store = Arc::new(CredentialStore::in_memory());
        for provider in logged_in {
            store.write(provider, Credential::ApiKey { key: "k".into() }).unwrap();
        }
        let workspace = Workspace::from_current_dir().unwrap();
        Self {
            agent,
            worker: ScriptedLoginWorker::new(),
            refresh_worker: ScriptedRefreshWorker::new(),
            store,
            files: Arc::new(FileReferences::new(workspace.clone())),
            workspace,
        }
    }

    #[inline]
    /// The services of a Harness, wired to the fixture.
    pub fn services(&self, models: Arc<InMemoryModelsStore>) -> Services {
        Services {
            personas: PersonaRepository::new(),
            builder: AgentBuilder::new(
                Arc::clone(&self.store),
                models.clone(),
                self.workspace.clone(),
                AgentContext::new(self.workspace.root()),
                SkillSet::default(),
            ),
            logins: LoginSession::new(Box::new(Rc::clone(&self.worker)), Arc::clone(&self.store)),
            refresh: RefreshSession::new(Box::new(Rc::clone(&self.refresh_worker))),
            credentials: Arc::clone(&self.store),
            models,
            files: Arc::clone(&self.files),
        }
    }
}
