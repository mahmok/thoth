//! Refreshing the models each provider offers.

use crate::domain::provider::Provider;
use crate::notice::Notice;
use core::fmt::Debug;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Something a refresh reports.
pub enum Event {
    /// Fetching a provider's models failed.
    Failed {
        /// Which provider.
        provider: Provider,
        /// What went wrong.
        error: String,
    },
    /// A provider's models were stored.
    Fetched {
        /// Which provider.
        provider: Provider,
        /// How many models.
        count: usize,
    },
    /// All providers are done.
    Finished,
    /// A provider was skipped: not logged in.
    Skipped(Provider),
}

/// Runs a refresh in the background and reports what happened to each provider.
pub trait Worker: Debug {
    fn start(&self, providers: Vec<Provider>);

    /// The next thing the running refresh reported, if anything.
    fn try_next(&self) -> Option<Event>;
}

#[derive(Debug)]
/// A model refresh run in progress.
pub struct ModelRefreshRun {
    running: bool,
    worker: Box<dyn Worker>,
}

impl ModelRefreshRun {
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    #[inline]
    pub const fn is_running(&self) -> bool {
        self.running
    }

    #[must_use]
    #[inline]
    pub fn new(worker: Box<dyn Worker>) -> Self {
        Self { worker, running: false }
    }

    /// Turns a refresh event into notices.
    fn on_event(&mut self, event: Event) -> Vec<Notice> {
        match event {
            Event::Fetched { provider, count } => {
                let noun = if count == 1 { "model" } else { "models" };
                vec![Notice::Say(format!("{}: {count} {noun}", provider.display_name()))]
            },
            Event::Skipped(provider) => {
                vec![Notice::Say(format!("{}: skipped (not logged in)", provider.display_name()))]
            },
            Event::Failed { provider, error } => {
                vec![Notice::Fail(format!("{}: failed ({error})", provider.display_name()))]
            },
            Event::Finished => {
                self.running = false;
                Vec::new()
            },
        }
    }

    /// Takes in what the running refresh has reported since the last call.
    #[inline]
    pub fn poll(&mut self) -> Vec<Notice> {
        let mut notices = Vec::new();
        while let Some(event) = self.worker.try_next() {
            notices.extend(self.on_event(event));
        }
        notices
    }

    /// Starts the refresh. What it reports comes back through [`poll`](Self::poll).
    #[inline]
    pub fn start(&mut self, providers: Vec<Provider>) -> Vec<Notice> {
        if self.running {
            return vec![Notice::Say("A refresh is already running.".into())];
        }
        self.running = true;
        self.worker.start(providers);
        vec![Notice::Say("Refreshing models...".into())]
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;

    use super::*;
    use crate::adapters::models::testing::scripted::Worker as ScriptedRefreshWorker;

    struct Rig {
        run: ModelRefreshRun,
        worker: Rc<ScriptedRefreshWorker>,
    }

    impl Rig {
        fn poll(&mut self) -> Vec<Notice> {
            self.run.poll()
        }

        fn start(&mut self, providers: Vec<Provider>) -> Vec<Notice> {
            self.run.start(providers)
        }
    }

    fn rig() -> Rig {
        let worker = ScriptedRefreshWorker::new();
        let run = ModelRefreshRun::new(Box::new(Rc::clone(&worker)));
        Rig { run, worker }
    }

    #[test]
    fn a_refresh_announces_itself_and_reports_each_provider() {
        let mut r = rig();
        let providers = vec![
            Provider::builtin("ollama").unwrap(),
            Provider::builtin("openai").unwrap(),
            Provider::builtin("anthropic").unwrap(),
        ];
        let updates = r.start(providers.clone());
        assert_eq!(updates, [Notice::Say("Refreshing models...".into())]);
        assert_eq!(r.worker.started(), [providers]);
        assert!(r.run.is_running());

        r.worker.event(Event::Fetched { provider: Provider::builtin("ollama").unwrap(), count: 3 });
        r.worker.event(Event::Fetched { provider: Provider::builtin("openai").unwrap(), count: 1 });
        r.worker.event(Event::Skipped(Provider::builtin("github-copilot").unwrap()));
        r.worker.event(Event::Failed { provider: Provider::builtin("anthropic").unwrap(), error: "http error".into() });
        r.worker.event(Event::Finished);
        assert_eq!(
            r.poll(),
            [
                Notice::Say("Ollama: 3 models".into()),
                Notice::Say("OpenAI: 1 model".into()),
                Notice::Say("GitHub Copilot: skipped (not logged in)".into()),
                Notice::Fail("Anthropic: failed (http error)".into()),
            ]
        );
        assert!(!r.run.is_running());
    }

    #[test]
    fn a_second_refresh_while_one_runs_is_ignored() {
        let mut r = rig();
        r.start(vec![Provider::builtin("ollama").unwrap()]);
        assert_eq!(
            r.start(vec![Provider::builtin("openai").unwrap()]),
            [Notice::Say("A refresh is already running.".into())]
        );
        assert_eq!(r.worker.started().len(), 1);

        r.worker.event(Event::Finished);
        r.poll();
        assert_eq!(r.start(vec![Provider::builtin("openai").unwrap()]), [Notice::Say("Refreshing models...".into())]);
        assert_eq!(r.worker.started().len(), 2);
    }
}
