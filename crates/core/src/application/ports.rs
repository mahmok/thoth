//! Ports the application uses to drive the runtime.

use core::fmt::Debug;

use crate::adapters::llm::client::chat::Usage;
use crate::adapters::skills::SkillSummary;
use crate::runtime::agent::{AgentChange, AgentEvent, Settings};

#[derive(Debug)]
/// What the Agent task sends back.
pub enum Reply {
    Done { usage: Option<Usage> },
    Event(AgentEvent),
    Failed(String),
}

/// Runs a Turn and hands back what it reports.
pub trait TurnRunner: Debug {
    fn apply(&self, change: AgentChange);
    /// Compacts the Conversation, reporting through [`try_next`](Self::try_next) like a Turn.
    fn compact(&self, focus: Option<String>);
    fn settings(&self) -> Settings;
    fn skill_block(&self, name: &str) -> Option<String>;
    fn skills(&self) -> Vec<SkillSummary>;
    /// Starts a Turn for `prompt`. What it produces comes back through
    /// [`try_next`](Self::try_next).
    fn start(&self, prompt: String);
    /// The next thing the running Turn reported, if anything.
    fn try_next(&self) -> Option<Reply>;
}
