use alloc::rc::Rc;
use alloc::sync::Arc;

use crate::agent::Event as AgentEvent;
use crate::agent::context::Context as AgentContext;
use crate::agent::controller::Reply;
use crate::agent::controller::scripted::Controller as ScriptedAgent;
use crate::agent::skills::SkillSet;
use crate::agent_builder::AgentBuilder;
use crate::auth::credential::Credential;
use crate::auth::session::Login as LoginSession;
use crate::auth::store::Credentials as CredentialStore;
use crate::auth::testing::scripted::Worker as ScriptedLoginWorker;
use crate::builtin::files::FileReferences;
use crate::harness::{Event as HarnessEvent, Harness as CoreHarness};
use crate::llm::provider::Provider;
use crate::models::refresh::Session as RefreshSession;
use crate::models::store::InMemory as InMemoryModelsStore;
use crate::models::testing::scripted::Worker as ScriptedRefreshWorker;
use crate::persona::Repository as PersonaRepository;
use crate::services::Services;
use crate::session::Session;
use crate::session::event::Event as SessionEvent;
use crate::session::view::View;
use crate::transcript::{Content, Speaker};
use crate::workspace::Workspace;

/// The scripted collaborators a Harness is wired to.
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
    /// Creates the fixture: a scripted Agent, a store with the given logins, and the current
    /// directory.
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

/// A session wired to scripted collaborators, driven by calling it the way a client would.
#[derive(Debug)]
pub struct Harness {
    pub agent: Rc<ScriptedAgent>,
    pub events: Vec<SessionEvent>,
    pub files: Arc<FileReferences>,
    pub harness: CoreHarness,
    pub harness_events: Vec<HarnessEvent>,
    pub refresh_worker: Rc<ScriptedRefreshWorker>,
    pub store: Arc<CredentialStore>,
    /// The session this client is viewing; the harness has no say in it.
    pub viewing: u64,
    pub worker: Rc<ScriptedLoginWorker>,
}

impl Harness {
    fn build(agent: Rc<ScriptedAgent>, logged_in: &[Provider], models: Arc<InMemoryModelsStore>) -> Self {
        let scripted = Scripted::new(agent, logged_in);
        let mut harness = CoreHarness::new(scripted.services(models));
        harness.start_session_with(1, Box::new(Rc::clone(&scripted.agent)));
        Self {
            harness,
            viewing: 1,
            agent: scripted.agent,
            worker: scripted.worker,
            refresh_worker: scripted.refresh_worker,
            store: scripted.store,
            files: scripted.files,
            events: Vec::new(),
            harness_events: Vec::new(),
        }
    }

    pub fn cancel(&mut self) {
        self.harness.cancel_login();
        self.harness.cancel_session(self.viewing);
        self.poll();
    }

    pub fn done(&mut self) {
        self.reply(Reply::Done { usage: None });
    }

    #[must_use]
    pub fn new() -> Self {
        Self::build(ScriptedAgent::new(), &[], crate::test::fixtures::models())
    }

    pub fn poll(&mut self) {
        for event in self.harness.poll() {
            match event {
                HarnessEvent::Session { id, event: inner } if id == self.viewing => self.events.push(inner),
                HarnessEvent::Session { .. } => {},
                HarnessEvent::Notice { speaker, text } => self.session().say(speaker, text),
                other @ (HarnessEvent::Login(_) | HarnessEvent::SessionClosed(_)) => self.harness_events.push(other),
            }
        }
        let events = self.session().poll();
        self.events.extend(events);
    }

    pub fn reply(&mut self, reply: Reply) {
        self.agent.reply(reply);
        self.poll();
    }

    #[must_use]
    pub fn said(&self) -> Vec<(Speaker, String)> {
        self.view()
            .transcript
            .items()
            .iter()
            .filter_map(|item| match item.content {
                Content::Message(ref message) => Some((message.speaker, message.text.clone())),
                Content::Tool(_) => None,
            })
            .collect()
    }

    #[must_use]
    pub fn said_by(&self, speaker: Speaker) -> Vec<String> {
        self.said().into_iter().filter(|&(who, _)| who == speaker).map(|(_, text)| text).collect()
    }

    /// Submits `text`; true when the session took it.
    pub fn send(&mut self, text: &str) -> bool {
        self.harness.submit(self.viewing, text.into(), text.into())
    }

    /// The Session being viewed.
    ///
    /// # Panics
    ///
    /// Panics if the viewed Session does not exist.
    #[must_use]
    pub fn session(&mut self) -> &mut Session {
        self.harness.session_mut(self.viewing).unwrap()
    }

    pub fn text(&mut self, delta: &str) {
        self.reply(Reply::Event(AgentEvent::Text(delta.to_owned())));
    }

    /// What the viewed Session shows.
    ///
    /// # Panics
    ///
    /// Panics if the viewed Session does not exist.
    #[must_use]
    pub fn view(&self) -> &View {
        self.harness.session(self.viewing).unwrap().view()
    }

    #[must_use]
    pub fn with_agent(agent: Rc<ScriptedAgent>) -> Self {
        Self::build(agent, &[], crate::test::fixtures::models())
    }

    #[must_use]
    pub fn with_logged_in(providers: &[Provider]) -> Self {
        Self::build(ScriptedAgent::new(), providers, crate::test::fixtures::models())
    }

    #[must_use]
    pub fn with_models(models: Arc<InMemoryModelsStore>) -> Self {
        Self::build(ScriptedAgent::new(), &[], models)
    }
}

impl Default for Harness {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}
