use super::event::Event as SessionEvent;
use super::stats::Stats;
use crate::agent::Settings;
use crate::transcript::Transcript;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// A login as a client shows it.
pub struct Login {
    /// Whether a login is running.
    pub active: bool,
    /// Whether it waits for the user to paste a key.
    pub awaiting_secret: bool,
    /// The code to enter on GitHub, if one was issued.
    pub device_code: Option<String>,
}

#[derive(Debug, Clone, Default)]
/// What a client shows of a Session.
pub struct View {
    /// Whether the Agent is working.
    pub busy: bool,
    /// The model and thinking level.
    pub settings: Settings,
    /// Token statistics.
    pub stats: Stats,
    /// What the user sees.
    pub transcript: Transcript,
}

impl View {
    #[inline]
    /// Applies an event to the view.
    pub fn apply(&mut self, event: &SessionEvent) {
        match *event {
            SessionEvent::Transcript(ref change) => self.transcript.apply(change),
            SessionEvent::Busy(busy) => self.busy = busy,
            SessionEvent::Stats(stats) => self.stats = stats,
            SessionEvent::Settings(ref settings) => self.settings = settings.clone(),
        }
    }
}
