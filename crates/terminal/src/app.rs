use core::mem;
use core::time::Duration;
use std::io;
use std::io::stdout;
use std::time::Instant;
use thoth_core::llm::provider::ModelSpec;

use super::browser;
use super::chat::{ChatAction, ChatView};
use super::client::Client;
use super::clipboard;
use super::command::{self, Ran};
use super::header;
use super::keymap::{self, Action};
use super::modes::Modes;
use super::popup;
use super::prompt::{Prompt, Submission};
use crate::picker::{Catalog, Outcome, PickerHost, PickerKey, PromptText};
use crate::stats;
use crate::theme::{self, Status, Theme};
use crate::units::to_u16;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use thoth_core::builtin::files::{FILE_TRIGGER, Suggestions};
use thoth_core::harness::Event as HarnessEvent;
use thoth_core::session::event::Event as SessionEvent;
use thoth_core::session::view::Login as LoginView;
use thoth_core::transcript::{Change as TranscriptChange, Content, Item, Speaker};
use unicode_width::UnicodeWidthStr as _;

/// The characters that start a word the picker can complete.
const TRIGGERS: &[char] = &[FILE_TRIGGER];

/// How often the harness is asked what happened in the background.
const TICK: Duration = Duration::from_millis(50);
/// How often the screen is redrawn for what changes by itself: the clock on a running Tool card
/// shows tenths of a second.
const ANIMATION_TICK: Duration = Duration::from_millis(100);

/// The terminal app.
pub struct TerminalApp {
    chat: ChatView,
    harness: Client,
    pickers: PickerHost,
    prompt: Prompt,
    quit: bool,
    /// The login as the last event this app handled left it.
    seen_login: LoginView,
    submitted: Option<Submission>,
}

impl TerminalApp {
    /// Whether something on screen changes with time alone.
    fn animating(&self) -> bool {
        self.harness.view().busy || self.harness.view().transcript.has_running_tool()
    }

    fn apply(&mut self, ran: Ran) {
        match ran {
            Ran::Done => {},
            Ran::Insert(text) => self.set_input(&text),
            Ran::Choices(choices) => self.pickers.open_choices(choices),
            Ran::Quit => self.quit = true,
        }
    }

    /// Draws and handles events until the user quits.
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut dirty = true;
        let mut drawn = Instant::now();
        loop {
            dirty |= self.poll_events();
            if self.quit {
                return Ok(());
            }
            // Draw when something changed, or when a running Tool card's clock needs updating.
            if self.animating() && drawn.elapsed() >= ANIMATION_TICK {
                dirty = true;
            }
            if dirty {
                let _frame = terminal.draw(|frame| self.render(frame))?;
                dirty = false;
                drawn = Instant::now();
            }

            // Take everything that is waiting before drawing again, so a burst of
            // input (a paste, a mouse drag) costs one redraw rather than one per event.
            let mut wait = TICK;
            while event::poll(wait)? {
                self.handle_event(event::read()?);
                dirty = true;
                wait = Duration::ZERO;
            }
        }
    }

    /// Handles one terminal event.
    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => {
                if let Some(text) = self.prompt.handle_mouse(mouse) {
                    let _unused = clipboard::copy(&text);
                } else {
                    match self.chat.handle_mouse(mouse, &self.harness.view().transcript) {
                        Some(ChatAction::Copy(text)) => {
                            let _unused = clipboard::copy(&text);
                        },
                        Some(ChatAction::OpenUrl(url)) => browser::open(&url),
                        None => {},
                    }
                }
            },
            Event::Paste(pasted) => {
                self.prompt.handle_paste(&pasted);
                self.prompt_changed();
            },
            Event::FocusGained | Event::FocusLost | Event::Key(_) | Event::Resize(..) => {},
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.pickers.is_open()
            && let Some(picker_key) = picker_key(key.code)
        {
            let _outcome = self.press(picker_key);
            return;
        }
        if let Some(action) = keymap::action_for(key) {
            self.perform(action);
        } else {
            self.prompt.handle_key(key);
            self.prompt_changed();
        }
    }

    fn last_user_message(&self) -> Option<String> {
        self.harness.view().transcript.items().iter().rev().find_map(|item| match item.content {
            Content::Message(ref message) if message.speaker == Speaker::You => Some(message.text.clone()),
            Content::Message(_) | Content::Tool(_) => None,
        })
    }

    pub fn new(harness: Client) -> Self {
        let mut prompt = Prompt::new();
        let seen_login = harness.login_view().clone();
        prompt.set_masked(seen_login.awaiting_secret);
        Self {
            harness,
            seen_login,
            chat: ChatView::default(),
            prompt,
            pickers: PickerHost::default(),
            submitted: None,
            quit: false,
        }
    }

    fn on_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Transcript(TranscriptChange::Cleared) => self.chat.reset(),
            SessionEvent::Transcript(TranscriptChange::Added(item)) if is_said(&item) => {
                self.chat.scroll_down(usize::MAX);
            },
            SessionEvent::Transcript(
                TranscriptChange::Added(_) | TranscriptChange::Appended { .. } | TranscriptChange::Replaced(_),
            )
            | SessionEvent::Busy(_)
            | SessionEvent::Stats(_)
            | SessionEvent::Settings(_) => {},
        }
    }

    fn on_login(&mut self, login: &LoginView, before: &LoginView) {
        self.prompt.set_masked(login.awaiting_secret);
        if before.awaiting_secret && !login.awaiting_secret {
            // Whatever part of a secret was typed goes with the question.
            self.set_input("");
        }
        if let Some(code) = login.device_code.as_ref()
            && before.device_code.as_ref() != Some(code)
        {
            let _unused = clipboard::copy(code);
        }
    }

    /// Does what a key action asks.
    fn perform(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Cancel => self.harness.cancel(),
            Action::Submit => {
                if self.press(PickerKey::Enter) == Outcome::Unclaimed {
                    self.submit();
                }
            },
            Action::ScrollUp if self.prompt.text().is_empty() => self.recall_last_input(),
            Action::ScrollUp => self.chat.scroll_up(1),
            Action::ScrollDown => self.chat.scroll_down(1),
            Action::PageUp => self.chat.scroll_up(self.chat.page_size()),
            Action::PageDown => self.chat.scroll_down(self.chat.page_size()),
        }
    }

    /// Takes what the harness has to report. Returns whether there was anything, so the screen
    /// is only redrawn when it has changed.
    pub fn poll_events(&mut self) -> bool {
        let current = self.harness.session_id();
        let events = self.harness.poll();
        let mut changed = !events.is_empty();
        for event in events {
            match event {
                HarnessEvent::Login(login) => {
                    let before = mem::replace(&mut self.seen_login, login.clone());
                    self.on_login(&login, &before);
                },
                HarnessEvent::Session { id, event: inner } if id == current => self.on_event(inner),
                HarnessEvent::Session { .. } | HarnessEvent::SessionClosed(_) => {},
                HarnessEvent::Notice { speaker, text } => {
                    self.harness.session_mut().say(speaker, text);
                },
            }
        }
        // What the notices added to the viewed session.
        let said = self.harness.session_mut().poll();
        changed |= !said.is_empty();
        for event in said {
            self.on_event(event);
        }
        changed
    }

    /// Hands a key to the open picker, and does what it decided.
    fn press(&mut self, key: PickerKey) -> Outcome {
        let prompt = self.prompt_text();
        let mut catalog = SessionCatalog(&self.harness);
        let view = self.harness.view();
        let outcome = self.pickers.key(key, view.busy, &prompt, TRIGGERS, &mut catalog);
        match outcome {
            Outcome::Unclaimed | Outcome::Handled => {},
            Outcome::Edit(ref edited) => self.prompt.set_text(&edited.text, edited.cursor),
            Outcome::Run { ref command, ref choice } => {
                self.prompt.set_text("", 0);
                // A refusal leaves nothing to undo: the picker already checked what it could,
                // and whatever went stale since shows when the user looks again.
                let before = self.harness.session_id();
                if let Ok(ran) = command::run(&mut self.harness, command, choice.as_deref()) {
                    self.apply(ran);
                }
                if self.harness.session_id() != before {
                    self.chat.reset();
                }
                self.prompt_changed();
            },
        }
        outcome
    }

    /// Keeps the picker in step with the prompt. A secret being typed opens none.
    fn prompt_changed(&mut self) {
        if self.harness.login_view().awaiting_secret {
            return self.pickers.close();
        }
        let prompt = self.prompt_text();
        let mut catalog = SessionCatalog(&self.harness);
        self.pickers.prompt_changed(&prompt, TRIGGERS, &mut catalog);
    }

    fn prompt_text(&self) -> PromptText {
        PromptText { text: self.prompt.text(), cursor: self.prompt.cursor() }
    }

    /// Puts the user's latest message back in the empty prompt, with any pasted text in full.
    fn recall_last_input(&mut self) {
        let Some(shown) = self.last_user_message() else {
            return self.chat.scroll_up(1);
        };
        let text = match self.submitted.as_ref() {
            Some(sent) if sent.shown == shown => sent.full.clone(),
            _ => shown,
        };
        self.set_input(&text);
    }

    /// Draws the header, chat, input, picker and footer.
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let title = self.title();

        let screen = frame.area();
        theme::current().fill(frame, screen);
        let [header_area, chat_area, input_area, footer_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(3), Constraint::Length(1)])
                .areas(screen);

        header::render(frame, header_area, self.status());
        self.chat.render(frame, chat_area, &self.harness.view().transcript);
        self.prompt.render(frame, input_area, &title);

        let settings = &self.harness.view().settings;
        let model_name = settings.model.as_ref().map_or_else(|| "no model".to_owned(), ModelSpec::label);
        let model = format!("{model_name} · thinking {}", settings.thinking.label());
        let [stats_area, model_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(to_u16(model.width()).saturating_add(1))])
                .areas(Theme::pane().inner(footer_area));
        frame.render_widget(
            Paragraph::new(stats::summary(&self.harness.view().stats)).style(theme::current().muted()),
            stats_area,
        );
        frame.render_widget(Paragraph::new(model).right_aligned().style(theme::current().secondary()), model_area);

        if let Some(picker) = self.pickers.view() {
            popup::render(frame, &picker, input_area, chat_area, self.harness.view().busy);
        }
    }

    pub fn run(mut self) -> color_eyre::Result<()> {
        color_eyre::install()?;
        ratatui::run(|terminal| {
            let modes = Modes::enter(stdout())?;
            let result = self.event_loop(terminal);
            drop(modes);
            discard_pending_input();
            result
        })?;
        Ok(())
    }

    fn set_input(&mut self, text: &str) {
        self.prompt.set_text(text, text.chars().count());
        self.prompt_changed();
    }

    /// What the header says the agent is doing: a login that needs the user is waiting on them.
    fn status(&self) -> Status {
        let login = self.harness.login_view();
        if login.active || login.awaiting_secret {
            Status::Waiting
        } else if self.harness.view().busy {
            Status::Running
        } else {
            Status::Idle
        }
    }

    fn submit(&mut self) {
        self.submitted = self.prompt.submission();
        let Some(submission) = self.submitted.as_ref() else {
            return;
        };
        let (shown, full) = (submission.shown.clone(), submission.full.clone());
        if self.harness.submit(shown, full) {
            self.set_input("");
        }
    }

    fn title(&self) -> String {
        let login = self.harness.login_view();
        if self.pickers.is_open() {
            keymap::PICKER_HINTS.to_owned()
        } else if login.awaiting_secret {
            keymap::SECRET_HINTS.to_owned()
        } else if login.active {
            keymap::LOGIN_HINTS.to_owned()
        } else if self.harness.view().busy {
            "Waiting for response...".to_owned()
        } else {
            keymap::hints()
        }
    }
}

/// The pickers' lists, asked of the Session only when a picker opens.
struct SessionCatalog<'client>(&'client Client);

impl Catalog for SessionCatalog<'_> {
    fn commands(&mut self) -> Option<Vec<command::Command>> {
        Some(command::list(self.0))
    }

    fn suggestions(&mut self, trigger: char) -> Option<Suggestions> {
        (trigger == FILE_TRIGGER).then(|| self.0.suggest_files())
    }
}

/// Mouse reports already on their way when the modes were switched off would otherwise
/// reach the shell as typed text.
fn discard_pending_input() {
    while event::poll(Duration::from_millis(50)).unwrap_or(false) {
        let _unused = event::read();
    }
}

/// Whether a new item is something the user was told, which brings the chat down to it. Agent
/// output and Tool cards arrive while the user may be reading back, so they do not.
const fn is_said(item: &Item) -> bool {
    match item.content {
        Content::Message(ref message) => !matches!(message.speaker, Speaker::Agent | Speaker::Thinking),
        Content::Tool(_) => false,
    }
}

/// The picker key for a key code, if the picker uses it.
const fn picker_key(code: KeyCode) -> Option<PickerKey> {
    match code {
        KeyCode::Up => Some(PickerKey::Up),
        KeyCode::Down => Some(PickerKey::Down),
        KeyCode::Tab => Some(PickerKey::Tab),
        KeyCode::Esc => Some(PickerKey::Esc),
        KeyCode::Backspace
        | KeyCode::Enter
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::BackTab
        | KeyCode::Delete
        | KeyCode::Insert
        | KeyCode::F(_)
        | KeyCode::Char(_)
        | KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;

    use super::*;
    use alloc::collections::BTreeSet;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use thoth_core::agent::Event as AgentEvent;
    use thoth_core::agent::controller::Reply;
    use thoth_core::agent::controller::scripted::Controller as ScriptedAgent;
    use thoth_core::agent::tools::tool::Outcome as ToolResult;
    use thoth_core::auth::flow::LoginEvent;
    use thoth_core::harness;
    use thoth_core::llm::client::chat::Usage;
    use thoth_core::llm::provider::Provider;
    use thoth_core::models::testing::fixture;
    use thoth_core::test::scripted::Scripted;

    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 16;

    struct Harness {
        app: TerminalApp,
        scripted: Scripted,
        terminal: Terminal<TestBackend>,
    }

    impl Harness {
        fn build(scripted: Scripted, notice: Option<&str>) -> Self {
            let mut harness = harness::Harness::new(scripted.services(fixture()));
            harness.start_session_with(1, Box::new(Rc::clone(&scripted.agent)));
            if let Some(text) = notice {
                harness = harness.with_notice(text);
            }
            scripted.files.wait();
            Self {
                app: TerminalApp::new(Client::open(harness).unwrap()),
                scripted,
                terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap(),
            }
        }

        fn done(&self) {
            self.reply(Reply::Done { usage: None });
        }

        fn footer(&mut self) -> String {
            self.screen().lines().last().unwrap().trim_end().to_owned()
        }

        fn new() -> Self {
            Self::with(ScriptedAgent::new(), None)
        }

        fn paste(&mut self, text: &str) {
            self.app.handle_event(Event::Paste(text.to_owned()));
            self.app.poll_events();
        }

        fn press(&mut self, code: KeyCode) {
            self.app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
            self.app.poll_events();
        }

        fn press_with_modifiers(&mut self, code: KeyCode, modifiers: KeyModifiers) {
            self.app.handle_event(Event::Key(KeyEvent::new(code, modifiers)));
            self.app.poll_events();
        }

        fn reply(&self, reply: Reply) {
            self.scripted.agent.reply(reply);
        }

        fn screen(&mut self) -> String {
            self.app.poll_events();
            self.terminal.draw(|frame| self.app.render(frame)).unwrap();
            let buffer = self.terminal.backend().buffer().clone();
            (0..HEIGHT)
                .map(|y| (0..WIDTH).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        fn send(&mut self, text: &str) {
            self.type_text(text);
            self.press(KeyCode::Enter);
        }

        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                self.press(KeyCode::Char(c));
            }
        }

        fn with(control: Rc<ScriptedAgent>, notice: Option<&str>) -> Self {
            Self::build(Scripted::new(control, &[]), notice)
        }

        fn with_agent(control: Rc<ScriptedAgent>) -> Self {
            Self::with(control, None)
        }
    }

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
        assert!(screen.contains("Enter: send") && screen.contains("Esc: cancel"), "{screen}");
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
        let mut h = Harness::with(
            ScriptedAgent::new(),
            Some("Could not read auth.json. Logins will not be saved this session."),
        );
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
        for (summary, result) in
            [("read_file(a)", ToolResult::ok("contents")), ("read_file(b)", ToolResult::error("nope"))]
        {
            h.reply(Reply::Event(AgentEvent::ToolStarted { id: summary.into(), summary: summary.into() }));
            h.reply(Reply::Event(AgentEvent::ToolFinished { id: summary.into(), summary: summary.into(), result }));
        }
        within(&mut h, "tool cards");
        h.type_text("/");
        within(&mut h, "picker");
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
        h.scripted
            .worker
            .event(LoginEvent::PromptSecret { message: "Paste your OpenAI API key (input is hidden)".into() });
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
        let mut h = Harness::build(Scripted::new(ScriptedAgent::new(), &[Provider::Copilot]), None);
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
}
