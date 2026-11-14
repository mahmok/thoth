//! Tests for the Harness itself: the route a client takes to a Turn.

#![expect(clippy::future_not_send, reason = "the Harness holds scripted workers, and the test drives it on one thread")]
#![expect(clippy::result_large_err, reason = "the fake client's script holds Results, as the product's client returns")]

use alloc::sync::Arc;
use core::time::Duration;

use tokio::time;

use super::*;
use crate::agent::COMPACTION_CALL_ID;
use crate::agent::context::{Budget, Compaction, Context as AgentContext};
use crate::agent::skills::SkillSet;
use crate::agent_builder::AgentBuilder;
use crate::auth::session::Login as LoginSession;
use crate::auth::testing::scripted::Worker as ScriptedLoginWorker;
use crate::llm::client::StreamedCompletion;
use crate::llm::client::error::Failure as ClientError;
use crate::llm::client::fake::Client as FakeClient;
use crate::llm::provider::Model;
use crate::models::refresh::Session as RefreshSession;
use crate::models::store::InMemory as InMemoryModelsStore;
use crate::models::testing::scripted::Worker as ScriptedRefreshWorker;
use crate::persona::{Persona, Repository as PersonaRepository};
use crate::services::Services;
use crate::test::temp_tree::TempTree;
use crate::transcript::Content;

#[expect(clippy::unnecessary_wraps, reason = "the fake client's script holds Results, and the caller compares Options")]
fn reply(text: &str) -> Result<StreamedCompletion, ClientError> {
    Ok(StreamedCompletion { text: text.into(), ..Default::default() })
}

/// A Harness with one Persona whose Agent waits on `fake`, and a budget that compacts on any
/// conversation at all.
fn over_budget(fake: FakeClient, tree: &TempTree) -> Harness {
    let budget = Budget { elision: None, compaction: Some(Compaction { budget: 1, limit_share: 1.0, keep_recent: 0 }) };
    let store = Arc::new(CredentialStore::in_memory());
    let models = Arc::new(InMemoryModelsStore::default());
    let builder = AgentBuilder::new(
        Arc::clone(&store),
        models.clone(),
        tree.workspace(),
        AgentContext::new(tree.path()).with_budget(budget),
        SkillSet::default(),
    )
    .with_fake_client(fake);
    let mut personas = PersonaRepository::new();
    personas.add(Persona::new("root", "Root", "").with_model(Model::reference("ollama", "m"))).unwrap();
    Harness::new(Services {
        builder,
        credentials: store,
        files: Arc::new(FileReferences::new(tree.workspace())),
        logins: LoginSession::new(Box::new(ScriptedLoginWorker::new()), Arc::new(CredentialStore::in_memory())),
        models,
        personas,
        refresh: RefreshSession::new(Box::new(ScriptedRefreshWorker::new())),
    })
}

/// Polls until the Session is idle, so a Turn started by `submit` has finished.
async fn settle(harness: &mut Harness, id: u64) {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let _events = harness.poll();
            if !harness.session(id).unwrap().view().busy {
                break;
            }
            time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the turn should finish");
}

async fn say(harness: &mut Harness, id: u64, prompt: &str) {
    harness.submit(id, prompt.into(), prompt.into());
    settle(harness, id).await;
}

#[tokio::test]
async fn the_budget_still_applies_to_a_turn_submitted_through_the_harness() {
    let fake = FakeClient::new(vec![reply("a"), reply("b"), reply("gist"), reply("c")]);
    let tree = TempTree::new("harness-budget");
    let mut harness = over_budget(fake.clone(), &tree);
    let id = harness.create_session("root").unwrap();
    for prompt in ["1", "2"] {
        say(&mut harness, id, prompt).await;
    }
    assert_eq!(fake.requests().len(), 2, "small turns do not compact");

    say(&mut harness, id, "3").await;

    assert_eq!(fake.requests().len(), 4, "the third turn asked for a summary, then the model");
    let transcript = &harness.session(id).unwrap().view().transcript;
    let compacted = transcript
        .items()
        .iter()
        .find_map(|item| match item.content {
            Content::Tool(ref call) if call.summary.starts_with("compact") => Some(call),
            Content::Tool(_) | Content::Message(_) => None,
        })
        .expect("the compaction shows as a Tool card");
    assert_eq!(compacted.summary, "compact 1 turns into a summary");
    assert!(!compacted.is_running());
    assert_eq!(compacted.call_id, COMPACTION_CALL_ID);
}
