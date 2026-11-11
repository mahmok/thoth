use alloc::sync::Arc;

use crate::agent::context::Context as AgentContext;
use crate::agent::delegation::Delegation;
use crate::agent::skills::SkillSet;
use crate::agent::tools::tool::Outcome as ToolResult;
use crate::agent::{Agent, Event as AgentEvent};
use crate::agent_builder::AgentBuilder;
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::client::StreamedCompletion;
use crate::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind};
use crate::llm::client::error::Failure as ClientError;
use crate::llm::client::fake::Client as FakeClient;
use crate::llm::provider::{ModelSpec, Protocol, Provider};
use crate::persona::{Persona, Repository as PersonaRepository, Selection};
use crate::test::temp_tree::TempTree;

pub type Script = Vec<Result<StreamedCompletion, ClientError>>;

fn spec() -> ModelSpec {
    ModelSpec::new(Provider::Ollama, Protocol::OpenAi, "m")
}

/// The Root's own Persona, a Persona with every tool, and one that only reads.
pub fn personas() -> Arc<PersonaRepository> {
    let mut registry = PersonaRepository::new();
    for persona in [
        Persona::new("root", "Root", "the root persona"),
        Persona::new("tester", "Tester", "writes tests"),
        Persona::new("reader", "Reader", "only reads").with_tools(Selection::Only(vec!["read_file".into()])),
    ] {
        registry.add(persona.with_model(spec())).unwrap();
    }
    Arc::new(registry)
}

/// A Root agent. Its model and every Sub-agent's model answer from `script`, in the order
/// the calls are made.
pub fn root(script: Script, limits: Delegation) -> (Agent, FakeClient) {
    let tree = TempTree::new("delegate");
    let fake = FakeClient::new(script);
    let builder = Arc::new(
        AgentBuilder::new(
            Arc::new(CredentialStore::in_memory()),
            tree.workspace(),
            AgentContext::new(tree.path()),
            SkillSet::default(),
        )
        .with_delegation(limits)
        .with_fake_client(fake.clone()),
    );
    let personas = personas();
    let own = personas.get("root").unwrap().clone();
    (builder.build_root(&own, personas), fake)
}

#[expect(clippy::unnecessary_wraps, reason = "the fake client's script holds Results, and the caller compares Options")]
pub fn reply(text: &str) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion { text: text.into(), ..Default::default() })
}

/// A reply that calls `delegate` once for each `(call id, arguments)`.
/// A reply that calls the Tool `name` once for each `(call id, arguments)`.
#[expect(clippy::unnecessary_wraps, reason = "the fake client's script holds Results, and the caller compares Options")]
pub fn tool_calls(name: &str, calls: &[(&str, &str)]) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion {
        tool_calls: calls
            .iter()
            .map(|&(id, arguments)| ToolCall {
                id: id.into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: name.into(), arguments: arguments.into() },
            })
            .collect(),
        ..Default::default()
    })
}

/// A reply that calls `delegate` once for each `(call id, arguments)`.
pub fn delegate_calls(calls: &[(&str, &str)]) -> Result<StreamedCompletion, ClientError> {
    tool_calls("delegate", calls)
}

/// Runs one Turn, returning the tool results by call id and the tool events in order.
pub async fn turn(agent: &Agent) -> (Vec<(String, ToolResult)>, Vec<String>) {
    let mut results = Vec::new();
    let mut order = Vec::new();
    agent
        .respond("go".into(), |event| match event {
            AgentEvent::ToolStarted { id, .. } => order.push(format!("start {id}")),
            AgentEvent::ToolFinished { id, result, .. } => {
                order.push(format!("finish {id}"));
                results.push((id, result));
            },
            AgentEvent::Text(_)
            | AgentEvent::Reasoning(_)
            | AgentEvent::StepStarted { .. }
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::Notice(_) => {},
        })
        .await
        .unwrap();
    (results, order)
}

pub fn result_of<'results>(results: &'results [(String, ToolResult)], id: &str) -> &'results ToolResult {
    &results.iter().find(|entry| entry.0 == id).unwrap().1
}
