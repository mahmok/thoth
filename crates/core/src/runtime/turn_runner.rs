/// A test double for [`TurnRunner`]: records what it is asked and replies on demand.
#[cfg(any(test, feature = "testing"))]
pub mod scripted;

use alloc::sync::Arc;

use crate::application::ports::{Reply, TurnRunner};
use crate::runtime::background::Background;

use crate::adapters::skills::SkillSummary;
use crate::runtime::agent::{Agent, AgentChange, Settings};

/// The [`TurnRunner`] that runs turns on the tokio runtime against a real [`Agent`].
#[derive(Debug)]
pub struct TokioTurnRunner {
    agent: Arc<Agent>,
    turn: Background<Reply>,
}

impl TokioTurnRunner {
    #[inline]
    pub fn new(agent: Arc<Agent>) -> Self {
        Self { agent, turn: Background::default() }
    }

    /// Starts a task that runs `work` and sends its events back.
    fn run(&self, work: Work) {
        let agent = Arc::clone(&self.agent);
        self.turn.start(|replies| async move {
            let streamed = replies.clone();
            let on_event = move |event| {
                let _unused = streamed.send(Reply::Event(event));
            };
            let result = match work {
                Work::Turn(prompt) => agent.respond(prompt, on_event).await.map(|done| done.usage),
                Work::Compact(focus) => agent.compact(focus, on_event).await,
            };
            let _unused = replies.send(match result {
                Ok(usage) => Reply::Done { usage },
                Err(e) => Reply::Failed(e.to_string()),
            });
        });
    }
}

/// A job for the Agent task.
enum Work {
    Compact(Option<String>),
    Turn(String),
}

impl TurnRunner for TokioTurnRunner {
    #[inline]
    fn apply(&self, change: AgentChange) {
        self.agent.apply(change);
    }

    #[inline]
    fn compact(&self, focus: Option<String>) {
        self.run(Work::Compact(focus));
    }

    #[inline]
    fn settings(&self) -> Settings {
        self.agent.settings()
    }

    #[inline]
    fn skill_block(&self, name: &str) -> Option<String> {
        self.agent.skills().get_skill_by_name(name).map(crate::adapters::skills::Skill::block)
    }

    #[inline]
    fn skills(&self) -> Vec<SkillSummary> {
        self.agent.skills().summaries()
    }

    #[inline]
    fn start(&self, prompt: String) {
        self.run(Work::Turn(prompt));
    }

    #[inline]
    fn try_next(&self) -> Option<Reply> {
        self.turn.try_next()
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use crate::adapters::llm::client::StreamedCompletion;
    use crate::adapters::llm::client::error::Failure as ClientError;
    use crate::adapters::llm::client::fake;
    use crate::adapters::llm::client::fake::Client as FakeClient;
    use crate::domain::provider::{Model, ModelSpec, Provider};
    use crate::domain::thinking::Thinking;
    use crate::runtime::agent::{Agent, AgentEvent};
    use tokio::time;

    fn runner(script: Vec<Result<StreamedCompletion, ClientError>>) -> TokioTurnRunner {
        let agent = Agent::new(Arc::new(fake::Factory::new(FakeClient::new(script))))
            .with_model(ModelSpec::new(Arc::new(Provider::builtin("ollama").unwrap()), Model::new("m")));
        TokioTurnRunner::new(Arc::new(agent))
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    fn reply(text: &str) -> Result<StreamedCompletion, ClientError> {
        Ok(StreamedCompletion { text: text.into(), usage: None, tool_calls: Vec::new(), ..Default::default() })
    }

    /// Waits for the turn to end, returning every reply it produced.
    async fn run_turn(fail: bool) -> Vec<Reply> {
        let link = runner(vec![if fail { Err(fake::failure()) } else { reply("hello") }]);
        link.start("hi".into());

        let mut replies = Vec::new();
        time::timeout(Duration::from_secs(5), async {
            loop {
                match link.try_next() {
                    Some(reply) => {
                        let finished = matches!(reply, Reply::Done { .. } | Reply::Failed(_));
                        replies.push(reply);
                        if finished {
                            break;
                        }
                    },
                    None => time::sleep(Duration::from_millis(1)).await,
                }
            }
        })
        .await
        .expect("the turn should finish");
        replies
    }

    #[tokio::test]
    async fn streams_the_agents_events_then_reports_done() {
        let replies = run_turn(false).await;
        let (last, events) = replies.split_last().unwrap();
        assert!(matches!(last, Reply::Done { .. }));
        assert!(
            events.iter().any(|r| matches!(r, Reply::Event(AgentEvent::Text(t)) if t == "hello")),
            "the reply text streams before done"
        );
    }

    #[tokio::test]
    async fn a_failed_turn_reports_the_error() {
        let replies = run_turn(true).await;
        assert_eq!(replies.len(), 2, "the step that began, then the error");
        assert!(matches!(
            replies.last(),
            Some(Reply::Failed(message)) if message == "api error: fake failure"
        ));
    }

    #[test]
    fn settings_are_the_agents_and_changes_reach_it() {
        let link = runner(Vec::new());
        assert_eq!(link.settings().model.map(|m| m.model.id), Some("m".to_owned()));
        link.apply(AgentChange::SetThinking(Thinking::High));
        assert_eq!(link.settings().thinking, Thinking::High);
    }
}
