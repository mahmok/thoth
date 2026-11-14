use crate::picker::PickerView;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::theme::{self, Status, palette};
use crate::units::to_u16;

/// Rows a popup shows at once.
const MAX_ROWS: usize = 8;

/// A picker popup. Keeps the window it showed last, so the list scrolls once the selection
/// reaches an edge rather than on every move.
#[derive(Default)]
pub struct Popup {
    first_row: usize,
}

impl Popup {
    /// Draws a picker above `anchor`, as many rows as fit between it and `ceiling`.
    pub fn render(&mut self, frame: &mut Frame<'_>, picker: &PickerView, anchor: Rect, ceiling: Rect, busy: bool) {
        let room = anchor.y.saturating_sub(ceiling.y);
        let height = to_u16(picker.len().clamp(1, MAX_ROWS)).saturating_add(1).min(room);
        if height < 2 {
            return;
        }
        let area = Rect::new(anchor.x, anchor.y.saturating_sub(height), anchor.width, height);
        let theme = theme::current();
        let block = theme
            .rule_on(true, palette::ELEVATED)
            .title(Span::styled(format!(" {} ", picker.title), theme.section_title()));

        frame.render_widget(Clear, area);
        if picker.is_empty() {
            let empty = Paragraph::new(picker.empty_message.as_str()).style(theme::current().muted()).block(block);
            frame.render_widget(empty, area);
            return;
        }

        let capacity = usize::from(height.saturating_sub(1));
        self.keep_in_view(picker, capacity);
        let entries: Vec<ListItem<'_>> = picker
            .rows()
            .skip(self.first_row)
            .take(capacity)
            .map(|item| {
                let blocked = busy && item.needs_idle;
                let (name_style, note) = if blocked {
                    (theme::current().status(Status::Waiting), format!("{} (busy)", item.description))
                } else {
                    (theme::current().text(), item.description.clone())
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}{}", picker.prefix, item.label), name_style),
                    Span::raw("  "),
                    Span::styled(note, theme::current().muted()),
                ]))
            })
            .collect();
        let list = List::new(entries).block(block).highlight_style(theme::current().selected());
        let mut shown =
            ListState::default().with_selected(picker.selected.map(|selected| selected.saturating_sub(self.first_row)));
        frame.render_stateful_widget(list, area, &mut shown);
    }

    /// Forgets the window, as when there is no picker to show.
    pub const fn reset(&mut self) {
        self.first_row = 0;
    }

    /// Moves the window only as far as it must to keep the selection in view.
    fn keep_in_view(&mut self, picker: &PickerView<'_>, capacity: usize) {
        let Some(selected) = picker.selected else {
            self.first_row = 0;
            return;
        };
        let last_start = picker.len().saturating_sub(capacity);
        let mut first = self.first_row.min(last_start);
        if selected < first {
            first = selected;
        } else if selected >= first.saturating_add(capacity) {
            first = selected.saturating_add(1).saturating_sub(capacity);
        } else {
            // The selection is already in view.
        }
        self.first_row = first;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::Picker;
    use crate::testing::{self, fruit_commands, many_choices};

    fn fruit() -> Picker {
        Picker::commands(fruit_commands())
    }

    fn draw(popup: &mut Popup, picker: &Picker, busy: bool, anchor_y: u16) -> String {
        testing::screen(30, 10, |frame| {
            popup.render(frame, &picker.view(), Rect::new(0, anchor_y, 30, 3), Rect::new(0, 0, 30, anchor_y), busy);
        })
    }

    fn screen(picker: &Picker, busy: bool, anchor_y: u16) -> String {
        draw(&mut Popup::default(), picker, busy, anchor_y)
    }

    #[test]
    fn renders_a_popup_above_the_anchor() {
        let shown = screen(&fruit(), false, 7);
        assert!(shown.contains("Commands"), "{shown}");
        assert!(shown.contains("/apple  red") && shown.contains("/grape  purple"), "{shown}");
    }

    #[test]
    fn busy_items_are_labelled() {
        let shown = screen(&fruit(), true, 7);
        assert!(shown.contains("yellow (busy)"), "{shown}");
        assert!(!shown.contains("red (busy)"), "{shown}");
    }

    #[test]
    fn an_empty_result_says_so() {
        let mut picker = fruit();
        picker.set_text("/zzz");
        assert!(screen(&picker, false, 7).contains("no matches"));
    }

    #[test]
    fn with_little_room_the_selected_row_stays_in_view() {
        let mut picker = fruit();
        picker.move_selection(2);
        let shown = screen(&picker, false, 3);
        assert!(shown.contains("grape") && !shown.contains("apple"), "{shown}");
    }

    #[test]
    fn with_more_rows_than_fit_the_first_row_is_shown_first() {
        let picker = Picker::choices(many_choices());
        let shown = screen(&picker, false, 6);
        assert!(shown.contains("item00"), "{shown}");
        assert!(!shown.contains("item05"), "only five rows fit: {shown}");
    }

    #[test]
    fn with_more_rows_than_fit_the_last_row_scrolls_into_view() {
        let mut picker = Picker::choices(many_choices());
        picker.move_selection(-1);
        let shown = screen(&picker, false, 6);
        assert!(shown.contains("item19"), "{shown}");
        assert!(!shown.contains("item00"), "{shown}");
    }

    #[test]
    fn moving_up_keeps_the_window_until_the_selection_reaches_its_top() {
        let mut picker = Picker::choices(many_choices());
        let mut popup = Popup::default();
        picker.move_selection(5);
        let down = draw(&mut popup, &picker, false, 6);
        assert!(down.contains("item01") && down.contains("item05") && !down.contains("item00"), "{down}");

        picker.move_selection(3);
        let lower = draw(&mut popup, &picker, false, 6);
        assert!(lower.contains("item04") && lower.contains("item08") && !lower.contains("item03"), "{lower}");

        picker.move_selection(-2);
        let inside = draw(&mut popup, &picker, false, 6);
        assert!(inside.contains("item04") && !inside.contains("item03"), "still the same window: {inside}");

        picker.move_selection(-3);
        let top = draw(&mut popup, &picker, false, 6);
        assert!(top.contains("item03") && !top.contains("item02"), "{top}");
    }
}
