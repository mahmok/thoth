use crate::agent::context::{Budget, Compaction, Elision};
use crate::utils::numbers;

/// The tokens a request would carry: the Conversation, the Tool definitions, and a prompt that is
/// not in the Conversation yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Load {
    /// Tokens of the messages already in the Conversation.
    pub conversation: usize,
    /// Tokens the Tool definitions cost in every request.
    pub tools: usize,
    /// Tokens of a prompt that is about to be added to the Conversation.
    pub incoming: usize,
}

impl Load {
    #[must_use]
    #[inline]
    pub const fn new(conversation: usize, tools: usize) -> Self {
        Self { conversation, tools, incoming: 0 }
    }

    #[must_use]
    #[inline]
    pub const fn with_incoming(self, incoming: usize) -> Self {
        Self { incoming, ..self }
    }

    /// The estimate as it is, before the provider's correction.
    #[must_use]
    #[inline]
    pub const fn raw(self) -> usize {
        self.conversation.saturating_add(self.tools).saturating_add(self.incoming)
    }

    /// The estimate as the provider would count it, corrected by `calibration`.
    #[must_use]
    #[inline]
    pub fn tokens(self, calibration: f32) -> usize {
        numbers::scaled(self.raw(), calibration)
    }
}

/// Where a request sits in a Turn. It decides when old Tool output is worth dropping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before the Turn's first request. Elision waits until the context is half way to compaction,
    /// because dropping old Tool output costs the provider's prompt cache.
    BeforeTurn,
    /// Between the steps of a Turn, when the model has already read the old Tool output.
    BetweenSteps,
}

/// What to do before sending a request, with the rule to do it by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    /// The request fits: send it as it is.
    Keep,
    /// Drop old Tool output by these rules, then ask again.
    Shrink(Elision),
    /// Summarise older turns, keeping these many tokens of the newest ones.
    Compact(Compaction),
}

/// What the budget is asked about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    /// The tokens the request would carry.
    pub load: Load,
    /// How much real prompt tokens one estimated token turned out to be at the last request.
    pub calibration: f32,
    /// The model's context window, when it names one.
    pub limit: Option<u32>,
    /// Whether the request opens a Turn or follows a step.
    pub phase: Phase,
    /// Whether old Tool output has already been dropped for this request.
    pub elided: bool,
}

impl Request {
    #[must_use]
    #[inline]
    pub const fn before_turn(load: Load, calibration: f32, limit: Option<u32>) -> Self {
        Self { load, calibration, limit, phase: Phase::BeforeTurn, elided: false }
    }

    #[must_use]
    #[inline]
    pub const fn between_steps(load: Load, calibration: f32, limit: Option<u32>) -> Self {
        Self { load, calibration, limit, phase: Phase::BetweenSteps, elided: false }
    }

    /// The same request, after old Tool output has been dropped for it.
    #[must_use]
    #[inline]
    pub const fn elided(self) -> Self {
        Self { elided: true, ..self }
    }
}

/// The rules, and the one decision made from them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextBudget {
    rules: Budget,
}

impl ContextBudget {
    #[must_use]
    #[inline]
    pub const fn new(rules: Budget) -> Self {
        Self { rules }
    }

    /// Whether compaction is configured at all.
    #[must_use]
    #[inline]
    pub const fn compacts(&self) -> bool {
        self.rules.compaction.is_some()
    }

    /// What to do before a request that would carry `request`.
    #[must_use]
    pub fn decide(&self, request: &Request) -> Decision {
        let used = request.load.tokens(request.calibration);
        let Some(compaction) = self.rules.compaction else {
            return match request.phase {
                Phase::BeforeTurn => self.elision(request),
                Phase::BetweenSteps => Decision::Keep,
            };
        };
        let threshold = compaction.threshold(request.limit);
        match request.phase {
            // Between steps, old Tool output has already been read: dropping it only helps once
            // compaction is due, and compaction itself waits for the next Turn.
            Phase::BetweenSteps => {
                if let Some(rules) = self.rules.elision
                    && used > threshold
                {
                    return Decision::Shrink(rules);
                }
                Decision::Keep
            },
            // Before a Turn, elision is cheap enough to run at half the compaction threshold. It
            // goes first, so that dropping old output can avoid a summary altogether. Once it has
            // run, only a summary can help a request that is still over the threshold.
            Phase::BeforeTurn => {
                if let Some(rules) = self.rules.elision
                    && !request.elided
                    && used > threshold.div_euclid(2)
                {
                    return Decision::Shrink(rules);
                }
                if used > threshold { Decision::Compact(compaction) } else { Decision::Keep }
            },
        }
    }

    /// What to do when only elision is configured: run it once, then nothing is left to drop.
    const fn elision(&self, request: &Request) -> Decision {
        if request.elided {
            return Decision::Keep;
        }
        match self.rules.elision {
            Some(rules) => Decision::Shrink(rules),
            None => Decision::Keep,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compaction at `100_000` tokens, and elision as it is configured by default.
    fn budget() -> Budget {
        Budget {
            elision: Some(Elision { trigger: 25_000, target: 12_500, smallest: 250 }),
            compaction: Some(Compaction { budget: 100_000, limit_share: 1.0, keep_recent: 20_000 }),
        }
    }

    fn decide(budget: ContextBudget, load: Load, phase: Phase) -> Decision {
        let request = match phase {
            Phase::BeforeTurn => Request::before_turn(load, 1.0, None),
            Phase::BetweenSteps => Request::between_steps(load, 1.0, None),
        };
        budget.decide(&request)
    }

    #[test]
    fn a_request_under_the_budget_is_kept() {
        let budget = ContextBudget::new(budget());
        assert_eq!(decide(budget, Load::new(10_000, 1_000), Phase::BeforeTurn), Decision::Keep);
        assert_eq!(decide(budget, Load::new(10_000, 1_000), Phase::BetweenSteps), Decision::Keep);
    }

    #[test]
    fn before_a_turn_old_output_is_dropped_once_the_context_is_half_way_to_compaction() {
        let budget = ContextBudget::new(budget());
        assert_eq!(decide(budget, Load::new(49_999, 0), Phase::BeforeTurn), Decision::Keep, "under half way");
        assert!(
            matches!(decide(budget, Load::new(50_001, 0), Phase::BeforeTurn), Decision::Shrink(_)),
            "over half way"
        );
    }

    #[test]
    fn exactly_half_way_to_compaction_is_still_kept() {
        let budget = ContextBudget::new(budget());
        assert_eq!(decide(budget, Load::new(50_000, 0), Phase::BeforeTurn), Decision::Keep);
    }

    #[test]
    fn between_steps_old_output_waits_for_the_compaction_threshold() {
        let budget = ContextBudget::new(budget());
        assert_eq!(decide(budget, Load::new(50_001, 0), Phase::BetweenSteps), Decision::Keep);
        assert_eq!(decide(budget, Load::new(60_000, 0), Phase::BetweenSteps), Decision::Keep);
        assert!(matches!(decide(budget, Load::new(100_001, 0), Phase::BetweenSteps), Decision::Shrink(_)));
    }

    #[test]
    fn after_elision_a_request_over_the_budget_is_compacted() {
        let budget = ContextBudget::new(budget());
        let over = Request::before_turn(Load::new(100_001, 0), 1.0, None);
        assert!(matches!(budget.decide(&over), Decision::Shrink(_)));
        assert!(matches!(budget.decide(&over.elided()), Decision::Compact(_)));
    }

    #[test]
    fn elision_that_brings_a_request_under_the_threshold_needs_no_compaction() {
        let budget = ContextBudget::new(budget());
        let shrunk = Request::before_turn(Load::new(60_000, 0), 1.0, None).elided();
        assert_eq!(budget.decide(&shrunk), Decision::Keep);
    }

    #[test]
    fn without_elision_a_request_over_the_budget_is_compacted() {
        let budget = ContextBudget::new(Budget { elision: None, ..budget() });
        assert_eq!(decide(budget, Load::new(100_000, 0), Phase::BeforeTurn), Decision::Keep);
        assert!(matches!(decide(budget, Load::new(100_001, 0), Phase::BeforeTurn), Decision::Compact(_)));
    }

    #[test]
    fn without_compaction_elision_runs_at_once_and_then_stops() {
        let budget = ContextBudget::new(Budget { compaction: None, ..budget() });
        let load = Load::new(1, 0);
        assert!(matches!(decide(budget, load, Phase::BeforeTurn), Decision::Shrink(_)));
        assert_eq!(budget.decide(&Request::before_turn(load, 1.0, None).elided()), Decision::Keep);
    }

    #[test]
    fn with_nothing_turned_on_nothing_runs() {
        let budget = ContextBudget::new(Budget { compaction: None, elision: None });
        let heavy = Load::new(1_000_000, 0);
        assert_eq!(decide(budget, heavy, Phase::BeforeTurn), Decision::Keep);
        assert_eq!(decide(budget, heavy, Phase::BetweenSteps), Decision::Keep);
    }

    #[test]
    fn between_steps_needs_both_rules_to_do_anything() {
        let elision_only = ContextBudget::new(Budget { compaction: None, ..budget() });
        assert_eq!(decide(elision_only, Load::new(1_000_000, 0), Phase::BetweenSteps), Decision::Keep);
        let compaction_only = ContextBudget::new(Budget { elision: None, ..budget() });
        assert_eq!(decide(compaction_only, Load::new(1_000_000, 0), Phase::BetweenSteps), Decision::Keep);
    }

    #[test]
    fn tools_and_the_incoming_prompt_count_toward_the_budget() {
        let budget = ContextBudget::new(budget());
        let load = Load::new(0, 40_000).with_incoming(20_000);
        assert!(matches!(decide(budget, load, Phase::BeforeTurn), Decision::Shrink(_)), "60k is over half way");
    }

    #[test]
    fn the_estimate_is_corrected_by_what_the_provider_last_counted() {
        let budget = ContextBudget::new(budget());
        let load = Load::new(10_000, 0);
        assert_eq!(budget.decide(&Request::before_turn(load, 3.0, None)), Decision::Keep, "30k is under half way");
        let shrink = Request::before_turn(load, 6.0, None);
        assert!(matches!(budget.decide(&shrink), Decision::Shrink(_)), "60k is over half way");
    }

    #[test]
    fn a_model_without_a_context_limit_uses_the_configured_budget() {
        let rules =
            Budget { compaction: Some(Compaction { budget: 10_000, limit_share: 0.75, keep_recent: 0 }), ..budget() };
        let budget = ContextBudget::new(rules);
        assert!(matches!(decide(budget, Load::new(10_001, 0), Phase::BetweenSteps), Decision::Shrink(_)));
        let with_limit = Request::between_steps(Load::new(10_001, 0), 1.0, Some(1_000_000));
        assert!(matches!(budget.decide(&with_limit), Decision::Shrink(_)), "the lower budget still decides");
    }

    #[test]
    fn a_decision_carries_the_rule_to_apply() {
        let rules = budget();
        let budget = ContextBudget::new(rules);
        assert_eq!(
            decide(budget, Load::new(1_000_000, 0), Phase::BeforeTurn),
            Decision::Shrink(rules.elision.unwrap())
        );
        let over = Request::before_turn(Load::new(1_000_000, 0), 1.0, None).elided();
        assert_eq!(budget.decide(&over), Decision::Compact(rules.compaction.unwrap()));
    }

    #[test]
    fn one_estimate_is_shared_by_the_three_call_sites() {
        // `shrink` passes the prompt in; `run_loop` and `trim` do not. All three sum the
        // Conversation and the Tool definitions the same way, and calibration is the only
        // difference between learning from a request and deciding about the next one.
        let conversation = 1_000;
        let tools = 200;
        let estimate = |incoming| Load::new(conversation, tools).with_incoming(incoming);
        assert_eq!(estimate(0).raw(), 1_200, "run_loop and trim");
        assert_eq!(estimate(300).raw(), 1_500, "shrink");
        assert_eq!(estimate(0).tokens(1.0), estimate(0).raw(), "learning and deciding agree before any correction");
    }
}
