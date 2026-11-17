use crate::domain::message::AgentMessage;
use crate::domain::shared::Usage;

#[derive(Clone, Debug)]
/// The result of a Turn.
pub struct AgentResponse {
    pub message: AgentMessage,
    pub usage: Option<Usage>,
}
