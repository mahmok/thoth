//! The `delegate` Tool: starts a Sub-agent playing a Persona and waits for its reply. Only a Root
//! agent has it, so a Sub-agent cannot delegate further.

use alloc::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits};
use crate::agent::sub_agents::SubAgents;
use crate::agent_builder::AgentBuilder;
use crate::persona::{Persona, Repository as PersonaRepository};

#[derive(Debug)]
/// The `delegate` Tool.
pub struct Delegate {
    builder: Arc<AgentBuilder>,
    /// The Root agent's own Persona, which plays the Sub-agent when none is named.
    own: Persona,
    personas: Arc<PersonaRepository>,
    schema: Value,
    subs: Arc<SubAgents>,
}

#[derive(Debug, Deserialize)]
/// Arguments of `delegate`.
pub struct Args {
    #[serde(default)]
    persona: Option<String>,
    task: String,
}

impl Delegate {
    #[inline]
    /// Creates the Tool, describing the Personas it can delegate to.
    pub fn new(
        personas: Arc<PersonaRepository>,
        builder: Arc<AgentBuilder>,
        subs: Arc<SubAgents>,
        own: Persona,
    ) -> Self {
        let infos = personas.infos();
        let keys: Vec<&str> = infos.iter().map(|info| info.key.as_str()).collect();
        let described: Vec<String> =
            infos.iter().map(|info| format!("`{}` ({})", info.key, info.description)).collect();
        let schema = json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "What the sub-agent should do. It does not see this conversation, so say what it needs.",
                },
                "persona": {
                    "type": "string",
                    "enum": keys,
                    "description": format!(
                        "Who plays the sub-agent: {}. Leave it out to play your own Persona.",
                        described.join("; ")
                    ),
                },
            },
            "required": ["task"],
        });
        Self { builder, own, personas, schema, subs }
    }

    /// The Persona a call names, or the Root agent's own when it names none. `None` when the
    /// name is not a Persona.
    fn persona_for(&self, key: Option<&str>) -> Option<Persona> {
        key.map_or_else(|| Some(self.own.clone()), |named| self.personas.get(named).cloned())
    }
}

impl Tool for Delegate {
    type Args = Args;

    #[inline]
    fn description(&self) -> &'static str {
        "Start a new sub-agent with an empty conversation, give it a task, and wait for its reply. \
         Every call starts a new sub-agent. To ask an existing sub-agent a follow-up, use the message \
         tool with its agent_id instead of delegating again. Sub-agents cannot delegate further."
    }

    #[inline]
    fn name(&self) -> &'static str {
        "delegate"
    }

    #[inline]
    fn parameters(&self) -> Value {
        self.schema.clone()
    }

    #[inline]
    async fn run(&self, args: Args, cx: &ToolContext) -> ToolResult {
        let Some(persona) = self.persona_for(args.persona.as_deref()) else {
            let name = args.persona.unwrap_or_default();
            return ToolResult::error(format!("there is no persona `{name}`"));
        };
        let id = match self.subs.spawn(&persona, || self.builder.build(&persona)) {
            Ok(id) => id,
            Err(refused) => return ToolResult::error(refused.to_string()),
        };
        self.subs.run(&id, args.task, cx).await
    }

    #[inline]
    fn summary(&self, args: &Args) -> String {
        args.persona.as_ref().map_or_else(|| args.task.clone(), |persona| format!("{persona}: {}", args.task))
    }

    #[inline]
    fn traits(&self) -> ToolTraits {
        ToolTraits::default()
    }

    /// Read-only when the Persona that would play the sub-agent only reads.
    #[inline]
    fn traits_for(&self, args: &Args) -> ToolTraits {
        match self.persona_for(args.persona.as_deref()) {
            Some(persona) if self.builder.is_read_only(&persona) => ToolTraits::read_only(),
            _ => ToolTraits::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delegation::Delegation;
    use crate::agent::tools::delegation_fixtures::{delegate_calls, reply, result_of, root, turn};
    use crate::test::assert_empty;

    #[tokio::test]
    async fn delegating_starts_a_sub_agent_that_plays_the_named_persona() {
        let (root, _) = root(
            vec![delegate_calls(&[("c1", r#"{"task":"look","persona":"tester"}"#)]), reply("found"), reply("done")],
            Delegation::default(),
        );
        let (results, _) = turn(&root).await;

        let result = result_of(&results, "c1");
        assert!(!result.is_error);
        assert_eq!(result.content, "agent_id: tester-1\n\nfound");
        let infos = root.sub_agents().infos();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].persona, "tester");
    }

    #[tokio::test]
    async fn without_a_persona_the_sub_agent_plays_the_roots_own() {
        let (root, _) = root(
            vec![delegate_calls(&[("c1", r#"{"task":"look"}"#)]), reply("found"), reply("done")],
            Delegation::default(),
        );
        turn(&root).await;
        assert_eq!(root.sub_agents().infos()[0].persona, "root");
    }

    #[tokio::test]
    async fn an_unknown_persona_is_an_error_and_starts_nothing() {
        let (root, _) = root(
            vec![delegate_calls(&[("c1", r#"{"task":"look","persona":"ghost"}"#)]), reply("done")],
            Delegation::default(),
        );
        let (results, _) = turn(&root).await;
        let result = result_of(&results, "c1");
        assert!(result.is_error && result.content.contains("ghost"));
        assert_empty(root.sub_agents().infos());
    }

    #[tokio::test]
    async fn a_session_at_its_limit_refuses_with_an_error_result() {
        let (root, _) = root(
            vec![
                delegate_calls(&[("c1", r#"{"task":"one","persona":"tester"}"#)]),
                reply("a"),
                delegate_calls(&[("c2", r#"{"task":"two","persona":"tester"}"#)]),
                reply("done"),
            ],
            Delegation { max_per_session: 1, ..Delegation::default() },
        );
        let (results, _) = turn(&root).await;
        assert!(!result_of(&results, "c1").is_error);
        let refused = result_of(&results, "c2");
        assert!(refused.is_error && refused.content.contains("already has 1 sub-agents"));
    }

    #[tokio::test]
    async fn the_sub_agent_cannot_delegate_so_it_does_not_have_the_tool() {
        let (root, _) = root(
            vec![delegate_calls(&[("c1", r#"{"task":"look","persona":"tester"}"#)]), reply("found"), reply("done")],
            Delegation::default(),
        );
        turn(&root).await;
        let sub = root.sub_agents().get("tester-1").unwrap();
        assert!(!sub.agent.tools().definitions().iter().any(|spec| spec.name == "delegate"));
    }

    #[test]
    fn the_description_says_to_follow_up_with_message_instead_of_delegating_again() {
        let (root, _) = root(vec![], Delegation::default());
        let delegate = root.tools().definitions().iter().find(|spec| spec.name == "delegate").unwrap().clone();
        assert!(delegate.description.contains("message"), "{}", delegate.description);
        assert!(delegate.description.contains("instead of delegating again"));
    }

    #[test]
    fn the_schema_lists_every_persona_key_and_what_it_is_for() {
        let (root, _) = root(vec![], Delegation::default());
        let spec = root.tools().definitions().iter().find(|spec| spec.name == "delegate").unwrap().clone();
        assert_eq!(spec.parameters["properties"]["persona"]["enum"], json!(["reader", "root", "tester"]));
        let described = spec.parameters["properties"]["persona"]["description"].as_str().unwrap();
        assert!(described.contains("`tester` (writes tests)"), "{described}");
        assert_eq!(spec.parameters["required"], json!(["task"]));
    }

    #[test]
    fn a_call_is_read_only_only_when_the_persona_it_names_only_reads() {
        let (root, _) = root(vec![], Delegation::default());
        let traits = |args: &str| root.tools().traits("delegate", args).read_only;
        assert!(traits(r#"{"task":"x","persona":"reader"}"#));
        assert!(!traits(r#"{"task":"x","persona":"tester"}"#));
        assert!(!traits(r#"{"task":"x"}"#), "the root's own Persona has every tool");
        assert!(!traits(r#"{"task":"x","persona":"ghost"}"#));
    }

    #[tokio::test]
    async fn read_only_delegations_in_one_step_run_together() {
        let (root, _) = root(
            vec![
                delegate_calls(&[
                    ("a", r#"{"task":"1","persona":"reader"}"#),
                    ("b", r#"{"task":"2","persona":"reader"}"#),
                ]),
                reply("first"),
                reply("second"),
                reply("done"),
            ],
            Delegation::default(),
        );
        let (results, order) = turn(&root).await;
        assert_eq!(order, ["start a", "start b", "finish a", "finish b"]);
        let mut ids: Vec<String> =
            results.iter().map(|entry| entry.1.content.lines().next().unwrap().to_owned()).collect();
        ids.sort();
        assert_eq!(ids, ["agent_id: reader-1", "agent_id: reader-2"]);
    }

    #[tokio::test]
    async fn delegations_that_may_edit_run_one_after_another() {
        let (root, _) = root(
            vec![
                delegate_calls(&[
                    ("a", r#"{"task":"1","persona":"tester"}"#),
                    ("b", r#"{"task":"2","persona":"tester"}"#),
                ]),
                reply("first"),
                reply("second"),
                reply("done"),
            ],
            Delegation::default(),
        );
        let (_, order) = turn(&root).await;
        assert_eq!(order, ["start a", "finish a", "start b", "finish b"]);
    }
}
