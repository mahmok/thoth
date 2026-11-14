use alloc::rc::Rc;
use alloc::sync::Arc;
use core::mem;

use super::*;
use crate::agent::Event as AgentEvent;
use crate::agent::controller::scripted::Controller as ScriptedAgent;
use crate::agent::tools::tool::Outcome as ToolResult;
use crate::auth::flow::LoginEvent;
use crate::harness::{Event as HarnessEvent, Harness};
use crate::llm::client::chat::Usage;
use crate::llm::provider::Provider;
use crate::llm::provider::{Model, ModelSpec, Protocol};
use crate::llm::thinking::Thinking;
use crate::models::refresh::Event as RefreshEvent;
use crate::models::store::Models as ModelsStore;
use crate::session::view::Login as LoginView;
use crate::test::assert_empty;
use crate::test::fixtures::{models, store_with};
use crate::test::harness::{Scripted, TestHarness};
use crate::transcript::Content;
use crate::transcript::tool_call::ToolStatus;

fn bonsai() -> ModelSpec {
    ModelSpec::new(
        Arc::new(Provider::builtin("ollama").unwrap()),
        Model::new("bonsai-2:2b").with_protocol(Some(Protocol::OpenAi)),
    )
}

fn device_code() -> LoginEvent {
    LoginEvent::DeviceCode { url: "https://github.com/login/device".into(), code: "ABCD-1234".into() }
}

#[test]
fn files_are_suggested_for_the_at_sign_and_referenced_files_are_attached_on_send() {
    let mut h = TestHarness::new();
    h.files.wait();
    let files = h.harness.suggest_files();
    assert_eq!(files.title, "Files");
    let cargo = files.items.iter().find(|item| item.label == "Cargo.toml").unwrap();
    assert_eq!(cargo.full, "@Cargo.toml ");

    h.send("see @Cargo.toml ");
    let sent = h.agent.prompts();
    assert!(sent[0].starts_with("see @Cargo.toml "), "{sent:?}");
    assert!(sent[0].contains("<file path=\"Cargo.toml\">"), "{sent:?}");
    assert_eq!(h.said_by(Speaker::You), ["see @Cargo.toml "]);
}

#[test]
fn loaded_skills_are_announced_in_the_transcript() {
    let h = TestHarness::with_agent(ScriptedAgent::new().with_skill("demo", "a demo").with_skill("tdd", "Test first"));
    assert_eq!(h.said(), [(Speaker::Skills, "2 loaded\ndemo, tdd".to_owned())]);
}

#[test]
fn no_skills_means_no_announcement() {
    assert_empty(TestHarness::new().said());
}

#[test]
fn a_startup_notice_is_a_harness_notice_shown_as_an_error() {
    let mut h = TestHarness::new();
    h.harness = mem::replace(&mut h.harness, Harness::new(Scripted::new(ScriptedAgent::new(), &[]).services(models())))
        .with_notice("Could not read auth.json.");
    h.poll();
    assert_eq!(h.said_by(Speaker::Error), ["Could not read auth.json."]);
}

#[test]
fn sending_shows_the_message_starts_a_turn_and_is_accepted() {
    let mut h = TestHarness::new();
    assert!(h.send("hello"));
    assert_eq!(h.said(), [(Speaker::You, "hello".to_owned())]);
    assert_eq!(h.agent.prompts(), ["hello"]);
    assert!(h.view().busy);
}

#[test]
fn an_empty_prompt_starts_no_turn() {
    let mut h = TestHarness::new();
    assert!(!h.send(""));
    assert_empty(h.agent.prompts());
    assert!(!h.view().busy);
}

#[test]
fn a_second_prompt_is_not_accepted_while_a_turn_is_running() {
    let mut h = TestHarness::new();
    h.send("first");
    assert!(!h.send("second"));
    assert_eq!(h.agent.prompts(), ["first"]);
    assert_eq!(h.said_by(Speaker::You), ["first"]);
}

#[test]
fn a_finished_turn_lets_the_next_prompt_go() {
    let mut h = TestHarness::new();
    h.send("first");
    h.done();
    assert!(h.send("second"));
    assert_eq!(h.agent.prompts(), ["first", "second"]);
}

#[test]
fn a_failed_turn_shows_the_error_and_ends_the_turn() {
    let mut h = TestHarness::new();
    h.send("first");
    h.reply(Reply::Failed("connection refused".into()));
    assert_eq!(h.said_by(Speaker::Error), ["connection refused"]);
    assert!(!h.view().busy);
    h.send("again");
    assert_eq!(h.agent.prompts(), ["first", "again"]);
}

#[test]
fn the_reply_streams_into_the_transcript() {
    let mut h = TestHarness::new();
    h.send("hi");
    h.text("Hello ");
    h.text("**there**");
    assert_eq!(h.said_by(Speaker::Agent), ["Hello **there**"]);
}

#[test]
fn tool_calls_are_tracked_and_failures_are_marked() {
    let mut h = TestHarness::new();
    h.send("look");
    for (summary, result) in
        [("read_file(a)", ToolResult::ok("contents")), ("read_file(../b)", ToolResult::error("outside"))]
    {
        h.reply(Reply::Event(AgentEvent::ToolStarted { id: summary.into(), summary: summary.into() }));
        h.reply(Reply::Event(AgentEvent::ToolFinished { id: summary.into(), summary: summary.into(), result }));
    }
    let statuses: Vec<_> = h
        .view()
        .transcript
        .items()
        .iter()
        .filter_map(|item| match item.content {
            Content::Tool(ref call) => Some(call.status),
            Content::Message(_) => None,
        })
        .collect();
    assert_eq!(statuses, [ToolStatus::Done, ToolStatus::Failed]);
}

#[test]
fn usage_is_counted_in_the_stats() {
    let mut h = TestHarness::new();
    h.send("hi");
    h.reply(Reply::Done {
        usage: Some(Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15, ..Default::default() }),
    });
    let stats = h.view().stats;
    assert_eq!((stats.requests, stats.total_tokens()), (1, 15));
}

#[test]
fn the_view_starts_with_the_settings_and_empty_stats() {
    let h = TestHarness::new();
    assert_eq!(h.view().settings.model, Some(bonsai()));
    assert_eq!(h.view().stats, Stats::default());
}

#[test]
fn the_view_has_no_model_when_none_is_selected() {
    let h = TestHarness::with_agent(ScriptedAgent::new().without_model());
    assert_eq!(h.view().settings.model, None);
}

#[test]
fn cancelling_with_nothing_to_cancel_changes_nothing() {
    let mut h = TestHarness::new();
    h.cancel();
    assert!(h.events.is_empty());
    assert_eq!(h.agent.interrupted(), 0);
}

#[test]
fn cancelling_interrupts_a_running_turn() {
    let mut h = TestHarness::new();
    h.send("hi");
    h.cancel();
    assert_eq!(h.agent.interrupted(), 1);
}

#[test]
fn a_prompt_that_looks_like_a_command_is_sent_as_typed() {
    let mut h = TestHarness::new();
    h.send("/nothing");
    assert_eq!(h.agent.prompts(), ["/nothing"]);
}

#[test]
fn the_device_code_is_shown_and_kept_in_the_view_while_the_login_waits() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    h.worker.event(device_code());
    h.poll();
    let said = h.said_by(Speaker::Login).join("\n");
    assert!(said.contains("https://github.com/login/device"), "{said}");
    assert!(said.contains("ABCD-1234"), "{said}");
    assert_eq!(h.harness.login_view().device_code.as_deref(), Some("ABCD-1234"));
}

#[test]
fn finishing_a_login_says_so_and_clears_the_login_state() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("anthropic").unwrap());
    h.worker.event(device_code());
    h.worker.event(LoginEvent::Done(Provider::builtin("anthropic").unwrap()));
    h.poll();
    assert!(h.said_by(Speaker::Login).contains(&"Logged in to Anthropic.".to_owned()));
    assert_eq!(h.harness.login_view(), &LoginView::default());
}

#[test]
fn a_failed_login_shows_the_error() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    h.worker.event(LoginEvent::Failed("device flow failed: access_denied".into()));
    h.poll();
    assert_eq!(h.said_by(Speaker::Error), ["device flow failed: access_denied"]);
}

#[test]
fn an_api_key_goes_to_the_login_and_never_to_the_agent() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("openai").unwrap());
    h.worker.event(LoginEvent::PromptSecret { message: "Paste your OpenAI API key (input is hidden)".into() });
    h.poll();
    assert!(h.harness.login_view().awaiting_secret);

    assert!(h.send("sk-secret"));
    assert_eq!(h.worker.answers(), ["sk-secret"]);
    assert_empty(h.agent.prompts());
    assert!(h.said().iter().all(|entry| !entry.1.contains("sk-secret")), "the key is not echoed");
}

#[test]
fn cancel_ends_a_pending_login() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("openai").unwrap());
    h.worker.event(LoginEvent::PromptSecret { message: "Paste".into() });
    h.poll();
    h.cancel();
    assert_eq!(h.worker.cancelled(), 1);
    assert_eq!(h.said_by(Speaker::Login).last().unwrap(), "Login cancelled.");
    assert_eq!(h.harness.login_view(), &LoginView::default());

    h.send("hello");
    assert_eq!(h.agent.prompts(), ["hello"], "the prompt is normal again");
    assert_empty(h.worker.answers());
}

#[test]
fn a_login_event_after_a_cancel_does_not_bring_the_key_prompt_back() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("openai").unwrap());
    h.worker.event(LoginEvent::PromptSecret { message: "Paste".into() });
    h.cancel();
    h.poll();
    assert!(!h.harness.login_view().awaiting_secret);
}

#[test]
fn refresh_models_starts_a_refresh_of_every_provider_and_reports_each_one() {
    let mut h = TestHarness::new();
    h.harness.refresh_models();
    h.poll();
    assert_eq!(h.refresh_worker.started(), [Provider::builtins()]);
    assert_eq!(h.said_by(Speaker::Models), ["Refreshing models..."]);

    h.refresh_worker.event(RefreshEvent::Fetched { provider: Provider::builtin("ollama").unwrap(), count: 3 });
    h.refresh_worker.event(RefreshEvent::Skipped(Provider::builtin("github-copilot").unwrap()));
    h.refresh_worker
        .event(RefreshEvent::Failed { provider: Provider::builtin("openai").unwrap(), error: "http error".into() });
    h.refresh_worker.event(RefreshEvent::Finished);
    h.poll();
    assert_eq!(
        h.said_by(Speaker::Models),
        ["Refreshing models...", "Ollama: 3 models", "GitHub Copilot: skipped (not logged in)"]
    );
    assert_eq!(h.said_by(Speaker::Error), ["OpenAI: failed (http error)"]);
}

#[test]
fn a_second_refresh_waits_for_the_first_and_refresh_works_while_a_turn_runs() {
    let mut h = TestHarness::new();
    h.send("hello");
    h.harness.refresh_models();
    h.harness.refresh_models();
    h.poll();
    assert_eq!(h.refresh_worker.started().len(), 1);
    assert!(h.said_by(Speaker::Models).contains(&"A refresh is already running.".to_owned()));
}

#[test]
fn logging_in_refreshes_just_that_provider() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("openai").unwrap());
    h.worker.event(LoginEvent::Done(Provider::builtin("openai").unwrap()));
    h.poll();
    assert!(h.said_by(Speaker::Login).contains(&"Logged in to OpenAI.".to_owned()));
    assert_eq!(h.refresh_worker.started(), [vec![Provider::builtin("openai").unwrap()]]);

    h.refresh_worker.event(RefreshEvent::Fetched { provider: Provider::builtin("openai").unwrap(), count: 18 });
    h.refresh_worker.event(RefreshEvent::Finished);
    h.poll();
    assert!(h.said_by(Speaker::Models).contains(&"OpenAI: 18 models".to_owned()));
}

#[test]
fn a_failed_login_does_not_refresh_anything() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    h.worker.event(LoginEvent::Failed("device flow failed".into()));
    h.poll();
    assert_empty(h.refresh_worker.started());
}

#[test]
fn login_works_while_a_turn_runs() {
    let mut h = TestHarness::new();
    h.send("hello");
    h.harness.login(Provider::builtin("openai").unwrap());
    assert_eq!(h.worker.started(), [Provider::builtin("openai").unwrap()]);
}

#[test]
fn polling_with_nothing_pending_changes_nothing() {
    let mut h = TestHarness::new();
    h.harness.poll();
    assert!(h.harness.poll().is_empty());
}

#[test]
fn the_view_follows_the_events_it_emitted() {
    let mut h = TestHarness::new();
    let mut mirror = h.view().clone();
    h.send("hi");
    h.text("hello");
    h.done();
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    h.worker.event(device_code());
    h.poll();
    for event in &h.events {
        mirror.apply(event);
    }

    assert_eq!(mirror.busy, h.view().busy);
    assert_eq!(mirror.stats, h.view().stats);
    let last_login = h.harness_events.iter().rev().find_map(|e| match *e {
        HarnessEvent::Login(ref login) => Some(login),
        HarnessEvent::Session { .. } | HarnessEvent::SessionClosed(_) | HarnessEvent::Notice { .. } => None,
    });
    assert_eq!(last_login, Some(h.harness.login_view()));
    assert_eq!(mirror.transcript.items().len(), h.view().transcript.items().len());
}

#[test]
fn the_skills_are_listed_and_expanded_when_sent() {
    let mut h = TestHarness::with_agent(ScriptedAgent::new().with_skill("tdd", "Test first"));
    let skills = h.session().skills();
    assert_eq!(skills[0].name, "tdd");

    h.send("/skill:tdd fix the bug");
    assert_eq!(h.agent.prompts(), ["<skill name=\"tdd\">body</skill> fix the bug"]);
    assert_eq!(h.said_by(Speaker::You), ["/skill:tdd fix the bug"]);
}

#[test]
fn setting_a_model_applies_it_and_updates_the_settings() {
    let mut h = TestHarness::new();
    let llama = ModelSpec::new(Arc::new(Provider::builtin("ollama").unwrap()), Model::new("llama3.2:3b"));
    assert!(h.harness.models().iter().any(|entry| entry.0 == llama));
    assert_eq!(h.session().set_model(llama.clone()), Ok(()));
    assert_eq!(h.agent.model(), Some(llama));
    assert_eq!(h.view().settings.model.as_ref().map(ModelSpec::label).as_deref(), Some("llama3.2:3b (ollama/openai)"));
    assert!(h.agent.prompts().is_empty(), "choosing never sends a prompt");
}

#[test]
fn setting_a_thinking_level_applies_it_and_updates_the_settings() {
    let mut h = TestHarness::new();
    h.session().set_thinking(Thinking::High);
    assert_eq!(h.agent.settings().thinking, Thinking::High);
    assert_eq!(h.view().settings.thinking, Thinking::High);
}

#[test]
fn clear_empties_the_transcript_and_the_conversation() {
    let mut h = TestHarness::new();
    h.send("hello");
    h.done();
    assert_eq!(h.session().clear(), Ok(()));
    assert_empty(h.said());
    assert_eq!(h.agent.cleared(), 1);
}

#[test]
fn compacting_keeps_the_session_busy_until_the_agent_is_done_and_waits_for_an_idle_one() {
    let mut h = TestHarness::new();
    assert_eq!(h.session().compact(Some("the parser".into())), Ok(()));
    assert!(h.view().busy);
    assert_eq!(h.session().compact(None), Err(Refusal::Busy));
    assert_eq!(h.agent.compactions(), [Some("the parser".to_owned())]);
    h.done();
    assert!(!h.view().busy);
}

#[test]
fn model_and_clear_are_refused_while_a_turn_runs() {
    let mut h = TestHarness::new();
    h.send("hello");
    assert_eq!(h.session().clear(), Err(Refusal::Busy));
    assert_eq!(h.agent.cleared(), 0);
    let other = ModelSpec::new(Arc::new(Provider::builtin("openai").unwrap()), Model::new("gpt-4o"));
    assert_eq!(h.session().set_model(other), Err(Refusal::Busy));
    assert_eq!(h.agent.model(), Some(bonsai()));
}

#[test]
fn the_models_are_whatever_the_store_holds_across_providers() {
    let mut h = TestHarness::new();
    let gpt = ModelSpec::new(Arc::new(Provider::builtin("openai").unwrap()), Model::new("gpt-4o"));
    assert!(h.harness.models().iter().any(|entry| entry.0 == gpt));
    h.session().set_model(gpt.clone()).unwrap();
    assert!(h.worker.started().is_empty(), "choosing a model never starts a login");
    assert_eq!(h.agent.model(), Some(gpt));
}

#[test]
fn an_empty_store_has_no_models() {
    let h = TestHarness::with_models(store_with(&[]));
    assert_empty(h.harness.models());
}

#[test]
fn logging_in_starts_the_provider_and_shows_it() {
    let mut h = TestHarness::with_logged_in(&[Provider::builtin("openai").unwrap()]);
    assert_eq!(h.harness.login_state(Provider::builtin("openai").unwrap()), LoginState::Saved);
    assert_eq!(h.harness.login_state(Provider::builtin("github-copilot").unwrap()), LoginState::NotLoggedIn);
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    h.poll();
    assert_eq!(h.worker.started(), [Provider::builtin("github-copilot").unwrap()]);
    assert_eq!(h.said_by(Speaker::Login), ["Logging in to GitHub Copilot..."]);
    assert!(h.harness.login_view().active);
    assert!(!h.harness.login_view().awaiting_secret);
}

#[test]
fn the_login_list_adds_custom_providers_that_need_a_key() {
    let models = store_with(&[]);
    let mut gateway = Provider::custom("gateway");
    gateway.display_name = "Gateway".into();
    models.save(&gateway).unwrap();
    let h = TestHarness::with_models(models);
    let ids: Vec<String> = h.harness.login_providers().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, ["github-copilot", "openai", "anthropic", "google", "gateway"]);
}

#[test]
fn logout_removes_the_saved_login() {
    let mut h = TestHarness::with_logged_in(&[Provider::builtin("github-copilot").unwrap()]);
    assert_eq!(h.harness.saved_logins(), [Provider::builtin("github-copilot").unwrap()]);
    h.harness.logout(Provider::builtin("github-copilot").unwrap());
    h.poll();
    assert_eq!(h.store.read(&Provider::builtin("github-copilot").unwrap()), None);
    assert_empty(h.harness.saved_logins());
    assert_eq!(h.said_by(Speaker::Login), ["Logged out of GitHub Copilot."]);
}

#[test]
fn a_session_that_is_not_viewed_keeps_receiving_its_replies() {
    let mut h = TestHarness::new();
    h.send("hello");
    h.harness.start_session_with(2, Box::new(ScriptedAgent::new()));
    h.viewing = 2;
    assert_eq!(h.harness.sessions(), [(1, true), (2, false)]);

    h.agent.reply(Reply::Event(AgentEvent::Text("hi".into())));
    h.agent.reply(Reply::Done { usage: None });
    let events = h.harness.poll();
    assert!(
        events.iter().all(|e| matches!(e, HarnessEvent::Session { id: 1, .. })) && !events.is_empty(),
        "the background session's events are tagged with its id"
    );
    assert_eq!(h.harness.sessions(), [(1, false), (2, false)]);

    h.viewing = 1;
    assert_eq!(h.said_by(Speaker::Agent), ["hi"]);
    assert!(!h.view().busy);
}

#[test]
fn a_prompt_can_be_sent_to_and_cancelled_in_a_session_that_is_not_viewed() {
    let mut h = TestHarness::new();
    let other = ScriptedAgent::new();
    h.harness.start_session_with(2, Box::new(Rc::clone(&other)));
    assert!(h.harness.submit(1, "to one".into(), "to one".into()));
    assert_eq!(h.agent.prompts(), ["to one"]);
    assert_empty(other.prompts());
    assert_eq!(h.harness.sessions(), [(1, true), (2, false)]);

    h.harness.cancel_session(1);
    assert_eq!(h.agent.interrupted(), 1);
    assert_eq!(other.interrupted(), 0);
    assert!(h.harness.session(1).is_some() && h.harness.session_mut(2).is_some());
    assert!(h.harness.session(3).is_none());
}

#[test]
fn the_login_is_the_harness_s_and_is_not_tied_to_a_session() {
    let mut h = TestHarness::new();
    h.harness.start_session_with(2, Box::new(ScriptedAgent::new()));
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    h.worker.event(device_code());
    h.poll();
    assert_eq!(h.harness.login_view().device_code.as_deref(), Some("ABCD-1234"));
    assert!(h.harness.login_view().active);
}

#[test]
fn notices_belong_to_no_session() {
    let mut h = TestHarness::new();
    h.harness.login(Provider::builtin("github-copilot").unwrap());
    let events = h.harness.poll();
    assert!(events.iter().any(|e| matches!(
        e,
        HarnessEvent::Notice { speaker: Speaker::Login, text } if text.contains("GitHub Copilot")
    )));
    assert!(h.said_by(Speaker::Login).is_empty(), "nothing was written into the transcript");
}

#[test]
fn sessions_are_created_without_being_viewed_and_personas_are_listed() {
    use crate::persona::{Info as PersonaInfo, Persona};

    let scripted = Scripted::new(ScriptedAgent::new(), &[]);
    let mut services = scripted.services(models());
    services.personas.add(Persona::new("helper", "Helper", "helps")).unwrap();
    let mut harness = Harness::new(services);
    assert_eq!(
        harness.list_personas(),
        [PersonaInfo { key: "helper".into(), name: "Helper".into(), description: "helps".into() }]
    );

    let first = harness.create_session("helper").unwrap();
    let second = harness.create_session("helper").unwrap();
    assert_ne!(first, second);

    // Each session has an agent of its own: changing one leaves the other alone.
    harness.session_mut(first).unwrap().set_thinking(Thinking::High);
    assert_eq!(harness.session(first).unwrap().view().settings.thinking, Thinking::High);
    harness.session_mut(second).unwrap().poll();
    assert_ne!(harness.session(second).unwrap().view().settings.thinking, Thinking::High);

    assert_eq!(harness.create_session("missing"), None);
    assert!(harness.poll().iter().any(|e| matches!(e, HarnessEvent::Notice { speaker: Speaker::Error, .. })));
}

#[test]
fn closing_a_session_stops_its_turn_and_forgets_it() {
    let mut h = TestHarness::new();
    h.harness.start_session_with(2, Box::new(ScriptedAgent::new()));
    h.harness.submit(1, "hi".into(), "hi".into());

    assert!(h.harness.close_session(1));
    assert_eq!(h.agent.interrupted(), 1);
    assert_eq!(h.harness.sessions(), [(2, false)]);
    assert!(h.harness.session(1).is_none());
    assert!(h.harness.poll().iter().any(|e| matches!(e, HarnessEvent::SessionClosed(1))));
    assert!(!h.harness.close_session(1));
    assert!(!h.harness.submit(1, "late".into(), "late".into()));
}
