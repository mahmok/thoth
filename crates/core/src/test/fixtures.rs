use alloc::sync::Arc;
use core::future;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::context::Context as AgentContext;
use crate::agent::delegation::Delegation;
use crate::agent::skills::SkillSet;
use crate::agent::tools::tool::{
    Context as ToolContext, Description as ToolDescription, Outcome as ToolResult, Tool, Traits as ToolTraits,
};
use crate::agent::tools::tool_set::ToolSet;
use crate::agent::{Agent, Event as AgentEvent};
use crate::agent_builder::AgentBuilder;
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::client::any::Client as AnyClient;
use crate::llm::provider::{Model, ModelSpec, Protocol, Provider};
use crate::models::store::{InMemory as InMemoryModelsStore, Models as ModelsStore};
use crate::persona::{Persona, Repository as PersonaRepository, Selection};
use crate::test::client::{Client, Script};
use crate::test::temp_tree::TempTree;

/// The models of one provider, as id and an optional protocol override.
pub type Listing<'listing> = &'listing [(&'listing str, Option<Protocol>)];

#[must_use]
#[inline]
/// A built-in provider.
///
/// # Panics
///
/// Panics if `id` is not a built-in provider.
pub fn provider(id: &str) -> Arc<Provider> {
    Arc::new(Provider::builtin(id).expect("the provider is built in"))
}

#[must_use]
#[inline]
/// A model of a built-in provider.
///
/// # Panics
///
/// Panics if `provider_id` is not a built-in provider.
pub fn model(provider_id: &str, id: &str) -> ModelSpec {
    ModelSpec::new(provider(provider_id), Model::new(id))
}

#[must_use]
#[inline]
/// The model every fixture Agent uses unless it says otherwise.
pub fn test_model() -> ModelSpec {
    model("ollama", "test-model")
}

#[must_use]
#[inline]
/// A stored model, as the provider lists it.
pub fn stored(id: &str, protocol: Option<Protocol>) -> Model {
    Model::new(id).with_protocol(protocol)
}

#[must_use]
#[inline]
/// A store holding the given models. Providers left out keep the built-in defaults.
///
/// # Panics
///
/// Panics if a store write fails or an id is not a built-in provider.
pub fn store_with(entries: &[(&str, Listing<'_>)]) -> Arc<InMemoryModelsStore> {
    let store = Arc::new(InMemoryModelsStore::default());
    for &(id, models) in entries {
        let mut provider = store.provider(id).expect("the provider is built in").as_ref().clone();
        provider.models = models.iter().map(|&(model, protocol)| stored(model, protocol)).collect();
        provider.checked_at = 1;
        store.save(&provider).unwrap();
    }
    store
}

/// A few models per provider, enough for the menus.
#[must_use]
#[inline]
pub fn models() -> Arc<InMemoryModelsStore> {
    use Protocol::{Anthropic, OpenAi};
    store_with(&[
        ("ollama", &[("bonsai-2:2b", Some(OpenAi)), ("llama3.2:3b", None), ("mistral:latest", None)]),
        ("openai", &[("gpt-4.1", None), ("gpt-4o", None)]),
        ("anthropic", &[("claude-sonnet-4-5", Some(Anthropic))]),
        ("github-copilot", &[("claude-sonnet-4.5", Some(Anthropic)), ("gpt-4.1", Some(OpenAi))]),
    ])
}

#[must_use]
#[inline]
/// A scripted client that follows `script`.
pub fn client(script: Script) -> Client {
    Client::new(script)
}

fn agent_with_model(script: Script, model: ModelSpec, tools: ToolSet) -> (Agent, Client) {
    let client = client(script);
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
        .with_model(model)
        .with_client(AnyClient::Scripted(client.clone()))
        .with_tools(tools);
    (agent, client)
}

/// An Agent whose model replies from `script`, offering `tools`.
#[must_use]
pub fn agent_with(script: Script, tools: ToolSet) -> (Agent, Client) {
    agent_with_model(script, test_model(), tools)
}

/// An Agent whose model replies from `script`, with no Tools.
#[must_use]
pub fn agent(script: Script) -> (Agent, Client) {
    agent_with(script, ToolSet::new())
}

/// A Sub-agent's Agent, whose model replies from `script`.
#[must_use]
pub fn sub_agent(script: Script) -> (Agent, Client) {
    agent_with_model(script, model("ollama", "sub"), ToolSet::new())
}

#[derive(Debug)]
/// A Tool that echoes its `text` argument back with an `echo: ` prefix.
pub struct Echo;

#[derive(Debug, Deserialize)]
/// Arguments of [`Echo`].
pub struct EchoArgs {
    pub text: String,
}

impl Tool for Echo {
    type Args = EchoArgs;

    fn describe(&self) -> ToolDescription {
        ToolDescription { description: "Echo the text back", name: "echo" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] })
    }

    fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl future::Future<Output = ToolResult> + Send {
        future::ready(ToolResult::ok(format!("echo: {}", args.text)))
    }

    fn summary(&self, args: &EchoArgs) -> String {
        args.text.clone()
    }

    #[inline]
    fn traits(&self) -> ToolTraits {
        ToolTraits::default()
    }
}

/// An Agent offering [`Echo`], whose model replies from `script`.
#[must_use]
pub fn echo_agent(script: Script) -> (Agent, Client) {
    let mut tools = ToolSet::new();
    tools.register(Echo).unwrap();
    agent_with(script, tools)
}

/// A Root Agent that owns the temporary workspace its Tools write to.
///
/// The workspace lives as long as this value does, so an Agent returned by a fixture never points
/// at a directory that has already been deleted.
#[derive(Debug)]
pub struct Root {
    agent: Agent,
    tree: TempTree,
}

impl core::ops::Deref for Root {
    type Target = Agent;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.agent
    }
}

impl Root {
    #[must_use]
    #[inline]
    /// The workspace the Root Agent's Tools work in.
    pub fn path(&self) -> &Path {
        self.tree.path()
    }
}

/// The Root's own Persona, a Persona with every tool, and one that only reads.
///
/// # Panics
///
/// Panics if the built-in Personas cannot be added.
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

/// A Root Agent. Its model and every Sub-agent's model answer from `script`, in the order
/// the calls are made. The returned value owns the workspace for as long as it lives.
///
/// # Panics
///
/// Panics if the Persona set cannot be built.
#[must_use]
pub fn root(script: Script, limits: Delegation) -> (Root, Client) {
    let tree = TempTree::new("delegate");
    let client = Client::new(script);
    let builder = Arc::new(
        AgentBuilder::new(
            Arc::new(CredentialStore::in_memory()),
            Arc::new(InMemoryModelsStore::default()),
            tree.workspace(),
            AgentContext::new(tree.path()),
            SkillSet::default(),
        )
        .with_delegation(limits)
        .with_scripted_client(client.clone()),
    );
    let personas = personas();
    let own = personas.get("root").unwrap().clone();
    (Root { agent: builder.build_root(&own, personas), tree }, client)
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

#[must_use]
#[inline]
/// The result of the Tool call with `id`.
///
/// # Panics
///
/// Panics if there is no result for `id`.
pub fn result_of<'results>(results: &'results [(String, ToolResult)], id: &str) -> &'results ToolResult {
    &results.iter().find(|entry| entry.0 == id).unwrap().1
}
