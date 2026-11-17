use alloc::sync::Arc;

use crate::adapters::llm::client::StreamedCompletion;
use crate::adapters::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind};
use crate::adapters::llm::client::error::Failure as ClientError;
use crate::adapters::llm::client::fake::Client as FakeClient;
use crate::auth::store::Credentials as CredentialStore;
use crate::domain::provider::Model;
use crate::domain::shared::SessionId;
use crate::domain::transcript::Observed;
use crate::models::store::InMemory as InMemoryModelsStore;
use crate::persona::{Persona, Repository as PersonaRepository, Selection};
use crate::runtime::agent::Agent;
use crate::runtime::agent::delegation::Delegation;
use crate::runtime::agent_builder::AgentBuilder;
use crate::runtime::context::AgentContext;
use crate::runtime::skills::SkillSet;
use crate::runtime::tools::tool::Outcome as ToolResult;
use crate::test::temp_tree::TempTree;

pub type Script = Vec<Result<StreamedCompletion, ClientError>>;

/// The Root's own Persona, a Persona with every tool, and one that only reads.
///
/// # Panics
///
/// Panics if the fixture registry rejects a persona.
#[must_use]
pub fn personas() -> Arc<PersonaRepository> {
    let mut registry = PersonaRepository::new();
    for persona in [
        Persona::new("root", "Root", "the root persona"),
        Persona::new("tester", "Tester", "writes tests"),
        Persona::new("reader", "Reader", "only reads").with_tools(Selection::Only(vec!["read_file".into()])),
    ] {
        registry.add(persona.with_model(Model::reference("ollama", "m"))).unwrap();
    }
    Arc::new(registry)
}

/// A Root agent. Its model and every Sub-agent's model answer from `script`, in the order
/// the calls are made.
///
/// # Panics
///
/// Panics if the `root` fixture persona is missing.
#[must_use]
pub fn root(script: Script, limits: Delegation) -> (Agent, FakeClient) {
    let tree = TempTree::new("delegate");
    let fake = FakeClient::new(script);
    let builder = Arc::new(
        AgentBuilder::new(
            Arc::new(CredentialStore::in_memory()),
            Arc::new(InMemoryModelsStore::default()),
            tree.workspace(),
            AgentContext::new(tree.path()),
            SkillSet::default(),
        )
        .with_delegation(limits)
        .with_fake_client(fake.clone()),
    );
    let personas = personas();
    let own = personas.get("root").unwrap().clone();
    (builder.build_root(SessionId::mint(), &own, personas), fake)
}

/// # Errors
///
/// Never fails; the `Result` is the shape the fake client's script stores.
pub fn reply(text: &str) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion { text: text.into(), ..Default::default() })
}

/// A reply that calls the Tool `name` once for each `(call id, arguments)`.
///
/// # Errors
///
/// Never fails; the `Result` is the shape the fake client's script stores.
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
///
/// # Errors
///
/// Never fails; the `Result` is the shape the fake client's script stores.
pub fn delegate_calls(calls: &[(&str, &str)]) -> Result<StreamedCompletion, ClientError> {
    tool_calls("delegate", calls)
}

/// Runs one Turn, returning the tool results by call id and the tool events in order.
///
/// # Panics
///
/// Panics if the Turn fails.
pub async fn turn(agent: &Agent) -> (Vec<(String, ToolResult)>, Vec<String>) {
    let mut results = Vec::new();
    let mut order = Vec::new();
    agent
        .respond("go".into(), |event| match Observed::of(event) {
            Observed::ToolStarted { id, .. } => order.push(format!("start {id}")),
            Observed::ToolFinished { id, result, .. } => {
                order.push(format!("finish {id}"));
                results.push((id, result));
            },
            Observed::Text(_)
            | Observed::Reasoning(_)
            | Observed::Step(_)
            | Observed::ToolProgress { .. }
            | Observed::Notice(_) => {},
        })
        .await
        .unwrap();
    (results, order)
}

/// The result of the call with id `id`.
///
/// # Panics
///
/// Panics if no result has that call id.
#[must_use]
pub fn result_of<'results>(results: &'results [(String, ToolResult)], id: &str) -> &'results ToolResult {
    &results.iter().find(|entry| entry.0 == id).unwrap().1
}
