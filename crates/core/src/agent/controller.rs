/// A test double for [`AgentController`]: records what it is asked and replies on demand.
#[cfg(any(test, feature = "testing"))]
pub mod scripted;

use alloc::sync::Arc;
use core::fmt::Debug;

use crate::background::Background;

use crate::agent::skills::SkillSummary;
use crate::agent::{Agent, Change as AgentChange, Event as AgentEvent, Settings};
use crate::llm::client::chat::Usage;

#[derive(Debug)]
/// What the Agent task sends back.
pub enum Reply {
    Done { usage: Option<Usage> },
    Event(AgentEvent),
    Failed(String),
}

/// Runs an Agent and hands back what it reports.
pub trait Controller: Debug {
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

/// The [`AgentController`] that runs turns on the tokio runtime against a real [`Agent`].
#[derive(Debug)]
pub struct TokioAgent {
    agent: Arc<Agent>,
    turn: Background<Reply>,
}

impl TokioAgent {
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

impl Controller for TokioAgent {
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
        self.agent.skills().get_skill_by_name(name).map(super::skills::Skill::block)
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
    use crate::agent::{Agent, Event as AgentEvent};
    use crate::auth::store::Credentials as CredentialStore;
    use crate::llm::client::StreamedCompletion;
    use crate::llm::client::any::Client as AnyClient;
    use crate::llm::client::error::Failure as ClientError;
    use crate::llm::client::fake;
    use crate::llm::client::fake::Client as FakeClient;
    use crate::llm::provider::{Model, ModelSpec, Provider};
    use crate::llm::thinking::Thinking;
    use tokio::time;

    fn controller(script: Vec<Result<StreamedCompletion, ClientError>>) -> TokioAgent {
        let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
            .with_model(ModelSpec::new(Arc::new(Provider::ollama()), Model::new("m")))
            .with_client(AnyClient::Fake(FakeClient::new(script)));
        TokioAgent::new(Arc::new(agent))
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
        let link = controller(vec![if fail { Err(fake::failure()) } else { reply("hello") }]);
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
        let link = controller(Vec::new());
        assert_eq!(link.settings().model.map(|m| m.model.id), Some("m".to_owned()));
        link.apply(AgentChange::SetThinking(Thinking::High));
        assert_eq!(link.settings().thinking, Thinking::High);
    }
}
