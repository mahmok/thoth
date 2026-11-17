//! The shared test harness: a headless app driven by keys, a screen drawn into a test backend,
//! and the fixtures the widget tests draw.

use alloc::rc::Rc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use thoth_core::domain::shared::SessionId;
use thoth_core::harness::Harness as CoreHarness;
use thoth_core::models::testing::fixture;
use thoth_core::runtime::turn_runner::Reply;
use thoth_core::runtime::turn_runner::scripted::ScriptedTurnRunner;
use thoth_core::test::scripted::Scripted;

use crate::app::TerminalApp;
use crate::client::TerminalClient;

/// The width the app is drawn at, wide enough for a full footer and a wrapped chat.
pub const WIDTH: u16 = 100;
/// The height the app is drawn at: header, chat, a three-line input and the footer.
pub const HEIGHT: u16 = 16;

/// A headless app: press keys, draw the screen, assert what a user would see.
pub struct Harness {
    pub app: TerminalApp,
    pub scripted: Scripted,
    pub terminal: Terminal<TestBackend>,
    /// The app's clock, in milliseconds. Nothing here sleeps: tests move it with [`advance`].
    ///
    /// [`advance`]: Harness::advance
    pub now: u64,
}

impl Harness {
    pub fn build(scripted: Scripted, notice: Option<&str>) -> Self {
        let mut harness = CoreHarness::new(scripted.services(fixture()));
        harness.start_session_with(SessionId::mint(), Box::new(Rc::clone(&scripted.agent)));
        if let Some(text) = notice {
            harness = harness.with_notice(text);
        }
        scripted.files.wait();
        Self {
            app: TerminalApp::new(TerminalClient::open(harness).unwrap()),
            scripted,
            terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap(),
            now: 0,
        }
    }

    /// Moves the app's clock forward, so a timed toast can expire without sleeping.
    pub fn advance(&mut self, millis: u64) {
        self.now = self.now.saturating_add(millis);
    }

    pub fn done(&self) {
        self.reply(Reply::Done { usage: None });
    }

    pub fn footer(&mut self) -> String {
        self.screen().lines().last().unwrap().trim_end().to_owned()
    }

    pub fn new() -> Self {
        Self::with(ScriptedTurnRunner::new(), None)
    }

    pub fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
        self.app.handle_event(Event::Mouse(event), self.now);
        let _changed = self.app.poll_events();
    }

    pub fn paste(&mut self, text: &str) {
        self.app.handle_event(Event::Paste(text.to_owned()), self.now);
        let _changed = self.app.poll_events();
    }

    pub fn press(&mut self, code: KeyCode) {
        self.app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), self.now);
        let _changed = self.app.poll_events();
    }

    pub fn press_with_modifiers(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.app.handle_event(Event::Key(KeyEvent::new(code, modifiers)), self.now);
        let _changed = self.app.poll_events();
    }

    pub fn reply(&self, reply: Reply) {
        self.scripted.agent.reply(reply);
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    }

    pub fn screen(&mut self) -> String {
        let _changed = self.app.poll_events();
        self.terminal.draw(|frame| self.app.render(frame, self.now)).unwrap();
        buffer_text(&self.terminal)
    }

    pub fn send(&mut self, text: &str) {
        self.type_text(text);
        self.press(KeyCode::Enter);
    }

    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(KeyCode::Char(c));
        }
    }

    pub fn with(control: Rc<ScriptedTurnRunner>, notice: Option<&str>) -> Self {
        Self::build(Scripted::new(control, &[]), notice)
    }

    pub fn with_agent(control: Rc<ScriptedTurnRunner>) -> Self {
        Self::with(control, None)
    }
}

/// Draws one widget into a test backend and returns the screen as text.
pub fn screen(width: u16, height: u16, draw: impl FnOnce(&mut ratatui::Frame<'_>)) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    buffer_text(&terminal)
}

/// The visible characters of the last frame, one line per row.
fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let area = terminal.size().unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..area.height)
        .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Three commands with different descriptions and idle needs.
pub fn fruit_commands() -> Vec<crate::command::Command> {
    let command = |id: &str, description: &str, needs_idle| crate::command::Command {
        id: id.into(),
        description: description.into(),
        needs_idle,
    };
    vec![command("apple", "red", false), command("banana", "yellow", true), command("grape", "purple", false)]
}

/// The choices of a `fruit` command.
pub fn fruit_choices() -> crate::command::Choices {
    crate::command::Choices {
        command: "fruit".into(),
        title: "Fruit".into(),
        items: vec![
            crate::command::Choice::new("a", "apple", "red"),
            crate::command::Choice::new("b", "banana", "yellow").needing_idle(),
            crate::command::Choice::new("g", "grape", "purple"),
        ],
        empty_message: "no fruit".into(),
    }
}

/// More choices than any popup shows at once.
pub fn many_choices() -> crate::command::Choices {
    crate::command::Choices {
        command: "many".into(),
        title: "Many".into(),
        items: (0..20).map(|i| crate::command::Choice::new(i.to_string(), format!("item{i:02}"), "")).collect(),
        empty_message: String::new(),
    }
}

/// Two file suggestions.
pub fn paths() -> thoth_core::builtin::files::Suggestions {
    let path = |p: &str| thoth_core::builtin::files::Suggestion {
        label: p.into(),
        description: String::new(),
        partial: format!("@{p}"),
        full: format!("@{p} "),
    };
    thoth_core::builtin::files::Suggestions {
        title: "Paths".into(),
        items: vec![path("a.rs"), path("b.rs")],
        empty_message: "no files".into(),
    }
}
