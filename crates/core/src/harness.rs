use crate::agent::controller::Controller as AgentController;
use crate::agent::controller::TokioAgent as TokioAgentController;
use crate::agent_builder::AgentBuilder;
use crate::auth::session::Login as LoginSession;
use crate::auth::store::{Credentials as CredentialStore, Source};
use crate::builtin::expand_skills;
use crate::builtin::files::{FileReferences, Suggestions};
use crate::llm::provider::{LOGIN_IDS, LoginMethod, ModelSpec, Provider};
use crate::models::choices;
use crate::models::refresh::Session as RefreshSession;
use crate::models::store::Models as ModelsStore;
use crate::notice::Notice;
use crate::persona::{Info as PersonaInfo, Repository as PersonaRepository};
use crate::services::Services;
use crate::session::LoginState;
use crate::session::Session;
use crate::session::event::Event as SessionEvent;
use crate::session::view::Login as LoginView;
use crate::transcript::Speaker;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::mem;

#[derive(Debug)]
/// What a Harness reports to clients.
pub enum Event {
    Login(LoginView),
    Notice { speaker: Speaker, text: String },
    Session { id: u64, event: SessionEvent },
    SessionClosed(u64),
}

#[derive(Debug)]
/// The Sessions and what they share.
pub struct Harness {
    builder: Arc<AgentBuilder>,
    credentials: Arc<CredentialStore>,
    files: Arc<FileReferences>,
    login: LoginView,
    logins: LoginSession,
    models: Arc<dyn ModelsStore>,
    next_session_id: u64,
    out: Vec<Event>,
    personas: Arc<PersonaRepository>,
    refresh: RefreshSession,
    sessions: BTreeMap<u64, Session>,
}

impl Harness {
    #[inline]
    /// Abandons the login in progress.
    pub fn cancel_login(&mut self) {
        let notices = self.logins.cancel();
        self.show(Speaker::Login, notices);
        self.sync_view();
    }

    #[inline]
    /// Interrupts the Session's running turn.
    pub fn cancel_session(&mut self, session_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.cancel();
        }
        self.sync_view();
    }

    #[inline]
    /// Cancels and removes a Session. Returns `false` if there was none.
    pub fn close_session(&mut self, session_id: u64) -> bool {
        let Some(mut session) = self.sessions.remove(&session_id) else {
            return false;
        };
        session.cancel();
        self.out.push(Event::SessionClosed(session_id));
        true
    }

    #[inline]
    /// Starts a Session under the next free id, and returns the id.
    pub fn create_session(&mut self, persona: &str) -> Option<u64> {
        while self.sessions.contains_key(&self.next_session_id) {
            self.next_session_id = self.next_session_id.saturating_add(1);
        }
        let id = self.next_session_id;
        self.insert_session(id, persona).then_some(id)
    }

    fn insert_session(&mut self, session_id: u64, persona: &str) -> bool {
        let Some(found) = self.personas.get(persona) else {
            self.say(Speaker::Error, format!("Persona {persona} not found."));
            return false;
        };
        let agent = Arc::new(Arc::clone(&self.builder).build_root(found, Arc::clone(&self.personas)));
        let session = Session::new(Box::new(TokioAgentController::new(agent)));
        drop(self.sessions.insert(session_id, session));
        true
    }

    #[must_use]
    #[inline]
    pub fn list_personas(&self) -> Vec<PersonaInfo> {
        self.personas.infos()
    }

    #[inline]
    /// Starts a login to `provider`.
    pub fn login(&mut self, provider: Provider) {
        let notices = self.logins.start(provider);
        self.show(Speaker::Login, notices);
        self.sync_view();
    }

    #[must_use]
    #[inline]
    /// Whether `provider` is logged in, and how.
    pub fn login_state(&self, provider: Provider) -> LoginState {
        match self.credentials.source(&provider) {
            Some(Source::Stored(_)) => LoginState::Saved,
            Some(Source::Environment(_)) => LoginState::FromEnvironment,
            Some(Source::Local) | None => LoginState::NotLoggedIn,
        }
    }

    #[must_use]
    #[inline]
    pub const fn login_view(&self) -> &LoginView {
        &self.login
    }

    #[inline]
    /// Removes the saved login of `provider`.
    pub fn logout(&mut self, provider: Provider) {
        let notices = self.logins.logout(provider);
        self.show(Speaker::Login, notices);
        self.sync_view();
    }

    #[must_use]
    #[inline]
    pub fn models(&self) -> Vec<(ModelSpec, String)> {
        choices(self.models.as_ref())
    }

    #[inline]
    /// Creates a Harness from its services.
    pub fn new(services: Services) -> Self {
        Self {
            logins: services.logins,
            refresh: services.refresh,
            credentials: services.credentials,
            files: services.files,
            models: services.models,
            sessions: BTreeMap::new(),
            personas: Arc::new(services.personas),
            builder: Arc::new(services.builder),
            login: LoginView::default(),
            next_session_id: 1,
            out: Vec::new(),
        }
    }

    #[inline]
    /// Collects what happened since the last poll: logins, refreshes and Sessions.
    pub fn poll(&mut self) -> Vec<Event> {
        let notices = self.logins.poll();
        self.show(Speaker::Login, notices);
        for provider in self.logins.take_logged_in() {
            self.start_refresh(vec![provider]);
        }
        let refreshed = self.refresh.poll();
        self.show(Speaker::Models, refreshed);
        let mut ids: Vec<u64> = self.sessions.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let events = match self.sessions.get_mut(&id) {
                Some(session) => session.poll(),
                None => continue,
            };
            self.out.extend(events.into_iter().map(|event| Event::Session { id, event }));
        }
        self.sync_view();
        mem::take(&mut self.out)
    }

    #[inline]
    pub fn refresh_models(&mut self) {
        let providers = self.models.providers().into_iter().map(|provider| provider.as_ref().clone()).collect();
        self.start_refresh(providers);
        self.sync_view();
    }

    #[must_use]
    #[inline]
    /// The providers a client can log in to: the built-ins in login order, then any custom
    /// provider that needs a credential.
    pub fn login_providers(&self) -> Vec<Provider> {
        let stored = self.models.providers();
        let mut providers: Vec<Provider> = LOGIN_IDS
            .iter()
            .filter_map(|id| stored.iter().find(|provider| provider.id == *id))
            .map(|provider| provider.as_ref().clone())
            .collect();
        providers.extend(
            stored
                .iter()
                .filter(|provider| {
                    provider.login_method() != LoginMethod::None && !LOGIN_IDS.contains(&provider.id.as_str())
                })
                .map(|provider| provider.as_ref().clone()),
        );
        providers
    }

    #[must_use]
    #[inline]
    pub fn saved_logins(&self) -> Vec<Provider> {
        self.credentials
            .stored()
            .into_iter()
            .filter_map(|id| self.models.read(&id).map(|provider| provider.as_ref().clone()))
            .collect()
    }

    fn say(&mut self, speaker: Speaker, text: String) {
        self.out.push(Event::Notice { speaker, text });
    }

    #[must_use]
    #[inline]
    pub fn session(&self, session_id: u64) -> Option<&Session> {
        self.sessions.get(&session_id)
    }

    #[inline]
    pub fn session_mut(&mut self, session_id: u64) -> Option<&mut Session> {
        self.sessions.get_mut(&session_id)
    }

    #[must_use]
    #[inline]
    /// The ids of the Sessions, with whether each is busy.
    pub fn sessions(&self) -> Vec<(u64, bool)> {
        let mut all: Vec<_> = self.sessions.iter().map(|(id, session)| (*id, session.view().busy)).collect();
        all.sort_unstable();
        all
    }

    fn show(&mut self, speaker: Speaker, notices: Vec<Notice>) {
        for notice in notices {
            match notice {
                Notice::Say(text) => self.say(speaker, text),
                Notice::Fail(text) => self.say(Speaker::Error, text),
            }
        }
    }

    fn start_refresh(&mut self, providers: Vec<Provider>) {
        let notices = self.refresh.start(providers);
        self.show(Speaker::Models, notices);
    }

    #[inline]
    /// Starts a Session with a Persona under `session_id`, unless one exists.
    pub fn start_session(&mut self, session_id: u64, persona: &str) {
        if !self.sessions.contains_key(&session_id) {
            let _inserted = self.insert_session(session_id, persona);
        }
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    /// Starts a Session with a given controller, for tests.
    pub fn start_session_with(&mut self, session_id: u64, controller: Box<dyn AgentController>) {
        let _session = self.sessions.entry(session_id).or_insert_with(|| Session::new(controller));
    }

    #[inline]
    /// Sends a prompt to a Session, or the secret being asked for. Returns whether it was taken.
    pub fn submit(&mut self, session_id: u64, shown: String, full: String) -> bool {
        let taken = self.submit_prompt(session_id, shown, full);
        self.sync_view();
        taken
    }

    /// Takes a prompt. A pending login secret goes to the login, anything else to the Session.
    fn submit_prompt(&mut self, session_id: u64, shown: String, full: String) -> bool {
        if shown.is_empty() {
            return false;
        }
        if self.logins.awaiting_secret() {
            self.logins.submit(full);
            return true;
        }

        let files = self.files.attach(&full);

        if let Some(session) = self.sessions.get_mut(&session_id) {
            if session.view().busy {
                return false;
            }
            session.say(Speaker::You, shown);
            session.emit(SessionEvent::Busy(true));
            let prompt = format!("{}{}", expand_skills(&full, session.agent_controller()), files);
            session.start_turn(prompt);
        } else {
            self.say(Speaker::Error, format!("No session {session_id} to submit the prompt to."));
            return false;
        }

        true
    }

    #[must_use]
    #[inline]
    pub fn suggest_files(&self) -> Suggestions {
        self.files.suggestions()
    }

    fn sync_view(&mut self) {
        let login = LoginView {
            active: self.logins.is_active(),
            awaiting_secret: self.logins.awaiting_secret(),
            device_code: self.logins.device_code().map(str::to_owned),
        };
        if login != self.login {
            self.login = login.clone();
            self.out.push(Event::Login(login));
        }
        for session in self.sessions.values_mut() {
            session.sync_view();
        }
    }

    #[inline]
    #[must_use]
    pub fn with_notice<IntoString>(mut self, notice: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        self.say(Speaker::Error, notice.into());
        self
    }
}
