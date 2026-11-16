use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::{self, Status, Theme, palette};
use crate::units::to_u16;

fn mark(theme: Theme) -> Vec<Span<'static>> {
    let teal = theme.accent();
    vec![Span::styled("──", teal), Span::styled("●", theme.gold()), Span::styled("──", teal)]
}

fn brand(theme: Theme) -> Line<'static> {
    let mut spans = mark(theme);
    spans.push(Span::styled(" THOTH", theme.fg(palette::TEAL_BRIGHT).add_modifier(Modifier::BOLD)));
    Line::from(spans)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// What the header can say the agent is doing. The tool-card states are not here: the app cannot
/// produce them for the header, so they are not arms that can never run.
pub enum State {
    Idle,
    Running,
    Waiting,
}

fn state_line(theme: Theme, state: State) -> Line<'static> {
    let (word, status) = match state {
        State::Idle => ("idle", Status::Idle),
        State::Running => ("running", Status::Running),
        State::Waiting => ("waiting", Status::Waiting),
    };
    Line::from(Span::styled(format!("● {word}"), theme.status(status)))
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: State) {
    let theme = theme::current();
    theme.fill_on(frame, area, palette::SURFACE);
    let shown = state_line(*theme, state);
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(to_u16(shown.width()).saturating_add(1))])
            .areas(Theme::pane().inner(area));
    frame.render_widget(Paragraph::new(brand(*theme)), left);
    frame.render_widget(Paragraph::new(shown), right);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::theme::ColorDepth;

    fn row(state: State) -> String {
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        terminal.draw(|frame| render(frame, frame.area(), state)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..40).map(|x| buffer[(x, 0)].symbol()).collect()
    }

    #[test]
    fn the_name_is_on_the_left_and_the_state_on_the_right() {
        let shown = row(State::Running);
        assert!(shown.trim_start().starts_with("\u{2500}\u{2500}\u{25cf}\u{2500}\u{2500} THOTH"), "{shown:?}");
        assert!(shown.trim_end().ends_with("\u{25cf} running"), "{shown:?}");
    }

    #[test]
    fn each_state_is_named_in_words_and_not_by_colour_alone() {
        assert!(row(State::Waiting).contains("\u{25cf} waiting"));
        assert!(row(State::Idle).contains("\u{25cf} idle"));
    }

    #[test]
    fn every_state_reads_differently() {
        let shown: Vec<String> = [State::Idle, State::Running, State::Waiting]
            .into_iter()
            .map(|state| row(state).trim_end().to_owned())
            .collect();
        assert_ne!(shown[0], shown[1]);
        assert_ne!(shown[1], shown[2]);
        assert_ne!(shown[0], shown[2]);
    }

    #[test]
    fn the_eye_is_gold_and_the_rest_of_the_glyph_teal() {
        let theme = Theme::new(ColorDepth::Truecolor);
        let spans = mark(theme);
        assert_eq!(spans[0].style.fg, theme.color(palette::TEAL));
        assert_eq!(spans[1].style.fg, theme.color(palette::GOLD));
        assert_eq!(spans[2].style.fg, theme.color(palette::TEAL));
    }
}
