//! Tests for one Turn: its steps, its Tool calls and the events it reports.

use alloc::sync::Arc;
use core::future;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::time;

use super::*;
use crate::adapters::auth::store::Credentials as CredentialStore;
use crate::adapters::context::AgentContext;
use crate::adapters::llm::client::chat::Role;
use crate::adapters::llm::client::error::Failure as ClientError;
use crate::adapters::llm::client::fake;
use crate::adapters::llm::client::fake::Client as FakeClient;
use crate::adapters::llm::client::protocol::ProtocolClient;
use crate::domain::conversation::budget::Budget;
use crate::runtime::agent::tests::{Echo, EchoArgs, build, call, calls, test_model, text};
use crate::runtime::agent::{AgentChange, DEFAULT_MAX_STEPS};
use crate::runtime::tools::tool::{Description, Outcome as ToolResult, Tool, ToolContext, Traits as ToolTraits};
use crate::runtime::tools::tool_set::ToolSet;
use crate::test::assert_empty;

/// Like `Echo`, but it changes nothing, so calls to it may run together.
#[derive(Debug)]
struct Peek;

impl Tool for Peek {
    type Args = EchoArgs;

    fn describe(&self) -> Description {
        Description { description: "Look at the text", name: "peek" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }

    fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
        future::ready(ToolResult::ok(format!("peek: {}", args.text)))
    }

    fn summary(&self, args: &EchoArgs) -> String {
        args.text.clone()
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits::read_only()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

/// Reports one status and says what its context holds: its call id and whether it is cancelled.
#[derive(Debug)]
struct Probe {
    read_only: bool,
}

impl Tool for Probe {
    type Args = EchoArgs;

    fn describe(&self) -> Description {
        Description { description: "Report what the call can see", name: "probe" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }

    fn run(&self, args: EchoArgs, cx: &ToolContext) -> impl Future<Output = ToolResult> {
        cx.report("working");
        future::ready(ToolResult::ok(format!("{}:{}", args.text, cx.cancel.is_cancelled())))
    }

    fn summary(&self, args: &EchoArgs) -> String {
        args.text.clone()
    }

    fn traits(&self) -> ToolTraits {
        if self.read_only { ToolTraits::read_only() } else { ToolTraits::default() }
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

/// Cancels the Turn it runs in, then reports whether its own context saw that.
#[derive(Debug)]
struct CancelParent {
    parent: CancellationToken,
    saw_cancel: Arc<AtomicBool>,
}

impl Tool for CancelParent {
    type Args = EchoArgs;

    fn describe(&self) -> Description {
        Description { description: "Cancel the Turn", name: "cancel_parent" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }

    fn run(&self, _args: EchoArgs, cx: &ToolContext) -> impl Future<Output = ToolResult> {
        self.parent.cancel();
        self.saw_cancel.store(cx.cancel.is_cancelled(), Ordering::SeqCst);
        future::ready(ToolResult::ok("cancelled"))
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

/// Spends tokens on a model of its own, as a sub-agent would.
#[derive(Debug)]
struct Spend;

impl Tool for Spend {
    type Args = EchoArgs;

    fn describe(&self) -> Description {
        Description { description: "Use another model", name: "spend" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }

    fn run(&self, _args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
        future::ready(ToolResult::ok("spent").with_usage(Usage {
            prompt_tokens: 5,
            completion_tokens: 2,
            total_tokens: 7,
            ..Default::default()
        }))
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

/// Waits far longer than any test, until it is cancelled.
#[derive(Debug)]
struct Sleepy;

impl Tool for Sleepy {
    type Args = EchoArgs;

    fn describe(&self) -> Description {
        Description { description: "Wait", name: "sleepy" }
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }

    async fn run(&self, _args: EchoArgs, _cx: &ToolContext) -> ToolResult {
        time::sleep(Duration::from_secs(60)).await;
        ToolResult::ok("woke")
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

fn agent(script: Vec<Result<StreamedCompletion, ClientError>>) -> Agent {
    build(script).0
}

fn agent_with_tool(tool: impl Tool, script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
    let mut tools = ToolSet::new();
    tools.register(tool).unwrap();
    let fake = FakeClient::new(script);
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
        .with_model(test_model())
        .with_client(ProtocolClient::Fake(fake.clone()))
        .with_tools(tools);
    (agent, fake)
}

fn with_peek(script: Vec<Result<StreamedCompletion, ClientError>>) -> (Agent, FakeClient) {
    let mut tools = ToolSet::new();
    tools.register(Echo).unwrap();
    tools.register(Peek).unwrap();
    let fake = FakeClient::new(script);
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
        .with_model(test_model())
        .with_client(ProtocolClient::Fake(fake.clone()))
        .with_tools(tools);
    (agent, fake)
}

/// The tool events, each as its kind and the id of its call.
fn tool_events(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match *event {
            AgentEvent::ToolStarted { ref id, .. } => Some(format!("start {id}")),
            AgentEvent::ToolProgress { ref id, ref status } => Some(format!("progress {id} {status}")),
            AgentEvent::ToolFinished { ref id, .. } => Some(format!("finish {id}")),
            AgentEvent::Text(_) | AgentEvent::Reasoning(_) | AgentEvent::StepStarted { .. } | AgentEvent::Notice(_) => {
                None
            },
        })
        .collect()
}

fn big(tokens: usize) -> String {
    "x".repeat(tokens.saturating_mul(4))
}

async fn roles(agent: &Agent) -> Vec<Role> {
    roles_of(agent).await
}

async fn roles_of(agent: &Agent) -> Vec<Role> {
    agent.conversation.lock().await.messages().iter().map(|m| m.role).collect()
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn runs_a_tool_then_answers() {
    let agent = agent(vec![call("call_1", "echo", r#"{"text":"hi"}"#), text("done")]);

    let mut events = Vec::new();
    let response = agent.respond("say hi".into(), |e| events.push(e)).await.unwrap();

    assert_eq!(response.message.text.unwrap(), "done");
    let shown: Vec<AgentEvent> = events.into_iter().filter(|e| !matches!(e, AgentEvent::StepStarted { .. })).collect();
    assert!(matches!(&shown[0], AgentEvent::ToolStarted { summary, .. } if summary == "echo(hi)"));
    assert!(matches!(
        &shown[1],
        AgentEvent::ToolFinished { summary, result, .. }
            if summary == "echo(hi)" && *result == ToolResult::ok("echo: hi")
    ));
    assert!(matches!(&shown[2], AgentEvent::Text(t) if t == "done"));

    assert_eq!(roles(&agent).await, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
    let conversation = agent.conversation.lock().await;
    let tool_message = &conversation.messages()[2];
    assert_eq!(tool_message.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(tool_message.content.as_deref(), Some("echo: hi"));
}

#[tokio::test]
async fn offers_the_tool_set_and_sends_results_back() {
    let (agent, fake) = build(vec![call("c", "echo", r#"{"text":"x"}"#), text("ok")]);
    agent.respond("go".into(), |_| {}).await.unwrap();

    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|entry| entry.1.len() == 1 && entry.1[0].name == "echo"));
    assert_eq!(requests[0].0, 1, "first request: just the user message");
    assert_eq!(requests[1].0, 3, "second request: user, assistant tool call, tool result");
}

#[tokio::test]
async fn an_agent_without_tools_offers_none() {
    let fake = FakeClient::new(vec![text("hi")]);
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
        .with_model(test_model())
        .with_client(ProtocolClient::Fake(fake.clone()));
    agent.respond("hello".into(), |_| {}).await.unwrap();
    assert_empty(&fake.requests()[0].1);
}

#[tokio::test]
async fn unknown_tools_and_bad_arguments_reach_the_model_as_errors() {
    let agent = agent(vec![call("a", "launch_missiles", "{}"), call("b", "echo", "{not json"), text("sorry")]);
    let mut events = Vec::new();
    agent.respond("hi".into(), |e| events.push(e)).await.unwrap();

    let errors: Vec<&ToolResult> = events
        .iter()
        .filter_map(|e| match *e {
            AgentEvent::ToolFinished { ref result, .. } if result.is_error => Some(result),
            AgentEvent::Text(_)
            | AgentEvent::Reasoning(_)
            | AgentEvent::StepStarted { .. }
            | AgentEvent::ToolStarted { .. }
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::Notice(_)
            | AgentEvent::ToolFinished { .. } => None,
        })
        .collect();
    assert_eq!(errors.len(), 2);
    assert!(errors[0].content.contains("unknown tool"));
    assert!(errors[1].content.contains("invalid arguments"));
}

#[tokio::test]
async fn a_failed_turn_that_only_read_leaves_the_conversation_unchanged() {
    let (agent, _) = with_peek(vec![text("first"), call("c", "peek", r#"{"text":"x"}"#), Err(fake::failure())]);
    agent.respond("one".into(), |_| {}).await.unwrap();
    let before = roles(&agent).await;

    let err = agent.respond("two".into(), |_| {}).await.err().unwrap();
    assert!(matches!(err, Failure::Client(_)));
    assert_eq!(roles(&agent).await, before);
    assert_eq!(before, [Role::User, Role::Assistant]);
}

#[tokio::test]
async fn a_failed_turn_that_changed_something_keeps_its_tool_steps() {
    let agent = agent(vec![call("c", "echo", r#"{"text":"x"}"#), Err(fake::failure())]);
    let err = agent.respond("edit".into(), |_| {}).await.err().unwrap();
    assert!(matches!(err, Failure::Client(_)));
    assert_eq!(roles(&agent).await, [Role::User, Role::Assistant, Role::Tool]);
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn an_interrupt_between_calls_still_records_the_calls_that_ran() {
    let parent = CancellationToken::new();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let mut tools = ToolSet::new();
    tools.register(CancelParent { parent: parent.clone(), saw_cancel }).unwrap();
    tools.register(Echo).unwrap();
    let fake = FakeClient::new(vec![calls(&[
        ("a", "cancel_parent", r#"{"text":"x"}"#),
        ("b", "echo", r#"{"text":"later"}"#),
    ])]);
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()))
        .with_model(test_model())
        .with_client(ProtocolClient::Fake(fake))
        .with_tools(tools);
    let result = agent.respond_under(&parent, "go".into(), |_| {}).await;
    assert!(matches!(result, Err(Failure::Interrupted)));

    let conversation = agent.conversation.lock().await;
    let results: Vec<_> =
        conversation.messages().iter().filter(|m| m.role == Role::Tool).map(|m| m.content.clone().unwrap()).collect();
    assert_eq!(results.len(), 2, "every call has a result");
    assert_eq!(results[0], "cancelled");
    assert!(results[1].contains("interrupted by the user"), "{}", results[1]);
}

#[tokio::test]
async fn stopping_at_the_step_limit_keeps_the_tool_steps() {
    let script = (0..DEFAULT_MAX_STEPS).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
    let agent = agent(script);
    let result = agent.respond("loop".into(), |_| {}).await;
    assert!(matches!(result, Err(Failure::StepLimit(DEFAULT_MAX_STEPS))));
    let roles = roles(&agent).await;
    assert_eq!(roles.len(), 1 + 2 * DEFAULT_MAX_STEPS);
    assert_eq!(roles[0], Role::User);
}

#[tokio::test]
async fn an_interrupt_stops_the_running_turn_and_the_ones_queued_behind_it() {
    let (agent, fake) = agent_with_tool(Sleepy, vec![call("c", "sleepy", r#"{"text":"x"}"#), text("never")]);
    let both = async {
        tokio::join!(agent.respond("one".into(), |_| {}), agent.respond("two".into(), |_| {}), async {
            time::sleep(Duration::from_millis(50)).await;
            agent.interrupt();
        })
    };
    let (first, second, ()) =
        time::timeout(Duration::from_secs(10), Box::pin(both)).await.expect("the interrupt reached the running Turn");
    assert!(matches!(first, Err(Failure::Interrupted)));
    assert!(matches!(second, Err(Failure::Interrupted)));
    assert_eq!(fake.requests().len(), 1, "the queued Turn never asked the model");
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn a_failed_tool_call_is_stored_as_an_error_result() {
    let agent = agent(vec![call("c", "echo", "{not json"), text("done")]);
    agent.respond("go".into(), |_| {}).await.unwrap();
    let conversation = agent.conversation.lock().await;
    let tool = conversation.messages().iter().find(|m| m.role == Role::Tool).unwrap();
    assert!(tool.is_error);
}

#[tokio::test]
async fn a_large_prompt_counts_toward_compaction() {
    let budget = Budget {
        elision: None,
        compaction: Some(crate::domain::conversation::budget::Compaction {
            budget: 2_000,
            limit_share: 1.0,
            keep_recent: 0,
        }),
    };
    let (plain, fake) = build(vec![text("a"), text("b"), text("summary"), text("c")]);
    let agent = plain.with_context(&AgentContext::new(PathBuf::new().as_path()).with_budget(budget));
    agent.respond("1".into(), |_| {}).await.unwrap();
    agent.respond("2".into(), |_| {}).await.unwrap();
    assert_eq!(fake.requests().len(), 2, "small turns do not compact");
    agent.respond(big(3_000), |_| {}).await.unwrap();
    assert_eq!(fake.requests().len(), 4, "the prompt alone is over the budget");
}

#[tokio::test]
async fn the_estimate_is_corrected_by_the_tokens_the_provider_counted() {
    let mut reply = text("ok").unwrap();
    reply.usage =
        Some(Usage { prompt_tokens: 100_000, completion_tokens: 1, total_tokens: 100_001, ..Default::default() });
    let agent = agent(vec![Ok(reply)]);
    assert_eq!(Load::new(100, 0).tokens(agent.calibration()), 100, "no correction before any request");
    agent.respond("hi".into(), |_| {}).await.unwrap();
    assert_eq!(Load::new(100, 0).tokens(agent.calibration()), 300, "the ratio is capped at 3");
    agent.apply(AgentChange::SetModel(test_model()));
    assert_eq!(Load::new(100, 0).tokens(agent.calibration()), 100, "another model starts afresh");
}

#[tokio::test]
async fn the_last_step_must_answer_in_text_and_ends_the_turn_with_a_reply() {
    let mut script: Vec<_> = (0i32..2i32).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
    script.push(text("ran out of steps; here is what is done"));
    let (agent, fake) = build(script);
    let response = agent.with_max_steps(3).respond("loop".into(), |_| {}).await.unwrap();
    assert_eq!(response.message.text.as_deref(), Some("ran out of steps; here is what is done"));
    assert_eq!(fake.tool_choices(), [ToolChoice::Auto, ToolChoice::Auto, ToolChoice::None]);
}

#[tokio::test]
async fn a_turn_of_one_step_keeps_its_tools() {
    let (agent, fake) = build(vec![text("hi")]);
    agent.with_max_steps(1).respond("go".into(), |_| {}).await.unwrap();
    assert_eq!(fake.tool_choices(), [ToolChoice::Auto]);
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn the_model_is_told_when_few_steps_are_left() {
    let script = (0i32..6i32).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
    let agent = agent(script).with_max_steps(6);
    let _unused = agent.respond("loop".into(), |_| {}).await;
    let conversation = agent.conversation.lock().await;
    let results: Vec<&str> = conversation
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_deref().unwrap())
        .collect();
    assert!(!results[1].contains("left in this turn"), "{}", results[1]);
    assert!(results[2].contains("3 model steps left"), "{}", results[2]);
    assert!(results[3].contains("2 model steps left"), "{}", results[3]);
    assert!(results[4].contains("1 model step left"), "{}", results[4]);
    assert!(!results[5].contains("left in this turn"), "{}", results[5]);
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn repeating_a_failed_call_gets_a_hint() {
    let agent = agent(vec![call("a", "echo", "{not json"), call("b", "echo", "{not json"), text("done")]);
    agent.respond("go".into(), |_| {}).await.unwrap();
    let conversation = agent.conversation.lock().await;
    let results: Vec<&str> = conversation
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.content.as_deref().unwrap())
        .collect();
    assert!(!results[0].contains("already failed"), "{}", results[0]);
    assert!(results[1].contains("already failed"), "{}", results[1]);
}

#[tokio::test]
async fn the_step_limit_is_each_agents_own() {
    let script = (0i32..3i32).map(|i| call(&format!("c{i}"), "echo", r#"{"text":"x"}"#)).collect();
    let agent = agent(script).with_max_steps(2);
    let result = agent.respond("loop".into(), |_| {}).await;
    assert!(matches!(result, Err(Failure::StepLimit(2))));
    assert_eq!(roles(&agent).await.len(), 1 + 2 * 2);
}

#[tokio::test]
async fn each_model_call_starts_a_numbered_step() {
    let (agent, _) =
        agent_with_tool(Probe { read_only: false }, vec![call("c", "probe", r#"{"text":"x"}"#), text("done")]);
    let mut steps = Vec::new();
    agent
        .respond("go".into(), |e| {
            if let AgentEvent::StepStarted { step } = e {
                steps.push(step);
            }
        })
        .await
        .unwrap();
    assert_eq!(steps, [1, 2]);
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn read_only_calls_start_together_and_finish_in_order() {
    let (agent, _) =
        with_peek(vec![calls(&[("a", "peek", r#"{"text":"1"}"#), ("b", "peek", r#"{"text":"2"}"#)]), text("done")]);
    let mut events = Vec::new();
    agent.respond("go".into(), |e| events.push(e)).await.unwrap();
    assert_eq!(tool_events(&events), ["start a", "start b", "finish a", "finish b"]);
    let conversation = agent.conversation.lock().await;
    let results: Vec<_> =
        conversation.messages()[2..4].iter().map(|m| (m.tool_call_id.as_deref(), m.content.as_deref())).collect();
    assert_eq!(results, [(Some("a"), Some("peek: 1")), (Some("b"), Some("peek: 2"))]);
}

#[tokio::test]
async fn a_step_with_a_tool_that_changes_things_runs_one_call_at_a_time() {
    let (agent, _) =
        with_peek(vec![calls(&[("a", "peek", r#"{"text":"1"}"#), ("b", "echo", r#"{"text":"2"}"#)]), text("done")]);
    let mut events = Vec::new();
    agent.respond("go".into(), |e| events.push(e)).await.unwrap();
    assert_eq!(tool_events(&events), ["start a", "finish a", "start b", "finish b"]);
}

#[tokio::test]
async fn a_reply_cut_off_after_some_text_is_kept_with_a_notice() {
    let cut =
        Ok(StreamedCompletion { text: "half an ans".into(), cut_off: Some("max_tokens".into()), ..Default::default() });
    let agent = agent(vec![cut]);
    let mut events = Vec::new();
    let response = agent.respond("hi".into(), |e| events.push(e)).await.unwrap();
    assert_eq!(response.message.text.as_deref(), Some("half an ans"));
    assert!(matches!(events.last(), Some(AgentEvent::Notice(n)) if n.contains("continue")));
    assert_eq!(roles(&agent).await, [Role::User, Role::Assistant]);
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn a_tool_call_cut_off_mid_argument_is_not_run_and_the_model_is_told() {
    let cut = Ok(StreamedCompletion {
        tool_calls: calls(&[("a", "echo", r#"{"text":"fine"}"#), ("b", "echo", r#"{"text":"cut of"#)])
            .unwrap()
            .tool_calls,
        cut_off: Some("max_tokens".into()),
        ..Default::default()
    });
    let agent = agent(vec![cut, text("smaller now")]);
    let response = agent.respond("go".into(), |_| {}).await.unwrap();
    assert_eq!(response.message.text.as_deref(), Some("smaller now"));

    let conversation = agent.conversation.lock().await;
    assert_eq!(conversation.messages()[2].content.as_deref(), Some("echo: fine"));
    let told = conversation.messages()[3].content.as_deref().unwrap();
    assert!(told.starts_with("error:") && told.contains("cut off"), "{told}");
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn a_request_that_is_too_long_is_retried_once_with_old_tool_output_elided() {
    let overflow = || Err(ClientError::BadRequest("prompt is too long: 300000 tokens".into()));
    let (agent, fake) = build(vec![
        call("c", "echo", &json!({ "text": big(5_000) }).to_string()),
        text("one"),
        overflow(),
        text("two"),
    ]);
    agent.respond("first".into(), |_| {}).await.unwrap();
    let mut events = Vec::new();
    agent.respond("second".into(), |e| events.push(e)).await.unwrap();

    assert_eq!(fake.requests().len(), 4);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Notice(_))));
    let conversation = agent.conversation.lock().await;
    let result = conversation.messages()[2].content.as_deref().unwrap();
    assert!(result.starts_with(r#"{"elided""#), "{result}");
}

#[tokio::test]
async fn too_long_with_nothing_to_elide_is_an_error_not_a_retry() {
    let overflow = Err(ClientError::BadRequest("prompt is too long".into()));
    let (agent, fake) = build(vec![overflow]);
    let err = agent.respond("hi".into(), |_| {}).await.err().unwrap();
    assert!(matches!(err, Failure::Client(_)));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn old_tool_output_is_elided_between_steps_but_what_the_model_has_not_read_is_not() {
    let budget = Budget {
        elision: Some(Elision { trigger: 100, target: 100, smallest: 10 }),
        compaction: Some(crate::domain::conversation::budget::Compaction {
            budget: 1_000,
            limit_share: 1.0,
            keep_recent: 0,
        }),
    };
    let arguments = json!({ "text": big(2_000) }).to_string();
    let (plain, _) = build(vec![call("a", "echo", &arguments), call("b", "echo", &arguments), text("done")]);
    let agent = plain.with_context(&AgentContext::new(PathBuf::new().as_path()).with_budget(budget));
    agent.respond("go".into(), |_| {}).await.unwrap();

    let conversation = agent.conversation.lock().await;
    let elided = |index: usize| conversation.messages()[index].content.as_deref().unwrap().starts_with(r#"{"elided""#);
    // system, user, [assistant, tool] for a, [assistant, tool] for b, assistant
    assert!(elided(3), "the first result is old by the last request");
    assert!(!elided(5), "the last result was only just read");
}

/// Two turns; the first leaves about 32k tokens of tool arguments and output behind.
async fn second_turn_after_bulky_first(agent: Agent) -> Agent {
    agent.respond("first".into(), |_| {}).await.unwrap();
    agent.respond("second".into(), |_| {}).await.unwrap();
    agent
}

fn bulky_script() -> Vec<Result<StreamedCompletion, ClientError>> {
    let arguments = json!({ "text": big(8_000) }).to_string();
    vec![calls(&[("a", "echo", &arguments), ("b", "echo", &arguments)]), text("one"), text("two")]
}

fn first_result_elided(conversation: &Conversation) -> bool {
    let first = conversation.messages().iter().find(|m| m.role == Role::Tool).unwrap();
    first.content.as_deref().unwrap().starts_with(r#"{"elided""#)
}

#[tokio::test]
async fn old_tool_output_is_kept_between_turns_while_the_context_has_room() {
    let (fresh, _) = build(bulky_script());
    let agent = second_turn_after_bulky_first(fresh).await;
    assert!(!first_result_elided(&*agent.conversation.lock().await));
}

#[tokio::test]
async fn old_tool_output_is_elided_between_turns_once_the_context_is_half_full() {
    let budget = Budget {
        compaction: Some(crate::domain::conversation::budget::Compaction {
            budget: 40_000,
            limit_share: 1.0,
            keep_recent: 20_000,
        }),
        ..Budget::default()
    };
    let (fresh, _) = build(bulky_script());
    let configured = fresh.with_context(&AgentContext::new(PathBuf::new().as_path()).with_budget(budget));
    let agent = second_turn_after_bulky_first(configured).await;
    assert!(first_result_elided(&*agent.conversation.lock().await));
}

#[tokio::test]
async fn a_tool_sees_its_call_id_and_reports_progress_on_its_own_card() {
    let (agent, _) =
        agent_with_tool(Probe { read_only: false }, vec![call("call_9", "probe", r#"{"text":"x"}"#), text("done")]);
    let mut events = Vec::new();
    agent.respond("go".into(), |e| events.push(e)).await.unwrap();

    assert_eq!(tool_events(&events), ["start call_9", "progress call_9 working", "finish call_9"]);
    let conversation = agent.conversation.lock().await;
    assert_eq!(
        conversation.messages()[2].content.as_deref(),
        Some("x:false"),
        "the tool saw its own context and no cancellation"
    );
}

#[tokio::test]
async fn read_only_calls_report_progress_on_their_own_cards_while_they_run_together() {
    let (agent, _) = agent_with_tool(
        Probe { read_only: true },
        vec![calls(&[("a", "probe", r#"{"text":"1"}"#), ("b", "probe", r#"{"text":"2"}"#)]), text("done")],
    );
    let mut events = Vec::new();
    agent.respond("go".into(), |e| events.push(e)).await.unwrap();

    assert_eq!(
        tool_events(&events),
        ["start a", "start b", "progress a working", "progress b working", "finish a", "finish b",]
    );
}

#[tokio::test]
async fn cancelling_the_parent_cancels_the_turn_and_the_tool_sees_it() {
    let parent = CancellationToken::new();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let (agent, fake) = agent_with_tool(
        CancelParent { parent: parent.clone(), saw_cancel: Arc::clone(&saw_cancel) },
        vec![call("c", "cancel_parent", r#"{"text":"x"}"#), text("never")],
    );
    let result = agent.respond_under(&parent, "go".into(), |_| {}).await;

    assert!(matches!(result, Err(Failure::Interrupted)));
    assert!(saw_cancel.load(Ordering::SeqCst));
    assert_eq!(fake.requests().len(), 1, "no model call after the cancel");
}

#[tokio::test]
async fn a_turn_whose_parent_is_already_cancelled_makes_no_model_call() {
    let parent = CancellationToken::new();
    parent.cancel();
    let (agent, fake) = build(vec![text("never")]);
    let result = agent.respond_under(&parent, "go".into(), |_| {}).await;
    assert!(matches!(result, Err(Failure::Interrupted)));
    assert_empty(fake.requests());
    assert_empty(&roles_of(&agent).await);
}

#[tokio::test]
async fn a_tools_usage_counts_toward_the_turn_with_the_models_own() {
    let (agent, _) = agent_with_tool(Spend, {
        let mut first = call("c", "spend", r#"{"text":"x"}"#).unwrap();
        first.usage = Some(Usage { prompt_tokens: 10, completion_tokens: 1, total_tokens: 11, ..Default::default() });
        vec![Ok(first), text("done")]
    });
    let response = agent.respond("go".into(), |_| {}).await.unwrap();
    let total = response.usage.unwrap();
    assert_eq!((total.prompt_tokens, total.completion_tokens, total.total_tokens), (15, 3, 18));
}

#[tokio::test]
async fn a_turns_usage_is_the_tools_when_the_model_reports_none() {
    let (agent, _) = agent_with_tool(Spend, vec![call("c", "spend", r#"{"text":"x"}"#), text("done")]);
    let response = agent.respond("go".into(), |_| {}).await.unwrap();
    assert_eq!(response.usage.map(|u| u.total_tokens), Some(7));
}

#[tokio::test]
async fn an_empty_reply_leaves_the_conversation_unchanged() {
    let agent = agent(vec![text("")]);
    let response = agent.respond("hi".into(), |_| {}).await.unwrap();
    assert_eq!(response.message.text.unwrap(), "");
    assert_empty(&roles(&agent).await);
}

#[tokio::test]
#[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
async fn an_empty_final_reply_keeps_prior_tool_steps_in_conversation() {
    let (agent, fake) = build(vec![call("c", "echo", r#"{"text":"x"}"#), text(""), text("next reply")]);

    let response = agent.respond("run tool".into(), |_| {}).await.unwrap();
    assert_eq!(response.message.text.as_deref(), Some(""));
    assert_eq!(roles(&agent).await, [Role::User, Role::Assistant, Role::Tool]);
    {
        let conversation = agent.conversation.lock().await;
        let tool_message = &conversation.messages()[2];
        assert_eq!(tool_message.tool_call_id.as_deref(), Some("c"));
        assert_eq!(tool_message.content.as_deref(), Some("echo: x"));
    }

    agent.respond("continue".into(), |_| {}).await.unwrap();
    assert_eq!(fake.requests()[2].0, 4, "next turn includes the prior tool step");
}

#[tokio::test]
async fn turns_never_interleave() {
    let agent = agent(vec![
        call("a", "echo", r#"{"text":"1"}"#),
        text("reply one"),
        call("b", "echo", r#"{"text":"2"}"#),
        text("reply two"),
    ]);
    let (first, second) = tokio::join!(agent.respond("one".into(), |_| {}), agent.respond("two".into(), |_| {}),);
    first.unwrap();
    second.unwrap();

    let expected = [Role::User, Role::Assistant, Role::Tool, Role::Assistant];
    let all = roles(&agent).await;
    assert_eq!(&all[..4], &expected);
    assert_eq!(&all[4..], &expected);
}

#[tokio::test]
async fn usage_is_summed_across_steps() {
    let usage = |p, c| Usage { prompt_tokens: p, completion_tokens: c, total_tokens: p + c, ..Default::default() };
    let mut first = call("c", "echo", r#"{"text":"x"}"#).unwrap();
    first.usage = Some(usage(10, 2));
    let mut second = text("done").unwrap();
    second.usage = Some(usage(30, 5));
    let agent = agent(vec![Ok(first), Ok(second)]);

    let total = agent.respond("hi".into(), |_| {}).await.unwrap().usage.unwrap();
    assert_eq!((total.prompt_tokens, total.completion_tokens, total.total_tokens), (40, 7, 47));
}

#[tokio::test]
async fn the_system_prompt_is_kept_across_turns() {
    let agent = agent(vec![text("a"), text("b")]).with_context(&AgentContext::new(PathBuf::new().as_path()));
    agent.respond("1".into(), |_| {}).await.unwrap();
    agent.respond("2".into(), |_| {}).await.unwrap();
    assert_eq!(roles(&agent).await, [Role::System, Role::User, Role::Assistant, Role::User, Role::Assistant]);
}

#[tokio::test]
async fn an_agent_without_a_model_says_how_to_get_one_and_keeps_the_conversation_clean() {
    let agent = Agent::new(Arc::new(CredentialStore::in_memory()));
    assert_eq!(agent.model(), None);
    let err = agent.respond("hi".into(), |_| {}).await.err().unwrap();
    assert!(matches!(err, Failure::NoModel));
    assert_eq!(err.to_string(), "No model selected. Run /refresh-models, then /model.");
    assert_empty(&roles_of(&agent).await);
}

#[tokio::test]
async fn clearing_keeps_the_system_prompt_and_drops_the_conversation() {
    let agent = agent(vec![text("a")]).with_context(&AgentContext::new(PathBuf::new().as_path()));
    agent.respond("1".into(), |_| {}).await.unwrap();
    assert!(agent.clear_conversation());
    assert_eq!(roles(&agent).await, [Role::System]);
}
