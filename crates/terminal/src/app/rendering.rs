//! Rendering: the whole screen, assembled from the core view. Pull-based, so what is on screen
//! has one source of truth.

use super::TerminalApp;
use crate::header;
use crate::keymap::{self, Context};
use crate::stats;
use crate::theme::{self, Theme};
use crate::units::to_u16;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;
use thoth_core::llm::provider::ModelSpec;
use unicode_width::UnicodeWidthStr as _;

impl TerminalApp {
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
        let model = format!("{model_name} \u{b7} thinking {}", settings.thinking.label());
        let [stats_area, model_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(to_u16(model.width()).saturating_add(1))])
                .areas(Theme::pane().inner(footer_area));
        frame.render_widget(
            Paragraph::new(stats::summary(&self.harness.view().stats)).style(theme::current().muted()),
            stats_area,
        );
        frame.render_widget(Paragraph::new(model).right_aligned().style(theme::current().secondary()), model_area);

        if let Some(picker) = self.pickers.view() {
            self.popup.render(frame, &picker, input_area, chat_area, self.harness.view().busy);
        } else {
            self.popup.reset();
        }
    }

    /// What the header says the agent is doing: a login that needs the user is waiting on them.
    fn status(&self) -> header::State {
        let login = self.harness.login_view();
        if login.active || login.awaiting_secret {
            header::State::Waiting
        } else if self.harness.view().busy {
            header::State::Running
        } else {
            header::State::Idle
        }
    }

    fn title(&self) -> String {
        let context = self.context();
        if context == Context::Always && self.harness.view().busy {
            "Waiting for response...".to_owned()
        } else {
            keymap::hints(context)
        }
    }
}
