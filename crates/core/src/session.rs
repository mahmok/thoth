/// Events of a Session.
pub mod event;
/// Token statistics.
pub mod stats;
/// What a Session shows clients.
pub mod view;

use crate::domain::provider::ModelSpec;
use crate::domain::thinking::Thinking;
use core::mem;

use crate::adapters::skills::SkillSummary;
use crate::domain::transcript::{Speaker, TranscriptChange};
use crate::runtime::agent::AgentChange;
use crate::runtime::turn_runner::{Reply, TurnRunner};

use crate::domain::shared::SessionId;
use event::SessionEvent;
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
/// The process manager for one conversation.
pub struct Session {
    runner: Box<dyn TurnRunner>,
    /// Whether the Agent is working. The Session owns this; [`View::busy`] is derived from it.
    busy: bool,
    id: SessionId,
    out: Vec<SessionEvent>,
    stats: Stats,
    view: View,
}

impl Session {
    fn announce_skills(&mut self) {
        let names: Vec<String> = self.runner.skills().into_iter().map(|s| s.name).collect();
        if !names.is_empty() {
            let text = format!("{} loaded\n{}", names.len(), names.join(", "));
            self.say(Speaker::Skills, text);
        }
    }

    #[must_use]
    #[inline]
    /// The identity of this conversation, which does not change with its model or its contents.
    pub const fn id(&self) -> SessionId {
        self.id
    }

    #[inline]
    /// Interrupts the running turn, if there is one.
    pub fn cancel(&mut self) {
        if self.busy {
            self.runner.apply(AgentChange::Interrupt);
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
        self.runner.apply(AgentChange::ClearConversation);
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
        self.busy = true;
        self.runner.compact(focus);
        self.sync_view();
        Ok(())
    }

    #[inline]
    fn emit(&mut self, event: SessionEvent) {
        self.view.apply(&event);
        self.out.push(event);
    }

    #[must_use]
    #[inline]
    /// Replaces each `/skill:name` in `prompt` with that skill's block.
    pub fn expand_skills(&self, prompt: &str) -> String {
        crate::adapters::skills::expand::expand_skills(prompt, self.runner.as_ref())
    }

    const fn idle(&self) -> Result<(), Refusal> {
        if self.busy { Err(Refusal::Busy) } else { Ok(()) }
    }

    #[must_use]
    #[inline]
    /// Whether the Agent is working. This is the Session's own state, not a projection.
    pub const fn is_busy(&self) -> bool {
        self.busy
    }

    #[must_use]
    #[inline]
    /// Creates a Session around a turn runner.
    pub fn new(id: SessionId, runner: Box<dyn TurnRunner>) -> Self {
        let mut session =
            Self { runner, busy: false, id, stats: Stats::default(), view: View::default(), out: Vec::new() };
        session.announce_skills();
        session.settle();
        session
    }

    /// Applies everything the Agent has reported, without projecting it yet.
    fn drain(&mut self) {
        while let Some(reply) = self.runner.try_next() {
            self.on_reply(reply);
        }
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
                self.busy = false;
            },
            Reply::Failed(text) => {
                self.say(Speaker::Error, text);
                self.busy = false;
            },
        }
    }

    #[inline]
    /// Reads what the Agent reported and returns the view changes since the last poll.
    pub fn poll(&mut self) -> Vec<SessionEvent> {
        self.drain();
        self.sync_view();
        mem::take(&mut self.out)
    }

    #[inline]
    /// Appends a Notice to the transcript.
    pub fn notice(&mut self, speaker: Speaker, text: String) {
        self.say(speaker, text);
    }

    #[inline]
    fn say(&mut self, speaker: Speaker, text: String) {
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
        self.runner.apply(AgentChange::SetModel(model));
        self.sync_view();
        Ok(())
    }

    /// Sets how hard the model thinks. Refused while the Agent is busy.
    ///
    /// # Errors
    ///
    /// Refused while the Agent is busy.
    #[inline]
    pub fn set_thinking(&mut self, level: Thinking) -> Result<(), Refusal> {
        self.idle()?;
        self.runner.apply(AgentChange::SetThinking(level));
        self.sync_view();
        Ok(())
    }

    fn settle(&mut self) {
        self.sync_view();
        self.out.clear();
    }

    #[must_use]
    #[inline]
    pub fn skills(&self) -> Vec<SkillSummary> {
        self.runner.skills()
    }

    /// Takes a prompt if the Agent is idle: shows it, becomes busy and starts the turn.
    ///
    /// # Errors
    ///
    /// Refused while the Agent is busy.
    pub fn submit(&mut self, shown: String, expanded_prompt: String) -> Result<(), Refusal> {
        // A turn that already finished is not a reason to refuse, even if nobody polled it.
        self.drain();
        self.idle()?;
        self.say(Speaker::You, shown);
        self.busy = true;
        self.runner.start(expanded_prompt);
        self.sync_view();
        Ok(())
    }

    #[inline]
    /// Emits events for settings, statistics and busy state that changed.
    fn sync_view(&mut self) {
        if self.busy != self.view.busy {
            self.emit(SessionEvent::Busy(self.busy));
        }
        let settings = self.runner.settings();
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
