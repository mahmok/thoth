use crate::drag::{Drag, Dragged};
use crate::select;
use crate::theme;
use crate::units::to_u16;
use crossterm::event::MouseEvent;
use ratatui::layout::{Position, Rect};
use ratatui_textarea::{CursorMove, TextArea};

#[derive(Default)]
/// Mouse state of the input box.
pub(super) struct InputMouse {
    area: Rect,
    drag: Drag,
}

impl InputMouse {
    /// Handles a mouse event in the input box. Returns text to copy after a selection.
    pub(super) fn handle(&mut self, textarea: &mut TextArea<'_>, ev: MouseEvent) -> Option<String> {
        let mut target = TextMouse { area: self.area, textarea };
        self.drag.handle(ev, &mut target).flatten()
    }

    pub(super) const fn set_area(&mut self, area: Rect) {
        self.area = area;
    }
}

/// The input box as a drag target.
struct TextMouse<'a, 'b> {
    area: Rect,
    textarea: &'a mut TextArea<'b>,
}

impl Dragged for TextMouse<'_, '_> {
    type Output = Option<String>;

    fn contains(&self, ev: MouseEvent) -> bool {
        self.area.contains(Position::new(ev.column, ev.row))
    }

    fn pressed(&mut self, ev: MouseEvent) {
        self.textarea.cancel_selection();
        jump_to(self.area, self.textarea, ev);
        self.textarea.start_selection();
    }

    fn moved(&mut self, ev: MouseEvent) {
        jump_to(self.area, self.textarea, ev);
    }

    fn released(&mut self, ev: MouseEvent) -> Option<String> {
        jump_to(self.area, self.textarea, ev);
        match self.textarea.selection_range() {
            // A plain click selects nothing: just keep the moved cursor.
            Some((start, end)) if start != end => Some(selected_text(self.textarea, start, end)),
            _ => {
                self.textarea.cancel_selection();
                None
            },
        }
    }
}

/// Moves the cursor to where the mouse is.
fn jump_to(area: Rect, textarea: &mut TextArea<'_>, ev: MouseEvent) {
    let inner = theme::current().rule(true).inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let x = ev.column.clamp(inner.x, inner.right().saturating_sub(1));
    let y = ev.row.clamp(inner.y, inner.bottom().saturating_sub(1));

    let (scroll_row, scroll_col) = textarea.scroll_offset();
    let row = usize::from(y.saturating_sub(inner.y)).saturating_add(usize::from(scroll_row));
    let col = usize::from(x.saturating_sub(inner.x))
        .saturating_add(usize::from(scroll_col))
        .saturating_sub(usize::from(textarea.line_number_width()));

    let target = textarea.screen_to_data(row, col);
    textarea.move_cursor(CursorMove::Jump(to_u16(target.0), to_u16(target.1)));
}

fn selected_text(textarea: &TextArea<'_>, start: (usize, usize), end: (usize, usize)) -> String {
    // The textarea's end is where the selection stops, so it is one past the last character.
    let last = match end.1 {
        0 => (end.0.saturating_sub(1), usize::MAX),
        column => (end.0, column.saturating_sub(1)),
    };
    let lines = textarea.lines();
    let rows = lines.iter().skip(start.0).take(end.0.saturating_sub(start.0).saturating_add(1));
    select::extract(start, last, rows.map(String::as_str))
}
