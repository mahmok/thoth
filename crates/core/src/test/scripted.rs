use alloc::rc::Rc;
use alloc::sync::Arc;

use crate::adapters::auth::credential::Credential;
use crate::adapters::auth::store::Credentials as CredentialStore;
use crate::adapters::auth::testing::scripted::Worker as ScriptedLoginWorker;
use crate::adapters::models::store::InMemory as InMemoryModelsStore;
use crate::adapters::models::testing::scripted::Worker as ScriptedRefreshWorker;
use crate::adapters::workspace::Workspace;
use crate::application::login::LoginAttempt;
use crate::application::model_refresh::ModelRefreshRun;
use crate::application::ports::SystemClock;
use crate::domain::provider::Provider;

use crate::adapters::context::AgentContext;
use crate::adapters::files::FileReferences;
use crate::adapters::persona::Repository as PersonaRepository;
use crate::adapters::skills::SkillSet;
use crate::composition::services::Services;
use crate::runtime::agent_builder::AgentBuilder;
use crate::runtime::turn_runner::scripted::ScriptedTurnRunner;

/// Scripted collaborators for a conversation, with handles the test keeps to drive and inspect them.
#[derive(Debug)]
#[expect(clippy::partial_pub_fields, reason = "the private fields keep the invariants of the type")]
pub struct Scripted {
    pub agent: Rc<ScriptedTurnRunner>,
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
    pub fn new(agent: Rc<ScriptedTurnRunner>, logged_in: &[Provider]) -> Self {
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
            clock: Arc::new(SystemClock),
            builder: AgentBuilder::new(
                Arc::new(crate::adapters::llm::factory::Factory::new(Arc::clone(&self.store))),
                models.clone(),
                self.workspace.clone(),
                AgentContext::new(self.workspace.root()),
                SkillSet::default(),
            ),
            logins: LoginAttempt::new(Box::new(Rc::clone(&self.worker)), Arc::clone(&self.store)),
            refresh: ModelRefreshRun::new(Box::new(Rc::clone(&self.refresh_worker))),
            credentials: Arc::clone(&self.store),
            models,
            files: Arc::clone(&self.files),
        }
    }
}
