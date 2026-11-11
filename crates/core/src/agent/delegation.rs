//! How far a Root agent may delegate to Sub-agents. Set once at startup, like the context budget.

use crate::agent::DEFAULT_MAX_STEPS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Limits on delegating to Sub-agents.
#[expect(clippy::struct_field_names, reason = "the names follow the provider's wire format or what the type wraps")]
pub struct Delegation {
    /// Sub-agents that may exist in one Session.
    pub max_per_session: usize,
    /// Sub-agent Turns that may run at the same time in one Session.
    pub max_running: usize,
    /// The step limit of a Sub-agent's Turn.
    pub max_steps: usize,
}

impl Default for Delegation {
    #[inline]
    fn default() -> Self {
        Self { max_running: 4, max_per_session: 16, max_steps: DEFAULT_MAX_STEPS }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_four_running_sixteen_per_session_and_the_usual_steps() {
        let limits = Delegation::default();
        assert_eq!((limits.max_running, limits.max_per_session, limits.max_steps), (4, 16, DEFAULT_MAX_STEPS));
    }
}
