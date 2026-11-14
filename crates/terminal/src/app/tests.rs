use alloc::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Color;
use thoth_core::agent::Event as AgentEvent;
use thoth_core::agent::controller::Reply;
use thoth_core::agent::controller::scripted::Controller as ScriptedAgent;
use thoth_core::agent::tools::tool::Outcome as ToolResult;
use thoth_core::auth::flow::LoginEvent;
use thoth_core::llm::client::chat::Usage;
use thoth_core::llm::provider::Provider;
use thoth_core::test::scripted::Scripted;

use crate::keymap;
use crate::testing::{HEIGHT, Harness};
use crate::theme;

#[test]
fn the_screen_is_only_marked_changed_when_something_happened() {
    let mut h = Harness::new();
    h.app.poll_events();
    assert!(!h.app.poll_events(), "nothing happened since the last look");
    h.reply(Reply::Event(AgentEvent::Text("hi".into())));
    assert!(h.app.poll_events());
    assert!(!h.app.poll_events());
}

#[test]
fn sending_shows_the_message_and_waits_for_the_reply() {
    let mut h = Harness::new();
    h.send("hello");
    let screen = h.screen();
    assert!(screen.contains("you: hello"), "{screen}");
    assert!(screen.contains("Waiting for response..."), "{screen}");
    assert_eq!(h.scripted.agent.prompts(), ["hello"]);
}

#[test]
fn a_prompt_sent_while_waiting_stays_in_the_input() {
    let mut h = Harness::new();
    h.send("first");
    h.send("second");
    let screen = h.screen();
    assert!(screen.contains("second") && !screen.contains("you: second"), "{screen}");
    assert_eq!(h.scripted.agent.prompts(), ["first"]);

    h.done();
    h.screen();
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.agent.prompts(), ["first", "second"]);
}

#[test]
fn the_input_is_cleared_after_sending() {
    let mut h = Harness::new();
    h.send("hello");
    h.done();
    h.screen();
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.agent.prompts(), ["hello"], "nothing left to send");
}

#[test]
fn a_failed_turn_shows_the_error() {
    let mut h = Harness::new();
    h.send("first");
    h.reply(Reply::Failed("connection refused".into()));
    let screen = h.screen();
    assert!(screen.contains("error: connection refused"), "{screen}");
}

#[test]
fn the_reply_streams_into_the_chat_as_markdown() {
    let mut h = Harness::new();
    h.send("hi");
    h.reply(Reply::Event(AgentEvent::Text("Hello ".into())));
    h.reply(Reply::Event(AgentEvent::Text("**there**".into())));
    let screen = h.screen();
    assert!(screen.contains("agent:"), "{screen}");
    assert!(screen.contains("Hello there") && !screen.contains("**"), "{screen}");
}

#[test]
fn tool_calls_show_and_failures_stand_out() {
    let mut h = Harness::new();
    h.send("look");
    for (summary, result) in [
        ("read_file(a)", ToolResult::ok("contents")),
        ("read_file(../b)", ToolResult::error("outside the working directory")),
    ] {
        h.reply(Reply::Event(AgentEvent::ToolStarted { id: summary.into(), summary: summary.into() }));
        h.reply(Reply::Event(AgentEvent::ToolFinished { id: summary.into(), summary: summary.into(), result }));
    }
    let screen = h.screen();
    for expected in ["read_file(a)", "done", "contents", "failed", "outside the working directory"] {
        assert!(screen.contains(expected), "{expected}:\n{screen}");
    }
}

#[test]
fn clicking_a_card_header_expands_it() {
    let mut h = Harness::new();
    h.send("look");
    h.reply(Reply::Event(AgentEvent::ToolStarted { id: "1".into(), summary: "bash(ls)".into() }));
    h.reply(Reply::Event(AgentEvent::ToolFinished {
        id: "1".into(),
        summary: "bash(ls)".into(),
        result: ToolResult::ok("1\n2\n3\n4\n5"),
    }));
    let screen = h.screen();
    let header = u16::try_from(screen.lines().position(|line| line.contains("bash(ls)")).unwrap()).unwrap();
    assert!(!screen.contains('4'), "{screen}");

    h.mouse(MouseEventKind::Down(MouseButton::Left), 3, header);
    h.mouse(MouseEventKind::Up(MouseButton::Left), 3, header);
    let opened = h.screen();
    assert!(opened.contains('4') && opened.contains('5'), "{opened}");
}

#[test]
fn usage_is_counted_in_the_footer() {
    let mut h = Harness::new();
    h.send("hi");
    h.reply(Reply::Done {
        usage: Some(Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15, ..Default::default() }),
    });
    let screen = h.screen();
    assert!(screen.contains("Requests: 1") && screen.contains("15 tokens"), "{screen}");
}

#[test]
fn a_large_paste_shows_a_placeholder_and_sends_everything() {
    let mut h = Harness::new();
    h.paste("line one\nline two\nline three");
    assert!(h.screen().contains("[Pasted 3 lines #1]"));
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.agent.prompts(), ["line one\nline two\nline three"]);
    let screen = h.screen();
    assert!(screen.contains("you: [Pasted 3 lines #1]"), "the chat keeps the compact form:\n{screen}");
}

#[test]
fn up_on_an_empty_prompt_brings_back_the_last_message_sent() {
    let mut h = Harness::new();
    h.send("first");
    h.done();
    h.send("second");
    h.done();
    h.screen();
    h.press(KeyCode::Up);
    let screen = h.screen();
    assert!(screen.contains("second") && !screen.contains("you: second\n"), "{screen}");
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.agent.prompts(), ["first", "second", "second"]);
}

#[test]
fn up_on_an_empty_prompt_with_nothing_sent_just_scrolls() {
    let mut h = Harness::new();
    h.press(KeyCode::Up);
    assert_eq!(h.app.prompt.text(), "");
}

#[test]
fn up_in_a_prompt_with_text_scrolls_instead_of_recalling() {
    let mut h = Harness::new();
    h.send("first");
    h.done();
    h.type_text("draft");
    h.press(KeyCode::Up);
    assert_eq!(h.app.prompt.text(), "draft");
}

#[test]
fn a_recalled_paste_is_sent_in_full() {
    let mut h = Harness::new();
    h.paste("line one\nline two\nline three");
    h.press(KeyCode::Enter);
    h.done();
    h.screen();
    h.press(KeyCode::Up);
    h.press(KeyCode::Enter);
    let full = "line one\nline two\nline three";
    assert_eq!(h.scripted.agent.prompts(), [full, full]);
}

#[test]
fn a_pasted_block_survives_completing_a_file_reference_after_it() {
    let mut h = Harness::new();
    h.paste("line one\nline two\nline three");
    h.type_text(" @Cargo.to");
    h.press(KeyCode::Enter);
    h.press(KeyCode::Enter);
    let sent = h.scripted.agent.prompts();
    assert!(sent[0].starts_with("line one\nline two\nline three @Cargo.toml "), "{sent:?}");
}

#[test]
fn ctrl_d_quits_and_other_keys_do_not() {
    let mut h = Harness::new();
    h.press(KeyCode::Char('a'));
    h.press(KeyCode::Enter);
    assert!(!h.app.quit);
    h.press_with_modifiers(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(h.app.quit);
}

#[test]
fn unbound_keys_are_typed_into_the_prompt() {
    let mut h = Harness::new();
    h.type_text("abc");
    assert!(h.screen().contains("abc"));
}

#[test]
fn the_hints_line_comes_from_the_keymap() {
    let mut h = Harness::new();
    let screen = h.screen();
    let expected = keymap::hints(keymap::Context::Always);
    assert!(screen.contains(&expected), "{screen}");
}

#[test]
fn the_footer_shows_the_model_and_thinking_level_on_the_right() {
    let mut h = Harness::new();
    assert!(h.footer().ends_with("bonsai-2:2b (ollama/openai) \u{b7} thinking medium"), "{}", h.footer());
}

#[test]
fn the_footer_says_when_no_model_is_selected() {
    let mut h = Harness::with_agent(ScriptedAgent::new().without_model());
    assert!(h.footer().ends_with("no model \u{b7} thinking medium"));
}

#[test]
fn a_startup_notice_is_shown_as_an_error() {
    let mut h =
        Harness::with(ScriptedAgent::new(), Some("Could not read auth.json. Logins will not be saved this session."));
    let screen = h.screen();
    assert!(screen.contains("error: Could not read auth.json"), "{screen}");
}

#[test]
fn loaded_skills_are_listed_at_the_top_of_the_chat() {
    let mut h = Harness::with_agent(ScriptedAgent::new().with_skill("demo", "a demo"));
    let screen = h.screen();
    assert!(screen.contains("skills: 1 loaded") && screen.contains("demo"), "{screen}");
}

#[test]
fn the_header_shows_the_name_and_whether_the_agent_is_busy() {
    let mut h = Harness::new();
    let screen = h.screen();
    assert!(screen.contains("THOTH") && screen.contains("idle"), "{screen}");
    h.send("hi");
    assert!(h.screen().contains("running"));
    h.done();
    assert!(h.screen().contains("idle"));
}

/// Every colour drawn, in the foreground or background, as the cells of the last frame.
fn colours_drawn(h: &Harness) -> BTreeSet<String> {
    let buffer = h.terminal.backend().buffer();
    buffer.content().iter().flat_map(|cell| [cell.fg, cell.bg]).map(|colour| format!("{colour:?}")).collect()
}

#[test]
fn only_palette_colours_are_drawn() {
    use crate::theme::palette::*;
    let theme = theme::current();
    let mut allowed: BTreeSet<String> = [
        BACKGROUND,
        SURFACE,
        ELEVATED,
        SELECTED_SURFACE,
        TEXT,
        TEXT_SECONDARY,
        TEXT_MUTED,
        TEAL,
        TEAL_BRIGHT,
        GOLD,
        GOLD_MUTED,
        BORDER,
        RUNNING,
        IDLE,
        ERROR,
    ]
    .into_iter()
    .filter_map(|shade| theme.color(shade))
    .map(|colour| format!("{colour:?}"))
    .collect();
    allowed.insert(format!("{:?}", Color::Reset));

    let within = |h: &mut Harness, what: &str| {
        h.screen();
        let drawn = colours_drawn(h);
        assert!(drawn.is_subset(&allowed), "{what}: {drawn:?} not within {allowed:?}");
    };

    // Each kind of content gets its own screen so that none scrolls out of view.
    for (what, markdown) in [
        ("headings and inline", "# Title\n\n## Sub\n\nA [link](https://x.test) and `code`."),
        ("lists and quotes", "- one\n- two\n\n> quoted"),
        ("tables", "| a | b |\n|---|---|\n| 1 | 2 |"),
    ] {
        let mut h = Harness::new();
        h.send("look");
        h.reply(Reply::Event(AgentEvent::Text(markdown.into())));
        let screen = h.screen();
        assert!(!screen.contains("]("), "{what} was not rendered:\n{screen}");
        within(&mut h, what);
    }

    let mut h = Harness::new();
    h.send("look");
    for (summary, result) in [("read_file(a)", ToolResult::ok("contents")), ("read_file(b)", ToolResult::error("nope"))]
    {
        h.reply(Reply::Event(AgentEvent::ToolStarted { id: summary.into(), summary: summary.into() }));
        h.reply(Reply::Event(AgentEvent::ToolFinished { id: summary.into(), summary: summary.into(), result }));
    }
    within(&mut h, "tool cards");
    h.type_text("/");
    within(&mut h, "picker");
}

#[test]
fn resizing_the_terminal_wraps_the_chat_to_the_new_width() {
    let mut h = Harness::new();
    h.send("alpha beta gamma delta epsilon zeta eta theta iota kappa");
    h.done();
    let wide = h.screen();
    assert!(wide.lines().any(|line| line.contains("you:") && line.contains("kappa")), "{wide}");

    h.resize(40, HEIGHT);
    let narrow = h.screen();
    assert!(narrow.contains("kappa"), "{narrow}");
    assert!(
        !narrow.lines().any(|line| line.contains("you:") && line.contains("kappa")),
        "the message is wrapped again: {narrow}"
    );
}

#[test]
fn page_keys_scroll_the_chat() {
    let mut h = Harness::new();
    for i in 0i32..20i32 {
        h.send(&format!("message {i}"));
        h.done();
        h.screen();
    }
    let screen = h.screen();
    assert!(screen.contains("message 19") && !screen.contains("message 0"), "{screen}");
    for _ in 0i32..10i32 {
        h.press(KeyCode::PageUp);
        h.screen();
    }
    let paged_up = h.screen();
    assert!(paged_up.contains("message 0") && !paged_up.contains("message 19"), "{paged_up}");
    for _ in 0i32..10i32 {
        h.press(KeyCode::PageDown);
        h.screen();
    }
    assert!(h.screen().contains("message 19"));
}

#[test]
fn sending_jumps_back_to_the_newest_message() {
    let mut h = Harness::new();
    for i in 0i32..20i32 {
        h.send(&format!("message {i}"));
        h.done();
        h.screen();
    }
    for _ in 0i32..10i32 {
        h.press(KeyCode::PageUp);
        h.screen();
    }
    h.send("latest");
    assert!(h.screen().contains("you: latest"));
}

#[test]
fn clearing_empties_the_chat() {
    let mut h = Harness::new();
    h.send("hello");
    h.done();
    h.screen();
    h.type_text("/clear");
    h.press(KeyCode::Enter);
    assert!(!h.screen().contains("you: hello"));
}

#[test]
fn a_slash_opens_the_command_picker_with_its_own_hints() {
    let mut h = Harness::with_agent(ScriptedAgent::new().with_skill("tdd", "Test first"));
    h.type_text("/");
    let screen = h.screen();
    for expected in ["Commands", "/model", "/clear", "/persona", "/session", "Esc: close"] {
        assert!(screen.contains(expected), "{expected}:\n{screen}");
    }
    h.type_text("sk");
    assert!(h.screen().contains("/skill:tdd"));
}

#[test]
fn typing_filters_the_picker_and_says_when_nothing_matches() {
    let mut h = Harness::new();
    h.type_text("/mo");
    let screen = h.screen();
    assert!(screen.contains("/model") && !screen.contains("/clear"), "{screen}");
    h.type_text("zz");
    assert!(h.screen().contains("no matches"));
}

#[test]
fn escape_closes_the_picker_first_and_doesnt_quit_the_app() {
    let mut h = Harness::new();
    h.type_text("/m");
    h.press(KeyCode::Esc);
    assert!(!h.app.quit);
    assert!(!h.screen().contains("Commands"));
    h.press(KeyCode::Esc);
    assert!(!h.app.quit);
}

#[test]
fn arrows_move_the_picker_selection_instead_of_scrolling() {
    let mut h = Harness::new();
    h.type_text("/");
    h.press(KeyCode::Down);
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.refresh_worker.started().len(), 1, "third item is /refresh-models");
}

#[test]
fn tab_completes_without_running() {
    let mut h = Harness::new();
    h.type_text("/cl");
    h.press(KeyCode::Tab);
    let screen = h.screen();
    assert!(screen.contains("/clear") && screen.contains("Commands"), "{screen}");
    assert_eq!(h.scripted.agent.cleared(), 0);
}

#[test]
fn busy_commands_are_labelled_and_stay_blocked() {
    let mut h = Harness::new();
    h.send("hello");
    h.type_text("/clear");
    assert!(h.screen().contains("(busy)"));
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.agent.cleared(), 0);
    assert!(h.screen().contains("Commands"), "the picker stays open");
}

#[test]
fn choosing_a_model_shows_it_in_the_footer() {
    let mut h = Harness::new();
    h.type_text("/model");
    h.press(KeyCode::Enter);
    assert!(h.screen().contains("Models"));
    h.type_text("llama3");
    h.press(KeyCode::Enter);
    assert!(h.footer().ends_with("llama3.2:3b (ollama/openai) \u{b7} thinking medium"));
}

#[test]
fn an_at_sign_opens_a_file_picker_that_inserts_a_reference() {
    let mut h = Harness::new();
    h.type_text("see @Cargo.to");
    let screen = h.screen();
    assert!(screen.contains("Files") && screen.contains("Cargo.toml"), "{screen}");
    h.press(KeyCode::Enter);
    let inserted = h.screen();
    assert!(inserted.contains("see @Cargo.toml"), "{inserted}");
    assert!(!inserted.contains("Files"));
}

fn start_login(h: &mut Harness, provider: &str) {
    h.type_text("/login");
    h.press(KeyCode::Enter);
    h.type_text(provider);
    h.press(KeyCode::Enter);
}

#[test]
fn a_pending_login_shows_its_hint_and_escape_cancels_it_instead_of_quitting() {
    let mut h = Harness::new();
    start_login(&mut h, "copilot");
    let screen = h.screen();
    assert!(screen.contains("Logging in to GitHub Copilot..."), "{screen}");
    assert!(screen.contains("Esc: cancel login"), "{screen}");
    h.press(KeyCode::Esc);
    assert!(!h.app.quit);
    assert!(h.screen().contains("Login cancelled."));
}

#[test]
fn an_api_key_is_typed_hidden_and_not_shown_afterwards() {
    let mut h = Harness::new();
    start_login(&mut h, "openai");
    h.scripted.worker.event(LoginEvent::PromptSecret { message: "Paste your OpenAI API key (input is hidden)".into() });
    let screen = h.screen();
    assert!(screen.contains("Enter: save the key"), "{screen}");

    h.type_text("sk-secret");
    let masked = h.screen();
    assert!(!masked.contains("sk-secret") && masked.contains("\u{2022}\u{2022}\u{2022}"), "{masked}");

    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.worker.answers(), ["sk-secret"]);
    let submitted = h.screen();
    assert!(!submitted.contains("sk-secret") && !submitted.contains("\u{2022}"), "{submitted}");
}

#[test]
fn typing_is_visible_again_once_a_login_is_cancelled() {
    let mut h = Harness::new();
    start_login(&mut h, "openai");
    h.scripted.worker.event(LoginEvent::PromptSecret { message: "Paste".into() });
    h.screen();
    h.type_text("sk-half");
    h.press(KeyCode::Esc);
    h.type_text("hello");
    let screen = h.screen();
    assert!(screen.contains("hello") && !screen.contains("sk-half") && !screen.contains("\u{2022}"), "{screen}");
}

#[test]
fn the_device_code_and_link_are_shown_in_the_chat() {
    let mut h = Harness::new();
    start_login(&mut h, "copilot");
    h.scripted
        .worker
        .event(LoginEvent::DeviceCode { url: "https://github.com/login/device".into(), code: "ABCD-1234".into() });
    let screen = h.screen();
    assert!(screen.contains("https://github.com/login/device"), "{screen}");
    assert!(screen.contains("ABCD-1234"), "{screen}");
}

#[test]
fn logging_out_says_so() {
    let mut h =
        Harness::build(Scripted::new(ScriptedAgent::new(), &[Provider::builtin("github-copilot").unwrap()]), None);
    h.type_text("/logout");
    h.press(KeyCode::Enter);
    h.press(KeyCode::Enter);
    assert!(h.screen().contains("Logged out of GitHub Copilot."));
}

#[test]
fn choosing_a_skill_fills_the_prompt_and_quit_quits() {
    let mut h = Harness::with_agent(ScriptedAgent::new().with_skill("tdd", "Test first"));
    h.type_text("/skill:tdd");
    h.press(KeyCode::Enter);
    assert_eq!(h.app.prompt.text(), "/skill:tdd ");
    h.app.set_input("");
    h.type_text("/quit");
    h.press(KeyCode::Enter);
    assert!(h.app.quit);
}

#[test]
fn a_slash_or_at_sign_inside_the_text_opens_nothing() {
    let mut h = Harness::new();
    h.type_text("look in src/ or me@host");
    assert!(!h.app.pickers.is_open());
}

#[test]
fn a_space_after_the_command_closes_the_picker() {
    let mut h = Harness::new();
    h.type_text("/model ");
    assert!(!h.app.pickers.is_open());
}

#[test]
fn enter_with_no_matching_command_sends_the_text() {
    let mut h = Harness::new();
    h.type_text("/nothing");
    h.press(KeyCode::Enter);
    assert_eq!(h.scripted.agent.prompts(), ["/nothing"]);
}

#[test]
fn escape_closes_the_model_menu_without_changing_the_model() {
    let mut h = Harness::new();
    h.type_text("/model");
    h.press(KeyCode::Enter);
    h.press(KeyCode::Esc);
    assert!(!h.screen().contains("Models"));
    assert!(h.footer().contains("bonsai-2:2b"));
}

#[test]
fn a_skill_command_fills_the_prompt_and_the_skill_is_sent_expanded() {
    let mut h = Harness::with_agent(ScriptedAgent::new().with_skill("tdd", "Test first"));
    h.type_text("/sk");
    h.press(KeyCode::Enter);
    assert_eq!(h.app.prompt.text(), "/skill:tdd ");
    assert!(!h.app.pickers.is_open());
    h.send("fix it");
    assert_eq!(h.scripted.agent.prompts(), ["<skill name=\"tdd\">body</skill> fix it"]);
}

#[test]
fn a_slash_in_the_key_prompt_opens_no_picker() {
    let mut h = Harness::new();
    start_login(&mut h, "openai");
    h.scripted.worker.event(LoginEvent::PromptSecret { message: "Paste".into() });
    h.screen();
    h.type_text("/");
    assert!(!h.app.pickers.is_open());
}

#[test]
fn the_footer_shows_the_usage_summary() {
    let mut h = Harness::new();
    assert!(h.footer().trim_start().starts_with("Requests: 0 | Last: - | Total: 0 tokens"), "{}", h.footer());
}

#[test]
fn agent_output_does_not_pull_a_reader_back_down_but_a_notice_does() {
    let mut h = Harness::new();
    for i in 0i32..20i32 {
        h.send(&format!("message {i}"));
        h.done();
        h.screen();
    }
    for _ in 0i32..10i32 {
        h.press(KeyCode::PageUp);
        h.screen();
    }
    h.reply(Reply::Event(AgentEvent::Text("streamed".into())));
    assert!(!h.screen().contains("streamed"), "reading back is not interrupted");

    h.reply(Reply::Failed("boom".into()));
    assert!(h.screen().contains("error: boom"), "a notice comes to the user");
}
