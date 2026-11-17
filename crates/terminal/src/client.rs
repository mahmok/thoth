use thoth_core::builtin::files::Suggestions;
use thoth_core::harness::{Harness, HarnessEvent, UseCaseError};
use thoth_core::llm::provider::{ModelSpec, Provider};
use thoth_core::llm::thinking::Thinking;
use thoth_core::persona::Info as PersonaInfo;
use thoth_core::session::LoginState;
use thoth_core::session::Session;
use thoth_core::session::SessionId;
use thoth_core::session::view::{Login as LoginView, View};
use thoth_core::transcript::Speaker;

/// A Harness and the one session this terminal is showing. Which session is viewed is the
/// terminal's business; the Harness only knows its sessions by id.
///
/// The façade exposes what the app and the command layer call, and nothing else: in
/// particular it offers no way to close a session except [`close`](Self::close), which moves
/// the view first, so "the viewed session exists" cannot be bypassed.
pub struct TerminalClient {
    harness: Harness,
    session: SessionId,
}

impl TerminalClient {
    /// Abandons a login in progress and interrupts the viewed session's turn.
    pub fn cancel(&mut self) {
        self.harness.cancel_login();
        self.harness.cancel_session(self.session);
    }

    /// Clears the viewed session's conversation.
    ///
    /// # Errors
    ///
    /// Refused while the session is busy, or when there is no such session.
    pub fn clear(&mut self) -> Result<(), UseCaseError> {
        self.harness.clear_session(self.session)
    }

    /// Closes `session`, moving the view to the earliest one left if it was shown. The last
    /// session is never closed.
    pub fn close(&mut self, session: SessionId) -> bool {
        let ids: Vec<SessionId> = self.harness.sessions().iter().map(|&(id, _)| id).collect();
        if ids.len() < 2 || !ids.contains(&session) {
            return false;
        }
        if !self.harness.close_session(session) {
            return false;
        }
        if self.session == session {
            self.session = ids.into_iter().find(|id| *id != session).unwrap_or(self.session);
        }
        true
    }

    /// Summarises the viewed session's older turns now.
    ///
    /// # Errors
    ///
    /// Refused while the session is busy, or when there is no such session.
    pub fn compact(&mut self, focus: Option<String>) -> Result<(), UseCaseError> {
        self.harness.compact_session(self.session, focus)
    }

    /// Starts a Session playing `persona`, without moving the view to it.
    pub fn create_session(&mut self, persona: &str) -> Option<SessionId> {
        self.harness.create_session(persona)
    }

    /// The login as it stands, for the header, the prompt and its hints.
    pub const fn login_view(&self) -> &LoginView {
        self.harness.login_view()
    }

    /// The providers that can be logged in to.
    pub fn login_providers(&self) -> Vec<Provider> {
        self.harness.login_providers()
    }

    /// Whether `provider` is logged in, and how.
    pub fn login_state(&self, provider: &Provider) -> LoginState {
        self.harness.login_state(provider)
    }

    /// Starts a login to `provider`.
    pub fn login(&mut self, provider: &Provider) {
        self.harness.login(provider);
    }

    /// Removes the saved login of `provider`.
    pub fn logout(&mut self, provider: &Provider) {
        self.harness.logout(provider);
    }

    /// The models the providers offer, with the name each is known by.
    pub fn models(&self) -> Vec<(ModelSpec, String)> {
        self.harness.models()
    }

    /// Views the harness's first session, or starts one on its first agent.
    pub fn open(mut harness: Harness) -> Result<Self, &'static str> {
        let session = if let Some(&(id, _)) = harness.sessions().first() {
            id
        } else {
            let persona = harness.list_personas().first().map(|p| p.key.clone());
            persona
                .and_then(|name| harness.create_session(&name))
                .ok_or("there is no persona to start a session with")?
        };
        Ok(Self { harness, session })
    }

    /// Writes a notice into the viewed session's transcript.
    ///
    /// # Errors
    ///
    /// Refused when there is no such session, rather than dropping the notice.
    pub fn notice(&mut self, speaker: Speaker, text: String) -> Result<(), UseCaseError> {
        self.harness.notice(self.session, speaker, text)
    }

    /// What the harness has to report: logins, notices, session events and closes.
    pub fn poll(&mut self) -> Vec<HarnessEvent> {
        self.harness.poll()
    }

    /// Asks the providers for their models again.
    pub fn refresh_models(&mut self) {
        self.harness.refresh_models();
    }

    /// The personas a session can be started with.
    pub fn personas(&self) -> Vec<PersonaInfo> {
        self.harness.list_personas()
    }

    /// The saved logins that can be removed.
    pub fn saved_logins(&self) -> Vec<Provider> {
        self.harness.saved_logins()
    }

    #[expect(clippy::expect_used, reason = "`close` keeps the viewed session in the harness")]
    pub fn session(&self) -> &Session {
        self.harness.session(self.session).expect("the viewed session exists")
    }

    pub const fn session_id(&self) -> SessionId {
        self.session
    }

    /// Switches the viewed session's model.
    ///
    /// # Errors
    ///
    /// Refused while the session is busy, or when there is no such session.
    pub fn set_model(&mut self, model: ModelSpec) -> Result<(), UseCaseError> {
        self.harness.set_model(self.session, model)
    }

    /// Sets how hard the viewed session's model thinks.
    ///
    /// # Errors
    ///
    /// Refused while the session is busy, or when there is no such session.
    pub fn set_thinking(&mut self, level: Thinking) -> Result<(), UseCaseError> {
        self.harness.set_thinking(self.session, level)
    }

    /// Every session's id, and whether it is working.
    pub fn sessions(&self) -> Vec<(SessionId, bool)> {
        self.harness.sessions()
    }

    pub fn submit(&mut self, shown: String, full: String) -> bool {
        self.harness.submit(self.session, shown, full)
    }

    /// The files a `@` word can complete to.
    pub fn suggest_files(&self) -> Suggestions {
        self.harness.suggest_files()
    }

    /// Shows `session` instead; false when there is no such session.
    pub fn switch(&mut self, session: SessionId) -> bool {
        let exists = self.harness.session(session).is_some();
        if exists {
            self.session = session;
        }
        exists
    }

    #[expect(clippy::expect_used, reason = "`close` keeps the viewed session in the harness")]
    pub fn view(&self) -> &View {
        self.harness.session(self.session).expect("the viewed session exists").view()
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::sync::Arc;

    use super::*;
    use thoth_core::agent::turn_runner::scripted::ScriptedTurnRunner;
    use thoth_core::llm::provider::Model;
    use thoth_core::models::testing::fixture;
    use thoth_core::persona::Persona;
    use thoth_core::test::scripted::Scripted;

    /// A façade over a harness with one session that a `helper` persona can add to.
    fn client() -> TerminalClient {
        let scripted = Scripted::new(ScriptedTurnRunner::new(), &[]);
        let mut services = scripted.services(fixture());
        services.personas.add(Persona::new("helper", "Helper", "helps")).unwrap();
        let mut harness = Harness::new(services);
        harness.start_session_with(SessionId::mint(), Box::new(Rc::clone(&scripted.agent)));
        TerminalClient::open(harness).unwrap()
    }

    /// The invariant every exposed path must keep: the viewed id names a session that exists
    /// and can be viewed.
    fn views_a_session(client: &TerminalClient) -> bool {
        let viewed = client.session_id();
        let consistent = client.sessions().iter().any(|&(id, _)| id == viewed);
        let _view = client.view();
        consistent
    }

    #[test]
    fn closing_the_viewed_session_leaves_the_facade_viewing_another() {
        let mut client = client();
        let first = client.session_id();
        let other = client.create_session("helper").expect("the helper persona exists");
        assert_eq!(client.session_id(), first);

        assert!(client.close(first));

        assert_eq!(client.session_id(), other);
        assert!(views_a_session(&client), "the view moved with the close");
    }

    #[test]
    fn closing_another_session_keeps_the_view_where_it_was() {
        let mut client = client();
        let first = client.session_id();
        let other = client.create_session("helper").expect("the helper persona exists");

        assert!(client.close(other));

        assert_eq!(client.session_id(), first);
        assert!(views_a_session(&client));
    }

    #[test]
    fn the_last_session_is_never_closed_and_the_view_does_not_move() {
        let mut client = client();
        let first = client.session_id();

        assert!(!client.close(first));

        assert_eq!(client.session_id(), first);
        assert!(views_a_session(&client), "the last session stays viewable");
    }

    #[test]
    fn switching_to_a_session_that_does_not_exist_changes_nothing() {
        let mut client = client();
        let first = client.session_id();

        assert!(!client.switch(SessionId::mint()));

        assert_eq!(client.session_id(), first);
        assert!(views_a_session(&client));
    }

    #[test]
    fn a_use_case_on_a_busy_session_is_refused_as_busy() {
        let mut client = client();
        assert!(client.submit("hello".into(), "hello".into()));
        let other = ModelSpec::new(Arc::new(Provider::builtin("openai").unwrap()), Model::new("gpt-4o"));

        assert_eq!(client.clear(), Err(UseCaseError::Busy));
        assert_eq!(client.compact(None), Err(UseCaseError::Busy));
        assert_eq!(client.set_model(other), Err(UseCaseError::Busy));
        assert_eq!(client.set_thinking(Thinking::Low), Err(UseCaseError::Busy));
    }

    #[test]
    fn a_use_case_for_a_session_that_is_gone_is_refused_distinctly() {
        let mut client = client();
        let other = ModelSpec::new(Arc::new(Provider::builtin("openai").unwrap()), Model::new("gpt-4o"));
        // A session can only go missing from under the façade if its id is stale; keep the view
        // pointing at nothing to prove the refusal is not dropped.
        client.session = SessionId::mint();

        assert_eq!(client.clear(), Err(UseCaseError::MissingSession));
        assert_eq!(client.compact(None), Err(UseCaseError::MissingSession));
        assert_eq!(client.set_model(other), Err(UseCaseError::MissingSession));
        assert_eq!(client.set_thinking(Thinking::Low), Err(UseCaseError::MissingSession));
        assert_eq!(client.notice(Speaker::Error, "gone".into()), Err(UseCaseError::MissingSession));
    }
}
