use alloc::sync::Arc;
use core::fmt::Debug;
use core::mem;

use crate::llm::provider::Provider;

use crate::notice::Notice;

use super::flow::LoginEvent;
use super::store::Credentials as CredentialStore;

/// Runs a login off the UI thread.
pub trait LoginWorker: Debug {
    fn answer(&self, secret: String);
    /// Stops the login; whatever it had not yet reported is dropped.
    fn cancel(&self);
    /// Starts a login, replacing any that is running.
    fn start(&self, provider: Provider);
    /// The next thing the running login reported, if anything.
    fn try_next(&self) -> Option<LoginEvent>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How far a login has got.
enum Phase {
    AwaitingSecret,
    Idle,
    Waiting,
}

#[derive(Debug)]
/// A login in progress.
pub struct Login {
    device_code: Option<String>,
    logged_in: Vec<Provider>,
    phase: Phase,
    store: Arc<CredentialStore>,
    worker: Box<dyn LoginWorker>,
}

impl Login {
    #[must_use]
    #[inline]
    pub fn awaiting_secret(&self) -> bool {
        self.phase == Phase::AwaitingSecret
    }

    #[inline]
    /// Abandons the login, if one is running.
    pub fn cancel(&mut self) -> Vec<Notice> {
        if !self.is_active() {
            return Vec::new();
        }
        self.worker.cancel();
        self.go(Phase::Idle);
        vec![Notice::say("Login cancelled.")]
    }

    /// The code to enter on the provider's page, while a device login waits for it.
    #[must_use]
    #[inline]
    pub fn device_code(&self) -> Option<&str> {
        self.device_code.as_deref()
    }

    fn go(&mut self, phase: Phase) {
        self.phase = phase;
        if phase == Phase::Idle {
            self.device_code = None;
        }
    }

    #[must_use]
    #[inline]
    pub fn is_active(&self) -> bool {
        self.phase != Phase::Idle
    }

    #[inline]
    #[must_use]
    /// Removes the saved login for `provider`.
    pub fn logout(&self, provider: Provider) -> Vec<Notice> {
        match self.store.delete(&provider) {
            Ok(()) => vec![Notice::say(format!("Logged out of {}.", provider.display_name()))],
            Err(e) => vec![Notice::fail(format!("cannot remove the saved login: {e}"))],
        }
    }

    #[inline]
    pub fn new(worker: Box<dyn LoginWorker>, store: Arc<CredentialStore>) -> Self {
        Self { worker, store, phase: Phase::Idle, device_code: None, logged_in: Vec::new() }
    }

    /// Turns what the login worker reports into notices.
    fn on_event(&mut self, event: LoginEvent) -> Vec<Notice> {
        if self.phase == Phase::Idle {
            return Vec::new();
        }
        match event {
            LoginEvent::DeviceCode { url, code } => {
                let text = format!(
                    "Open {url}\nand enter the code {code}\n(the code is copied to your clipboard; click the link to open it)"
                );
                self.device_code = Some(code);
                vec![Notice::say(text)]
            },
            LoginEvent::PromptSecret { message } => {
                self.go(Phase::AwaitingSecret);
                vec![Notice::say(message)]
            },
            LoginEvent::Done(provider) => {
                self.go(Phase::Idle);
                self.logged_in.push(provider.clone());
                vec![Notice::say(format!("Logged in to {}.", provider.display_name()))]
            },
            LoginEvent::Failed(message) => {
                self.go(Phase::Idle);
                vec![Notice::fail(message)]
            },
        }
    }

    /// Takes in what the running login has reported since the last call.
    #[inline]
    pub fn poll(&mut self) -> Vec<Notice> {
        let mut notices = Vec::new();
        while let Some(event) = self.worker.try_next() {
            notices.extend(self.on_event(event));
        }
        notices
    }

    /// Starts the login. What it reports comes back through [`poll`](Self::poll).
    #[inline]
    pub fn start(&mut self, provider: Provider) -> Vec<Notice> {
        self.worker.start(provider.clone());
        self.go(Phase::Waiting);
        vec![Notice::say(format!("Logging in to {}...", provider.display_name()))]
    }

    #[inline]
    /// Passes the secret the user typed to the login, if one was asked for.
    pub fn submit(&mut self, secret: String) {
        if self.phase == Phase::AwaitingSecret {
            self.worker.answer(secret);
            self.go(Phase::Waiting);
        }
    }

    /// The Providers that finished logging in since the last call.
    #[inline]
    pub fn take_logged_in(&mut self) -> Vec<Provider> {
        mem::take(&mut self.logged_in)
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;

    use super::*;
    use crate::auth::credential::Credential;
    use crate::auth::testing::scripted::Worker as ScriptedLoginWorker;
    use crate::test::assert_empty;

    struct Rig {
        session: Login,
        store: Arc<CredentialStore>,
        worker: Rc<ScriptedLoginWorker>,
    }

    impl Rig {
        fn poll(&mut self) -> Vec<Notice> {
            self.session.poll()
        }

        fn start(&mut self, provider: Provider) -> Vec<Notice> {
            self.session.start(provider)
        }
    }

    fn rig() -> Rig {
        let worker = ScriptedLoginWorker::new();
        let store = Arc::new(CredentialStore::in_memory());
        let session = Login::new(Box::new(Rc::clone(&worker)), Arc::clone(&store));
        Rig { session, store, worker }
    }

    fn says(updates: &[Notice]) -> Vec<&str> {
        updates
            .iter()
            .filter_map(|u| match *u {
                Notice::Say(ref text) => Some(text.as_str()),
                Notice::Fail(_) => None,
            })
            .collect()
    }

    #[test]
    fn starting_a_login_announces_it_and_becomes_active() {
        let mut r = rig();
        assert!(!r.session.is_active());
        let updates = r.start(Provider::copilot());
        assert_eq!(says(&updates), ["Logging in to GitHub Copilot..."]);
        assert_eq!(r.worker.started(), [Provider::copilot()]);
        assert!(r.session.is_active() && !r.session.awaiting_secret());
    }

    #[test]
    fn the_device_code_is_kept_and_shown_with_its_link_until_the_login_ends() {
        let mut r = rig();
        r.start(Provider::copilot());
        r.worker
            .event(LoginEvent::DeviceCode { url: "https://github.com/login/device".into(), code: "ABCD-1234".into() });
        let updates = r.poll();
        assert!(
            matches!(&updates[0], Notice::Say(t) if t.contains("https://github.com/login/device") && t.contains("ABCD-1234"))
        );
        assert_eq!(r.session.device_code(), Some("ABCD-1234"));
        assert!(r.session.is_active() && !r.session.awaiting_secret());

        r.worker.event(LoginEvent::Done(Provider::copilot()));
        r.poll();
        assert_eq!(r.session.device_code(), None);
    }

    #[test]
    fn asking_for_a_secret_switches_to_hidden_input_until_it_is_submitted() {
        let mut r = rig();
        r.start(Provider::openai());
        r.worker.event(LoginEvent::PromptSecret { message: "Paste your OpenAI API key".into() });
        assert_eq!(says(&r.poll()), ["Paste your OpenAI API key"]);
        assert!(r.session.awaiting_secret());

        r.session.submit("sk-1".into());
        assert_eq!(r.worker.answers(), ["sk-1"]);
        assert!(!r.session.awaiting_secret() && r.session.is_active());
    }

    #[test]
    fn a_secret_is_only_accepted_when_one_was_asked_for() {
        let mut r = rig();
        r.session.submit("sk-stray".into());
        r.start(Provider::copilot());
        r.session.submit("sk-stray".into());
        assert_empty(r.worker.answers());
    }

    #[test]
    fn finishing_goes_idle_and_reports_who_logged_in() {
        let mut r = rig();
        r.start(Provider::copilot());
        r.worker.event(LoginEvent::Done(Provider::copilot()));
        assert_eq!(r.poll(), [Notice::say("Logged in to GitHub Copilot.")]);
        assert_eq!(r.session.take_logged_in(), [Provider::copilot()]);
        assert_empty(r.session.take_logged_in());
        assert!(!r.session.is_active());
    }

    #[test]
    fn a_failure_reports_the_error_and_goes_idle() {
        let mut r = rig();
        r.start(Provider::copilot());
        r.worker.event(LoginEvent::Failed("device flow failed: access_denied".into()));
        assert_eq!(r.poll(), [Notice::fail("device flow failed: access_denied")]);
        assert!(!r.session.is_active());
    }

    #[test]
    fn cancelling_stops_the_worker_and_ignores_what_the_login_still_sends() {
        let mut r = rig();
        r.start(Provider::copilot());
        assert_eq!(r.session.cancel(), [Notice::say("Login cancelled.")]);
        assert_eq!(r.worker.cancelled(), 1);
        assert!(!r.session.is_active());

        r.worker.event(LoginEvent::Done(Provider::copilot()));
        assert_empty(r.poll());
    }

    #[test]
    fn cancelling_an_idle_session_does_nothing() {
        let mut r = rig();
        assert_empty(r.session.cancel());
        assert_eq!(r.worker.cancelled(), 0);
    }

    #[test]
    fn events_from_a_cancelled_login_are_ignored() {
        let mut r = rig();
        r.start(Provider::openai());
        r.worker.event(LoginEvent::PromptSecret { message: "Paste".into() });
        r.session.cancel();
        assert_empty(r.poll());
        assert!(!r.session.is_active() && !r.session.awaiting_secret());
    }

    #[test]
    fn events_with_no_login_running_are_ignored() {
        let mut r = rig();
        r.worker.event(LoginEvent::Done(Provider::copilot()));
        r.worker.event(LoginEvent::Failed("late".into()));
        assert_empty(r.poll());
        assert_empty(r.session.take_logged_in());
    }

    #[test]
    fn logging_out_removes_the_saved_login() {
        let r = rig();
        r.store.write(&Provider::copilot(), Credential::ApiKey { key: "k".into() }).unwrap();
        assert_eq!(r.session.logout(Provider::copilot()), [Notice::say("Logged out of GitHub Copilot.")]);
        assert!(r.store.read(&Provider::copilot()).is_none());
    }
}
