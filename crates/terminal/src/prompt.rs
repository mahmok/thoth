/// Mouse handling in the input box.
mod mouse;
/// Placeholders for pasted text.
mod paste;

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui_textarea::{CursorMove, TextArea};

use crate::theme;
use crate::units::to_u16;
use mouse::InputMouse;
use paste::PasteStore;

/// A prompt as shown and as sent.
pub struct Submission {
    pub full: String,
    pub shown: String,
}

/// The input box.
pub struct Prompt {
    mouse: InputMouse,
    pastes: PasteStore,
    textarea: TextArea<'static>,
}

impl Prompt {
    /// Where the cursor is, counted in characters from the start of the text.
    pub fn cursor(&self) -> usize {
        let ratatui_textarea::DataCursor(row, col) = self.textarea.cursor();
        let before: usize =
            self.textarea.lines().iter().take(row).map(|line| line.chars().count().saturating_add(1)).sum();
        before.saturating_add(col)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let _modified = self.textarea.input(key);
    }

    pub fn handle_mouse(&mut self, ev: MouseEvent) -> Option<String> {
        self.mouse.handle(&mut self.textarea, ev)
    }

    pub fn handle_paste(&mut self, pasted: &str) {
        self.pastes.insert(&mut self.textarea, pasted);
    }

    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_style(theme::current().text());
        textarea.set_cursor_line_style(Style::default());
        textarea.set_selection_style(theme::current().selected());
        Self { textarea, mouse: InputMouse::default(), pastes: PasteStore::default() }
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, title: &str) {
        let heading = Line::from(format!(" {title} "));
        self.textarea.set_block(theme::current().rule(true).title(heading));
        self.mouse.set_area(area);
        frame.render_widget(&self.textarea, area);
    }

    pub fn set_masked(&mut self, masked: bool) {
        if masked {
            self.textarea.set_mask_char('\u{2022}');
        } else {
            self.textarea.clear_mask_char();
        }
    }

    pub fn set_text(&mut self, text: &str, cursor: usize) {
        let _cleared = self.textarea.clear();
        let _inserted = self.textarea.insert_str(text);
        self.pastes.retain_present(text);
        let before: String = text.chars().take(cursor).collect();
        let row = before.matches('\n').count();
        let col = before.rsplit('\n').next().map_or(0, |line| line.chars().count());
        self.textarea.move_cursor(CursorMove::Jump(to_u16(row), to_u16(col)));
    }

    pub fn submission(&self) -> Option<Submission> {
        let shown = self.text();
        if shown.is_empty() {
            return None;
        }
        let full = self.pastes.expand(&shown);
        Some(Submission { full, shown })
    }

    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const WIDTH: u16 = 30;

    fn prompt() -> Prompt {
        let mut prompt = Prompt::new();
        draw(&mut prompt, "title");
        prompt
    }

    /// Draws the prompt in a 3-row box, as the app does, and returns its rows.
    fn draw(prompt: &mut Prompt, title: &str) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, 3)).unwrap();
        terminal.draw(|frame| prompt.render(frame, frame.area(), title)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..3).map(|y| (0..WIDTH).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>()).collect()
    }

    fn take(prompt: &mut Prompt) -> Option<Submission> {
        let submission = prompt.submission();
        prompt.set_text("", 0);
        submission
    }

    fn type_text(prompt: &mut Prompt, text: &str) {
        for c in text.chars() {
            prompt.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    fn left(kind: fn(MouseButton) -> MouseEventKind, column: u16) -> MouseEvent {
        mouse(kind(MouseButton::Left), column, 1)
    }

    #[test]
    fn typed_text_is_taken_and_the_input_is_left_empty() {
        let mut prompt = prompt();
        type_text(&mut prompt, "hello");
        let submission = take(&mut prompt).unwrap();
        assert_eq!((submission.shown.as_str(), submission.full.as_str()), ("hello", "hello"));
        assert!(take(&mut prompt).is_none());
    }

    #[test]
    fn a_masked_input_hides_what_is_typed_but_still_returns_it() {
        let mut prompt = prompt();
        prompt.set_masked(true);
        type_text(&mut prompt, "sk-secret");
        let shown = draw(&mut prompt, "t").join("\n");
        assert!(!shown.contains("sk-secret") && shown.contains("\u{2022}\u{2022}\u{2022}"), "{shown}");
        prompt.set_masked(false);
        assert_eq!(take(&mut prompt).unwrap().full, "sk-secret");
    }

    #[test]
    fn an_empty_input_has_nothing_to_take() {
        assert!(take(&mut prompt()).is_none());
    }

    #[test]
    fn a_large_paste_shows_a_placeholder_but_sends_everything() {
        let mut prompt = prompt();
        type_text(&mut prompt, "explain: ");
        prompt.handle_paste("a\r\nb\r\nc");
        let submission = take(&mut prompt).unwrap();
        assert_eq!(submission.shown, "explain: [Pasted 3 lines #1]");
        assert_eq!(submission.full, "explain: a\nb\nc");
    }

    #[test]
    fn the_cursor_is_counted_in_characters_across_lines() {
        let mut prompt = prompt();
        prompt.set_text("h\u{e9}llo\nw\u{f6}rld", 8);
        assert_eq!(prompt.cursor(), 8);
        type_text(&mut prompt, "X");
        assert_eq!(prompt.text(), "h\u{e9}llo\nw\u{f6}Xrld");
    }

    #[test]
    fn setting_text_puts_the_cursor_where_asked() {
        let mut prompt = prompt();
        prompt.set_text("abcdef", 2);
        type_text(&mut prompt, "-");
        assert_eq!(prompt.text(), "ab-cdef");
    }

    #[test]
    fn replacing_the_text_keeps_the_pastes_it_still_shows() {
        let mut prompt = prompt();
        prompt.handle_paste("a\nb\nc");
        let shown = format!("{} @src", prompt.text());
        prompt.set_text(&shown, shown.chars().count());
        let submission = prompt.submission().unwrap();
        assert_eq!(submission.full, "a\nb\nc @src");
    }

    #[test]
    fn a_short_paste_is_inserted_as_typed() {
        let mut prompt = prompt();
        prompt.handle_paste("one line\n");
        assert_eq!(take(&mut prompt).unwrap().shown, "one line");
    }

    #[test]
    fn clearing_the_prompt_resets_paste_numbering() {
        let mut prompt = prompt();
        prompt.handle_paste("a\nb");
        take(&mut prompt).unwrap();
        prompt.handle_paste("x\ny\nz");
        let submission = take(&mut prompt).unwrap();
        assert_eq!(submission.shown, "[Pasted 3 lines #1]");
        assert_eq!(submission.full, "x\ny\nz");
    }

    #[test]
    fn several_pastes_in_one_prompt_expand_independently() {
        let mut prompt = prompt();
        prompt.handle_paste("a\nb");
        type_text(&mut prompt, " and ");
        prompt.handle_paste("x\ny\nz");
        let submission = take(&mut prompt).unwrap();
        assert_eq!(submission.shown, "[Pasted 2 lines #1] and [Pasted 3 lines #2]");
        assert_eq!(submission.full, "a\nb and x\ny\nz");
    }

    #[test]
    fn clicking_moves_the_cursor() {
        let mut prompt = prompt();
        type_text(&mut prompt, "hello world");
        // The box has a border, so text column 0 is screen column 1.
        prompt.handle_mouse(left(MouseEventKind::Down, 1 + 3));
        assert_eq!(prompt.handle_mouse(left(MouseEventKind::Up, 1 + 3)), None);
        type_text(&mut prompt, "X");
        assert_eq!(take(&mut prompt).unwrap().shown, "helXlo world");
    }

    #[test]
    fn clicking_outside_the_box_does_nothing() {
        let mut prompt = prompt();
        type_text(&mut prompt, "hello");
        prompt.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 10));
        type_text(&mut prompt, "!");
        assert_eq!(take(&mut prompt).unwrap().shown, "hello!");
    }

    #[test]
    fn dragging_selects_and_returns_the_text() {
        let mut prompt = prompt();
        type_text(&mut prompt, "hello world");
        prompt.handle_mouse(left(MouseEventKind::Down, 1));
        prompt.handle_mouse(left(MouseEventKind::Drag, 4));
        let copied = prompt.handle_mouse(left(MouseEventKind::Up, 6));
        assert_eq!(copied.as_deref(), Some("hello"));
    }

    #[test]
    fn the_title_is_drawn_in_the_border() {
        let mut prompt = Prompt::new();
        let rows = draw(&mut prompt, "Enter: send");
        assert!(rows[0].contains("Enter: send"), "{rows:?}");
    }

    #[test]
    fn typed_text_is_drawn() {
        let mut prompt = prompt();
        type_text(&mut prompt, "hi there");
        assert!(draw(&mut prompt, "t")[1].contains("hi there"));
    }
}
