use crate::theme;
use crate::units::to_u16;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_textarea::{CursorMove, TextArea};

#[derive(Default)]
/// Mouse state of the input box.
pub(super) struct InputMouse {
    area: Rect,
    dragging: bool,
}

impl InputMouse {
    pub(super) const fn contains(&self, ev: MouseEvent) -> bool {
        self.area.contains(Position::new(ev.column, ev.row))
    }

    /// Handles a mouse event in the input box. Returns text to copy after a selection.
    pub(super) fn handle(&mut self, textarea: &mut TextArea<'_>, ev: MouseEvent) -> Option<String> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) if self.contains(ev) => {
                textarea.cancel_selection();
                self.jump_to(textarea, ev);
                textarea.start_selection();
                self.dragging = true;
            },
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.jump_to(textarea, ev);
            },
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                self.jump_to(textarea, ev);
                return match textarea.selection_range() {
                    // A plain click selects nothing: just keep the moved cursor.
                    Some((start, end)) if start != end => Some(selected_text(textarea, start, end)),
                    _ => {
                        textarea.cancel_selection();
                        None
                    },
                };
            },
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {},
        }
        None
    }

    /// Moves the cursor to where the mouse is.
    fn jump_to(&self, textarea: &mut TextArea<'_>, ev: MouseEvent) {
        let inner = theme::current().rule(true).inner(self.area);
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

    pub(super) const fn set_area(&mut self, area: Rect) {
        self.area = area;
    }
}

fn selected_text(textarea: &TextArea<'_>, start: (usize, usize), end: (usize, usize)) -> String {
    let lines = textarea.lines();
    (start.0..=end.0)
        .filter_map(|row| {
            let chars: Vec<char> = lines.get(row)?.chars().collect();
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 { end.1 } else { chars.len() };
            let selected = chars.get(from.min(chars.len())..to.min(chars.len())).unwrap_or_default();
            Some(selected.iter().collect::<String>())
        })
        .collect::<Vec<_>>()
        .join("\n")
}
