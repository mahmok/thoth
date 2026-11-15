//! Fixtures shared by the Agent's tests, and the tests for the Agent's own state.

#![expect(clippy::result_large_err, reason = "the fake client's script holds Results, as the product's client returns")]

use alloc::sync::Arc;
use core::future;

use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::agent::tools::tool::{
    Context as ToolContext, Description, Outcome as ToolResult, Tool, Traits as ToolTraits,
};
use crate::auth::testing::MockServer;
use crate::llm::client::StreamedCompletion;
use crate::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind};
use crate::llm::client::error::Failure as ClientError;
use crate::llm::client::fake::Client as FakeClient;
use crate::llm::provider::{LoginMethod, Model, ModelSpec, Protocol, Provider};
use crate::persona::Persona;
use crate::test::assert_empty;

/// A Tool that echoes its argument. Every test Agent has it.
#[derive(Debug)]
pub struct Echo;

#[derive(Debug, Deserialize)]
pub struct EchoArgs {
    pub text: String,
}

impl Tool for Echo {
    type Args = EchoArgs;

    fn describe(&self) -> Description {
        Description { description: "Echo the text back", name: "echo" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }

    fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
        future::ready(ToolResult::ok(format!("echo: {}", args.text)))
    }

    fn summary(&self, args: &EchoArgs) -> String {
        args.text.clone()
    }

    #[inline]
    fn traits(&self) -> ToolTraits {
        ToolTraits::default()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

pub fn test_model() -> ModelSpec {
    ModelSpec::new(Arc::new(Provider::builtin("ollama").unwrap()), Model::new("test-model"))
}

/// An Agent with one Tool, `Echo`, and a client that answers from `script`.
pub fn build(script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
    let mut tools = ToolSet::new();
    tools.register(Echo).unwrap();
    let fake = FakeClient::new(script);
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
        .with_model(test_model())
        .with_client(AnyClient::Fake(fake.clone()))
        .with_tools(tools);
    (agent, fake)
}

#[expect(clippy::unnecessary_wraps, reason = "the fake client's script holds Results, and the caller compares Options")]
pub fn text(text: &str) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion { text: text.into(), usage: None, tool_calls: Vec::new(), ..Default::default() })
}

#[expect(clippy::unnecessary_wraps, reason = "the fake client's script holds Results, and the caller compares Options")]
pub fn call(id: &str, name: &str, arguments: &str) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion {
        text: String::new(),
        usage: None,
        tool_calls: vec![ToolCall {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall { name: name.into(), arguments: arguments.into() },
        }],
        ..Default::default()
    })
}

#[expect(clippy::unnecessary_wraps, reason = "the fake client's script holds Results, and the caller compares Options")]
pub fn calls(calls: &[(&str, &str, &str)]) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion {
        tool_calls: calls
            .iter()
            .map(|&(id, name, arguments)| ToolCall {
                id: id.into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { name: name.into(), arguments: arguments.into() },
            })
            .collect(),
        ..Default::default()
    })
}

#[test]
fn the_model_can_be_switched() {
    let agent = build(vec![]).0;
    assert_eq!(agent.model(), Some(test_model()));
    let other = ModelSpec::new(Arc::new(Provider::builtin("openai").unwrap()), Model::new("gpt-4.1"));
    agent.apply(Change::SetModel(other.clone()));
    assert_eq!(agent.model(), Some(other));
}

#[tokio::test]
async fn every_request_of_a_session_carries_its_id_across_a_model_switch() {
    let reply = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
    );
    let server = MockServer::start(vec![("/responses", vec![(200, reply.into()), (200, reply.into())])]).await;
    let provider =
        Arc::new(Provider::new("gateway", "Gateway", LoginMethod::None, Protocol::Responses, server.url.clone()));
    let spec = |id: &str| ModelSpec::new(Arc::clone(&provider), Model::new(id));
    let session = SessionId::mint();
    let agent = Agent::new(Arc::new(CredentialStore::in_memory())).with_session_id(session).with_model(spec("first"));

    agent.respond("hi".into(), |_| {}).await.unwrap();
    agent.apply(Change::SetModel(spec("second")));
    agent.respond("hi".into(), |_| {}).await.unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["prompt_cache_key"], session.to_string(), "{body}");
    }
}

#[test]
fn settings_are_read_and_changed_together() {
    let agent = build(vec![text("a")]).0;
    assert_eq!(agent.settings(), Settings { model: Some(test_model()), thinking: Thinking::Medium });
    agent.apply(Change::SetThinking(Thinking::Max));
    let other = ModelSpec::new(Arc::new(Provider::builtin("ollama").unwrap()), Model::new("other"));
    agent.apply(Change::SetModel(other.clone()));
    assert_eq!(agent.settings(), Settings { model: Some(other), thinking: Thinking::Max });
}

#[test]
fn clearing_the_conversation_ends_its_sub_agents() {
    let agent = build(vec![])
        .0
        .with_sub_agents(Arc::new(SubAgents::new(Delegation { max_per_session: 1, ..Delegation::default() })));
    let spawn = || {
        agent
            .sub_agents()
            .spawn(&Persona::new("tester", "Tester", ""), || Agent::new(Arc::new(CredentialStore::in_memory())))
    };
    assert_eq!(spawn().unwrap(), "tester-1");
    assert!(spawn().is_err(), "the Root agent's limit applies");

    assert!(agent.clear_conversation());
    assert_empty(agent.sub_agents().infos());
    assert_eq!(spawn().unwrap(), "tester-2");
}
