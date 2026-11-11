use crate::picker::PickerView;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::theme::{self, Status, palette};
use crate::units::to_u16;

/// Draws a picker above `anchor`, as many rows as fit between it and `ceiling`.
pub fn render(frame: &mut Frame<'_>, picker: &PickerView, anchor: Rect, ceiling: Rect, busy: bool) {
    let room = anchor.y.saturating_sub(ceiling.y);
    let height = to_u16(picker.rows.len().max(1)).saturating_add(1).min(room);
    if height < 2 {
        return;
    }
    let area = Rect::new(anchor.x, anchor.y.saturating_sub(height), anchor.width, height);
    let theme = theme::current();
    let block = theme
        .rule_on(true, palette::ELEVATED)
        .title(Span::styled(format!(" {} ", picker.title), theme.section_title()));

    frame.render_widget(Clear, area);
    if picker.rows.is_empty() {
        let empty = Paragraph::new(picker.empty_message.as_str()).style(theme::current().muted()).block(block);
        frame.render_widget(empty, area);
        return;
    }

    let capacity = usize::from(height.saturating_sub(1));
    let skipped = picker.selected.map_or(0, |selected| selected.saturating_add(1).saturating_sub(capacity));
    let entries: Vec<ListItem<'_>> = picker
        .rows
        .iter()
        .skip(skipped)
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
    let mut shown = ListState::default().with_selected(picker.selected.map(|s| s.saturating_sub(skipped)));
    frame.render_stateful_widget(list, area, &mut shown);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::picker::Picker;
    use crate::picker::testing::fruit_commands;

    fn fruit() -> Picker {
        Picker::commands(fruit_commands())
    }

    fn screen(picker: &Picker, busy: bool, anchor_y: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &picker.view(), Rect::new(0, anchor_y, 30, 3), Rect::new(0, 0, 30, anchor_y), busy);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..10)
            .map(|y| (0..30).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
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
}
