//! Toast: one short-lived message the app draws over the screen, such as a confirmation
//! that something was copied. It is not a transcript Notice: nothing is recorded.

use core::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr as _;

use crate::theme::{self, palette};
use crate::units::to_u16;

/// How long the toast for a copy stays up.
pub const COPIED: Duration = Duration::from_secs(1);

/// The toast slot. Showing one replaces whatever is showing.
#[derive(Debug, Default)]
pub struct Toast {
    shown: Option<Shown>,
}

#[derive(Debug)]
struct Shown {
    text: String,
    until: u64,
}

impl Toast {
    /// Shows `text` until `timeout` has passed since `now`, in milliseconds.
    pub fn show(&mut self, text: impl Into<String>, timeout: Duration, now: u64) {
        self.shown = Some(Shown { text: text.into(), until: now.saturating_add(to_millis(timeout)) });
    }

    /// Takes the toast away once its time is up. Returns whether it took one away, so the
    /// caller knows the screen has to be redrawn to clear it.
    pub fn expire(&mut self, now: u64) -> bool {
        let expired = self.shown.as_ref().is_some_and(|shown| now >= shown.until);
        if expired {
            self.shown = None;
        }
        expired
    }

    /// What the toast says at `now`, if it is still up.
    pub fn text(&self, now: u64) -> Option<&str> {
        self.shown.as_ref().filter(|shown| now < shown.until).map(|shown| shown.text.as_str())
    }

    /// Draws the toast in the top-right corner of `screen`, on the row `top`. The row is inset
    /// one column from the edge so it never covers the chat's scrollbar.
    pub fn render(&self, frame: &mut Frame<'_>, screen: Rect, top: u16, now: u64) {
        let Some(text) = self.text(now) else {
            return;
        };
        // A space either side keeps the text off the edges of its surface.
        let width = to_u16(text.width().saturating_add(2)).min(screen.width);
        let area = Rect::new(screen.right().saturating_sub(width).saturating_sub(1), top, width, 1);
        if area.width == 0 || area.height == 0 || area.bottom() > screen.bottom() {
            return;
        }
        let theme = theme::current();
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(format!(" {text}")).style(theme.text().patch(theme.on(palette::ELEVATED))),
            area,
        );
    }
}

fn to_millis(timeout: Duration) -> u64 {
    u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn shown(timeout: Duration, now: u64) -> Toast {
        let mut toast = Toast::default();
        toast.show("copied", timeout, now);
        toast
    }

    fn rows(toast: &Toast, now: u64) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(30, 4)).unwrap();
        terminal.draw(|frame| toast.render(frame, frame.area(), 1, now)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..4).map(|y| (0..30).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>()).collect()
    }

    #[test]
    fn a_toast_stays_up_for_its_timeout() {
        let toast = shown(Duration::from_secs(1), 1_000);
        assert_eq!(toast.text(1_000), Some("copied"));
        assert_eq!(toast.text(1_999), Some("copied"));
        assert_eq!(toast.text(2_000), None);
    }

    #[test]
    fn the_timeout_is_settable() {
        let toast = shown(Duration::from_secs(5), 0);
        assert_eq!(toast.text(4_999), Some("copied"));
        assert_eq!(toast.text(5_000), None);
    }

    #[test]
    fn a_new_toast_replaces_the_one_showing() {
        let mut toast = shown(Duration::from_secs(1), 0);
        toast.show("saved", Duration::from_secs(10), 500);
        assert_eq!(toast.text(500), Some("saved"));
        assert_eq!(toast.text(1_000), Some("saved"));
    }

    #[test]
    fn expiring_takes_the_toast_away_once_and_says_so() {
        let mut toast = shown(Duration::from_secs(1), 0);
        assert!(!toast.expire(999));
        assert_eq!(toast.text(0), Some("copied"), "it is still up");
        assert!(toast.expire(1_000));
        assert_eq!(toast.text(0), None);
        assert!(!toast.expire(1_001));
    }

    #[test]
    fn a_toast_is_drawn_in_the_top_right_corner() {
        let rows = rows(&shown(Duration::from_secs(1), 0), 0);
        assert!(rows[0].trim().is_empty(), "the header row is left alone: {rows:?}");
        assert!(rows[2].trim().is_empty() && rows[3].trim().is_empty(), "one row tall: {rows:?}");
        let start = rows[1].find(" copied ").expect("the toast is drawn");
        assert_eq!(start, 21, "one column in from the right edge: {rows:?}");
    }

    #[test]
    fn an_expired_toast_is_not_drawn() {
        let rows = rows(&shown(Duration::from_secs(1), 0), 1_000);
        assert!(!rows[1].contains("copied"), "{rows:?}");
    }
}
