use crate::agent::controller::Controller as AgentController;
use crate::utils::strings::expand;

/// What a prompt's skill reference starts with, such as `/skill:tdd`.
pub const SKILL_PREFIX: &str = "/skill:";

/// Replaces each `/skill:name` in `prompt` with that skill's block.
pub fn expand_skills(prompt: &str, agent: &dyn AgentController) -> String {
    expand(SKILL_PREFIX, prompt, |name| agent.skill_block(name))
}
