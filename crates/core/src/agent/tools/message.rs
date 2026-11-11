//! The `message` Tool: sends a follow-up to a Sub-agent a Root agent started, and waits for its
//! reply. The Sub-agent keeps its own conversation, so it carries on from where it stopped.

use alloc::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits};
use crate::agent::sub_agents::SubAgents;
use crate::agent_builder::AgentBuilder;

#[derive(Debug)]
/// The `message` Tool.
pub struct Messenger {
    builder: Arc<AgentBuilder>,
    subs: Arc<SubAgents>,
}

#[derive(Debug, Deserialize)]
/// Arguments of `message`.
pub struct Args {
    agent_id: String,
    text: String,
}

impl Messenger {
    #[inline]
    pub const fn new(builder: Arc<AgentBuilder>, subs: Arc<SubAgents>) -> Self {
        Self { builder, subs }
    }
}

impl Tool for Messenger {
    type Args = Args;

    #[inline]
    fn description(&self) -> &'static str {
        "Send a follow-up to a sub-agent you already started, and wait for its reply. Prefer this \
         to delegate for follow-ups: the sub-agent remembers what it was asked before, and it does \
         not start a new one."
    }

    #[inline]
    fn name(&self) -> &'static str {
        "message"
    }

    #[inline]
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The agent_id that delegate returned, such as tester-1.",
                },
                "text": {
                    "type": "string",
                    "description": "What to say to that sub-agent.",
                },
            },
            "required": ["agent_id", "text"],
        })
    }

    #[inline]
    async fn run(&self, args: Args, cx: &ToolContext) -> ToolResult {
        self.subs.run(&args.agent_id, args.text, cx).await
    }

    #[inline]
    fn summary(&self, args: &Args) -> String {
        format!("{}: {}", args.agent_id, args.text)
    }

    #[inline]
    fn traits(&self) -> ToolTraits {
        ToolTraits::default()
    }

    /// Read-only when the Persona of the sub-agent it names only reads. An unknown id is not, so
    /// it is refused in the ordinary way.
    #[inline]
    fn traits_for(&self, args: &Args) -> ToolTraits {
        match self.subs.get(&args.agent_id) {
            Some(sub) if self.builder.is_read_only(&sub.persona) => ToolTraits::read_only(),
            _ => ToolTraits::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::Agent;
    use crate::agent::delegation::Delegation;
    use crate::agent::tools::delegation_fixtures::{delegate_calls, reply, result_of, root, tool_calls, turn};

    fn names(agent: &Agent) -> Vec<String> {
        agent.tools().definitions().iter().map(|spec| spec.name.clone()).collect()
    }

    #[tokio::test]
    async fn a_message_continues_the_sub_agents_conversation() {
        let (root, fake) = root(
            vec![
                delegate_calls(&[("c1", r#"{"task":"look","persona":"tester"}"#)]),
                reply("question?"),
                tool_calls("message", &[("m1", r#"{"agent_id":"tester-1","text":"more"}"#)]),
                reply("answer"),
                reply("done"),
            ],
            Delegation::default(),
        );
        let (results, _) = turn(&root).await;

        let answer = result_of(&results, "m1");
        assert!(!answer.is_error);
        assert_eq!(answer.content, "agent_id: tester-1\n\nanswer");
        // The sub-agent's second model call sees its first Turn (its task and its question) and
        // the new message, after its system prompt.
        assert_eq!(fake.requests()[3].0, 4);
        assert!(names(&root).contains(&"message".to_owned()));
        let sub = root.sub_agents().get("tester-1").unwrap();
        assert!(!names(&sub.agent).contains(&"message".to_owned()));
    }

    #[tokio::test]
    async fn a_message_to_an_unknown_sub_agent_is_an_error_result() {
        let (root, _) = root(
            vec![tool_calls("message", &[("m1", r#"{"agent_id":"ghost-1","text":"hi"}"#)]), reply("done")],
            Delegation::default(),
        );
        let (results, _) = turn(&root).await;
        let result = result_of(&results, "m1");
        assert!(result.is_error && result.content.contains("ghost-1"));
    }

    #[tokio::test]
    async fn a_message_is_read_only_only_to_a_sub_agent_whose_persona_only_reads() {
        let (root, _) = root(
            vec![
                delegate_calls(&[
                    ("a", r#"{"task":"1","persona":"reader"}"#),
                    ("b", r#"{"task":"2","persona":"tester"}"#),
                ]),
                reply("read"),
                reply("tested"),
                reply("done"),
            ],
            Delegation::default(),
        );
        turn(&root).await;
        let traits =
            |id: &str| root.tools().traits("message", &format!(r#"{{"agent_id":"{id}","text":"x"}}"#)).read_only;
        assert!(traits("reader-1"));
        assert!(!traits("tester-1"));
        assert!(!traits("ghost-1"), "an unknown id is not read-only");
    }

    #[tokio::test]
    async fn read_only_messages_in_one_step_run_together() {
        let (root, _) = root(
            vec![
                delegate_calls(&[
                    ("a", r#"{"task":"1","persona":"reader"}"#),
                    ("b", r#"{"task":"2","persona":"reader"}"#),
                ]),
                reply("first"),
                reply("second"),
                tool_calls(
                    "message",
                    &[
                        ("m1", r#"{"agent_id":"reader-1","text":"more"}"#),
                        ("m2", r#"{"agent_id":"reader-2","text":"more"}"#),
                    ],
                ),
                reply("again"),
                reply("again"),
                reply("done"),
            ],
            Delegation::default(),
        );
        let (results, order) = turn(&root).await;
        assert_eq!(order[4..], ["start m1", "start m2", "finish m1", "finish m2"]);
        assert_eq!(result_of(&results, "m1").content.lines().next(), Some("agent_id: reader-1"));
        assert_eq!(result_of(&results, "m2").content.lines().next(), Some("agent_id: reader-2"));
    }

    #[tokio::test]
    async fn messages_that_may_edit_run_one_after_another() {
        let (root, _) = root(
            vec![
                delegate_calls(&[("c1", r#"{"task":"look","persona":"tester"}"#)]),
                reply("question?"),
                tool_calls(
                    "message",
                    &[
                        ("m1", r#"{"agent_id":"tester-1","text":"one"}"#),
                        ("m2", r#"{"agent_id":"tester-1","text":"two"}"#),
                    ],
                ),
                reply("first"),
                reply("second"),
                reply("done"),
            ],
            Delegation::default(),
        );
        let (_, order) = turn(&root).await;
        assert_eq!(order[2..], ["start m1", "finish m1", "start m2", "finish m2"]);
    }
}
