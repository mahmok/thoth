use crate::adapters::auth::login::LoginAttempt;
use crate::adapters::auth::store::{Credentials as CredentialStore, Source};
use crate::adapters::files::{FileReferences, Suggestions};
use crate::adapters::models::choices;
use crate::adapters::models::model_refresh::ModelRefreshRun;
use crate::adapters::models::store::Models as ModelsStore;
use crate::adapters::persona::{Info as PersonaInfo, Repository as PersonaRepository};
use crate::domain::provider::{LOGIN_IDS, LoginMethod, ModelSpec, Provider};
use crate::domain::shared::SessionId;
use crate::domain::thinking::Thinking;
use crate::domain::transcript::Speaker;
use crate::notice::Notice;
use crate::runtime::agent_builder::AgentBuilder;
use crate::runtime::turn_runner::TokioTurnRunner;
#[cfg(any(test, feature = "testing"))]
use crate::runtime::turn_runner::TurnRunner;
use crate::services::Services;
use crate::session::Session;
use crate::session::event::SessionEvent;
use crate::session::view::Login as LoginView;
use crate::session::{LoginState, Refusal};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::mem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Why a use case refused.
pub enum UseCaseError {
    /// The Session is working, so the use case may not run now.
    Busy,
    /// There is no Session under that id.
    MissingSession,
}

impl From<Refusal> for UseCaseError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Busy => Self::Busy,
        }
    }
}

#[derive(Debug)]
/// What a Harness reports to clients.
pub enum HarnessEvent {
    Login(LoginView),
    Notice { speaker: Speaker, text: String },
    Session { id: SessionId, event: SessionEvent },
    SessionClosed(SessionId),
}

#[derive(Debug)]
/// The Sessions and what they share.
pub struct Harness {
    builder: Arc<AgentBuilder>,
    credentials: Arc<CredentialStore>,
    files: Arc<FileReferences>,
    ids: ulid::Generator,
    login: LoginView,
    logins: LoginAttempt,
    models: Arc<dyn ModelsStore>,
    out: Vec<HarnessEvent>,
    personas: Arc<PersonaRepository>,
    refresh: ModelRefreshRun,
    sessions: BTreeMap<SessionId, Session>,
}

impl Harness {
    #[inline]
    /// Abandons the login in progress.
    pub fn cancel_login(&mut self) {
        let notices = self.logins.cancel();
        self.show(Speaker::Login, notices);
        self.sync_login();
    }

    #[inline]
    /// Interrupts the Session's running turn.
    pub fn cancel_session(&mut self, session_id: SessionId) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.cancel();
        }
        self.sync_login();
    }

    #[inline]
    /// Clears the Session's Conversation.
    ///
    /// # Errors
    ///
    /// Refused while the Session is busy, or when there is no such Session.
    pub fn clear_session(&mut self, session_id: SessionId) -> Result<(), UseCaseError> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Err(UseCaseError::MissingSession);
        };
        let cleared = session.clear().map_err(UseCaseError::from);
        self.sync_login();
        cleared
    }

    #[inline]
    /// Compacts the Session's Conversation now, with an optional `focus`.
    ///
    /// # Errors
    ///
    /// Refused while the Session is busy, or when there is no such Session.
    pub fn compact_session(&mut self, session_id: SessionId, focus: Option<String>) -> Result<(), UseCaseError> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Err(UseCaseError::MissingSession);
        };
        let compacted = session.compact(focus).map_err(UseCaseError::from);
        self.sync_login();
        compacted
    }

    #[inline]
    /// Switches the Session's model.
    ///
    /// # Errors
    ///
    /// Refused while the Session is busy, or when there is no such Session.
    pub fn set_model(&mut self, session_id: SessionId, model: ModelSpec) -> Result<(), UseCaseError> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Err(UseCaseError::MissingSession);
        };
        let switched = session.set_model(model).map_err(UseCaseError::from);
        self.sync_login();
        switched
    }

    #[inline]
    /// Sets how hard the Session's model thinks.
    ///
    /// # Errors
    ///
    /// Refused while the Session is busy, or when there is no such Session.
    pub fn set_thinking(&mut self, session_id: SessionId, level: Thinking) -> Result<(), UseCaseError> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Err(UseCaseError::MissingSession);
        };
        let set = session.set_thinking(level).map_err(UseCaseError::from);
        self.sync_login();
        set
    }

    #[inline]
    /// Writes a notice into the Session's transcript.
    ///
    /// # Errors
    ///
    /// Refused when there is no such Session, rather than dropping the notice.
    pub fn notice(&mut self, session_id: SessionId, speaker: Speaker, text: String) -> Result<(), UseCaseError> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Err(UseCaseError::MissingSession);
        };
        session.notice(speaker, text);
        Ok(())
    }

    #[inline]
    /// Cancels and removes a Session. Returns `false` if there was none.
    pub fn close_session(&mut self, session_id: SessionId) -> bool {
        let Some(mut session) = self.sessions.remove(&session_id) else {
            return false;
        };
        session.cancel();
        self.out.push(HarnessEvent::SessionClosed(session_id));
        true
    }

    #[inline]
    /// Starts a Session with a freshly minted id, and returns the id.
    pub fn create_session(&mut self, persona: &str) -> Option<SessionId> {
        let id = self.mint_session_id();
        self.insert_session(id, persona).then_some(id)
    }

    fn insert_session(&mut self, session_id: SessionId, persona: &str) -> bool {
        let Some(found) = self.personas.get(persona) else {
            self.say(Speaker::Error, format!("Persona {persona} not found."));
            return false;
        };
        let agent = Arc::new(Arc::clone(&self.builder).build_root(session_id, found, Arc::clone(&self.personas)));
        let session = Session::new(session_id, Box::new(TokioTurnRunner::new(agent)));
        drop(self.sessions.insert(session_id, session));
        true
    }

    /// An id from the Harness's own generator, so Sessions sort in the order they were created.
    fn mint_session_id(&mut self) -> SessionId {
        match self.ids.generate() {
            Ok(ulid) => SessionId::from(ulid),
            Err(overflow) => SessionId::from(overflow.commit_overflow_increment()),
        }
    }

    #[must_use]
    #[inline]
    pub fn list_personas(&self) -> Vec<PersonaInfo> {
        self.personas.infos()
    }

    #[inline]
    /// Starts a login to `provider`.
    pub fn login(&mut self, provider: &Provider) {
        let notices = self.logins.start(provider);
        self.show(Speaker::Login, notices);
        self.sync_login();
    }

    #[must_use]
    #[inline]
    /// Whether `provider` is logged in, and how.
    pub fn login_state(&self, provider: &Provider) -> LoginState {
        match self.credentials.source(provider) {
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
    pub fn logout(&mut self, provider: &Provider) {
        let notices = self.logins.logout(provider);
        self.show(Speaker::Login, notices);
        self.sync_login();
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
            ids: ulid::Generator::new(),
            out: Vec::new(),
        }
    }

    #[inline]
    /// Collects what happened since the last poll: logins, refreshes and Sessions.
    pub fn poll(&mut self) -> Vec<HarnessEvent> {
        let notices = self.logins.poll();
        self.show(Speaker::Login, notices);
        for provider in self.logins.take_logged_in() {
            self.start_refresh(vec![provider]);
        }
        let refreshed = self.refresh.poll();
        self.show(Speaker::Models, refreshed);
        let mut ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let events = match self.sessions.get_mut(&id) {
                Some(session) => session.poll(),
                None => continue,
            };
            self.out.extend(events.into_iter().map(|event| HarnessEvent::Session { id, event }));
        }
        self.sync_login();
        mem::take(&mut self.out)
    }

    #[inline]
    pub fn refresh_models(&mut self) {
        let providers = self.models.providers().into_iter().map(|provider| provider.as_ref().clone()).collect();
        self.start_refresh(providers);
        self.sync_login();
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
            .filter_map(|id| self.models.provider(&id).map(|provider| provider.as_ref().clone()))
            .collect()
    }

    fn say(&mut self, speaker: Speaker, text: String) {
        self.out.push(HarnessEvent::Notice { speaker, text });
    }

    #[must_use]
    #[inline]
    pub fn session(&self, session_id: SessionId) -> Option<&Session> {
        self.sessions.get(&session_id)
    }

    #[must_use]
    #[inline]
    /// The ids of the Sessions, with whether each is busy.
    pub fn sessions(&self) -> Vec<(SessionId, bool)> {
        let mut all: Vec<_> = self.sessions.iter().map(|(id, session)| (*id, session.is_busy())).collect();
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
    pub fn start_session(&mut self, session_id: SessionId, persona: &str) {
        if !self.sessions.contains_key(&session_id) {
            let _inserted = self.insert_session(session_id, persona);
        }
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    /// Starts a Session with a given runner, for tests.
    pub fn start_session_with(&mut self, session_id: SessionId, runner: Box<dyn TurnRunner>) {
        let _session = self.sessions.entry(session_id).or_insert_with(|| Session::new(session_id, runner));
    }

    #[inline]
    /// Sends a prompt to a Session, or the secret being asked for. Returns whether it was taken.
    pub fn submit(&mut self, session_id: SessionId, shown: String, full: String) -> bool {
        let taken = self.submit_prompt(session_id, shown, full);
        self.sync_login();
        taken
    }

    /// Takes a prompt. A pending login secret goes to the login, anything else to the Session.
    fn submit_prompt(&mut self, session_id: SessionId, shown: String, full: String) -> bool {
        if shown.is_empty() {
            return false;
        }
        if self.logins.awaiting_secret() {
            self.logins.submit(full);
            return true;
        }

        let files = self.files.attach(&full);

        let Some(session) = self.sessions.get_mut(&session_id) else {
            self.say(Speaker::Error, format!("No session {session_id} to submit the prompt to."));
            return false;
        };
        let prompt = format!("{}{}", session.expand_skills(&full), files);
        session.submit(shown, prompt).is_ok()
    }

    #[must_use]
    #[inline]
    pub fn suggest_files(&self) -> Suggestions {
        self.files.suggestions()
    }

    fn sync_login(&mut self) {
        let login = LoginView {
            active: self.logins.is_active(),
            awaiting_secret: self.logins.awaiting_secret(),
            device_code: self.logins.device_code().map(str::to_owned),
        };
        if login != self.login {
            self.login = login.clone();
            self.out.push(HarnessEvent::Login(login));
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

#[cfg(test)]
mod tests;
