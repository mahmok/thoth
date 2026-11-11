//! The one-row header: a tiny ibis glyph and the name on the left, what the agent is doing on
//! the right. The branding is small so the task stays the centre of the screen.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::{self, Status, Theme, palette};
use crate::units::to_u16;

/// The glyph in profile: a teal crest and curved beak around a gold eye.
fn mark(theme: Theme) -> Vec<Span<'static>> {
    let teal = theme.accent();
    vec![Span::styled("\u{256d}", teal), Span::styled("\u{25c6}", theme.gold()), Span::styled("\u{256e}", teal)]
}

fn brand(theme: Theme) -> Line<'static> {
    let mut spans = mark(theme);
    spans.push(Span::styled(" THOTH", theme.fg(palette::TEAL_BRIGHT).add_modifier(Modifier::BOLD)));
    Line::from(spans)
}

fn state(theme: Theme, status: Status) -> Line<'static> {
    let word = match status {
        Status::Running => "running",
        Status::Waiting => "waiting",
        Status::Idle => "idle",
        Status::Completed => "done",
        Status::Error => "error",
    };
    Line::from(Span::styled(format!("● {word}"), theme.status(status)))
}

pub fn render(frame: &mut Frame<'_>, area: Rect, status: Status) {
    let theme = theme::current();
    theme.fill_on(frame, area, palette::SURFACE);
    let state = state(*theme, status);
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(to_u16(state.width()).saturating_add(1))])
            .areas(Theme::pane().inner(area));
    frame.render_widget(Paragraph::new(brand(*theme)), left);
    frame.render_widget(Paragraph::new(state), right);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::theme::ColorDepth;

    fn row(status: Status) -> String {
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        terminal.draw(|frame| render(frame, frame.area(), status)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..40).map(|x| buffer[(x, 0)].symbol()).collect()
    }

    #[test]
    fn the_name_is_on_the_left_and_the_state_on_the_right() {
        let shown = row(Status::Running);
        assert!(shown.trim_start().starts_with("\u{256d}\u{25c6}\u{256e} THOTH"), "{shown:?}");
        assert!(shown.trim_end().ends_with("\u{25cf} running"), "{shown:?}");
    }

    #[test]
    fn each_state_is_named_in_words_and_not_by_colour_alone() {
        assert!(row(Status::Waiting).contains("\u{25cf} waiting"));
        assert!(row(Status::Idle).contains("\u{25cf} idle"));
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
