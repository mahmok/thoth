/// Events of a Session.
pub mod event;
/// Token statistics.
pub mod stats;
/// What a Session shows clients.
pub mod view;

use crate::llm::provider::ModelSpec;
use crate::llm::thinking::Thinking;
use core::mem;

use crate::agent::Change as AgentChange;
use crate::agent::controller::{Controller as AgentController, Reply};
use crate::agent::skills::SkillSummary;
use crate::transcript::{Change as TranscriptChange, Speaker};

use event::Event as SessionEvent;
use stats::Stats;
use view::View;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Why a request was refused.
pub enum Refusal {
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Whether a provider is logged in.
pub enum LoginState {
    FromEnvironment,
    NotLoggedIn,
    Saved,
}

#[derive(Debug)]
/// One conversation and its view.
pub struct Session {
    agent: Box<dyn AgentController>,
    out: Vec<SessionEvent>,
    stats: Stats,
    view: View,
}

impl Session {
    #[must_use]
    #[inline]
    pub fn agent_controller(&self) -> &dyn AgentController {
        self.agent.as_ref()
    }

    fn announce_skills(&mut self) {
        let names: Vec<String> = self.agent.skills().into_iter().map(|s| s.name).collect();
        if !names.is_empty() {
            let text = format!("{} loaded\n{}", names.len(), names.join(", "));
            self.say(Speaker::Skills, text);
        }
    }

    #[inline]
    /// Interrupts the running turn, if there is one.
    pub fn cancel(&mut self) {
        if self.view.busy {
            self.agent.apply(AgentChange::Interrupt);
        }
        self.sync_view();
    }

    #[inline]
    /// Clears the conversation. Refused while the Agent is busy.
    ///
    /// # Errors
    ///
    /// Refused while the Agent is busy.
    pub fn clear(&mut self) -> Result<(), Refusal> {
        self.idle()?;
        self.agent.apply(AgentChange::ClearConversation);
        self.emit(SessionEvent::Transcript(TranscriptChange::Cleared));
        self.sync_view();
        Ok(())
    }

    /// Compacts the Conversation now. The Session is busy until it is done.
    #[inline]
    /// Summarises older turns now.
    ///
    /// # Errors
    ///
    /// Refused while the Agent is busy.
    pub fn compact(&mut self, focus: Option<String>) -> Result<(), Refusal> {
        self.idle()?;
        self.emit(SessionEvent::Busy(true));
        self.agent.compact(focus);
        Ok(())
    }

    #[inline]
    pub fn emit(&mut self, event: SessionEvent) {
        self.view.apply(&event);
        self.out.push(event);
    }

    const fn idle(&self) -> Result<(), Refusal> {
        if self.view.busy { Err(Refusal::Busy) } else { Ok(()) }
    }

    #[must_use]
    #[inline]
    /// Creates a Session around an Agent controller.
    pub fn new(agent: Box<dyn AgentController>) -> Self {
        let mut session = Self { agent, stats: Stats::default(), view: View::default(), out: Vec::new() };
        session.announce_skills();
        session.settle();
        session
    }

    /// Applies one reply of the Agent to the transcript and statistics.
    fn on_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Event(event) => {
                if let Some(change) = self.view.transcript.agent_event(event) {
                    self.emit(SessionEvent::Transcript(change));
                }
            },
            Reply::Done { usage } => {
                self.stats.record(usage);
                self.emit(SessionEvent::Busy(false));
            },
            Reply::Failed(text) => {
                self.say(Speaker::Error, text);
                self.emit(SessionEvent::Busy(false));
            },
        }
    }

    #[inline]
    /// Reads what the Agent reported and returns the view changes since the last poll.
    pub fn poll(&mut self) -> Vec<SessionEvent> {
        while let Some(reply) = self.agent.try_next() {
            self.on_reply(reply);
        }
        self.sync_view();
        mem::take(&mut self.out)
    }

    #[inline]
    pub fn say(&mut self, speaker: Speaker, text: String) {
        let change = self.view.transcript.said(speaker, text);
        self.emit(SessionEvent::Transcript(change));
    }

    #[inline]
    /// Switches the model. Refused while the Agent is busy.
    ///
    /// # Errors
    ///
    /// Refused while the Agent is busy.
    pub fn set_model(&mut self, model: ModelSpec) -> Result<(), Refusal> {
        self.idle()?;
        self.agent.apply(AgentChange::SetModel(model));
        self.sync_view();
        Ok(())
    }

    #[inline]
    pub fn set_thinking(&mut self, level: Thinking) {
        self.agent.apply(AgentChange::SetThinking(level));
        self.sync_view();
    }

    fn settle(&mut self) {
        self.sync_view();
        self.out.clear();
    }

    #[must_use]
    #[inline]
    pub fn skills(&self) -> Vec<SkillSummary> {
        self.agent.skills()
    }

    #[inline]
    pub fn start_turn(&self, expanded_prompt: String) {
        self.agent.start(expanded_prompt);
    }

    #[inline]
    /// Emits events for settings, statistics and busy state that changed.
    pub fn sync_view(&mut self) {
        let settings = self.agent.settings();
        if settings != self.view.settings {
            self.emit(SessionEvent::Settings(settings));
        }
        if self.stats != self.view.stats {
            self.emit(SessionEvent::Stats(self.stats));
        }
    }

    #[must_use]
    #[inline]
    pub const fn view(&self) -> &View {
        &self.view
    }
}

#[cfg(test)]
mod tests;
