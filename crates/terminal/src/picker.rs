/// Picker state shared with the prompt.
mod host;

use crate::command::{Choices, Command};
use thoth_core::builtin::files::Suggestions;

pub use host::{Catalog, Outcome, PickerHost, PickerKey, PromptText};

/// Rows a picker shows at once.
pub const VISIBLE_ROWS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
/// A row of the picker.
pub struct Row {
    pub description: String,
    pub label: String,
    pub needs_idle: bool,
}

/// What the popup draws: a window of the matching rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerView {
    pub empty_message: String,
    /// Drawn before each label, such as the `/` of commands.
    pub prefix: &'static str,
    pub rows: Vec<Row>,
    pub selected: Option<usize>,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Whether a picker edits the prompt or a mention.
pub enum Scope {
    /// Completing replaces the trigger word before the cursor.
    Mention,
    /// Completing replaces the whole prompt.
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Text a picker completes to.
pub struct Completion {
    pub scope: Scope,
    pub text: String,
}

/// What a picker lists.
enum Kind {
    /// What a Command can be run with, such as the models. Holds each Choice's key.
    Choices { command: String, keys: Vec<String> },
    /// The Commands, open while the prompt is `/` and a word. Holds each one's id.
    Commands(Vec<String>),
    /// Suggestions for the trigger word being typed, with what each completes to.
    Suggestions(Vec<(String, String)>),
}

/// A list the user filters and picks from.
pub struct Picker {
    empty_message: String,
    first_row: usize,
    items: Vec<Row>,
    kind: Kind,
    query: String,
    selected: Option<usize>,
    title: String,
    visible: Vec<usize>,
}

impl Picker {
    pub fn choices(choices: Choices) -> Self {
        let (rows, keys) = choices
            .items
            .into_iter()
            .map(|c| {
                let row = Row { label: c.label, description: c.description, needs_idle: c.needs_idle };
                (row, c.key)
            })
            .unzip();
        let kind = Kind::Choices { command: choices.command, keys };
        Self::new(kind, choices.title, rows, choices.empty_message)
    }

    pub fn commands(commands: Vec<Command>) -> Self {
        let (rows, ids) = commands
            .into_iter()
            .map(|c| {
                let row = Row { label: c.id.clone(), description: c.description, needs_idle: c.needs_idle };
                (row, c.id)
            })
            .unzip();
        Self::new(Kind::Commands(ids), "Commands".to_owned(), rows, "no commands".to_owned())
    }

    /// The text the highlighted row completes to; `finish` when the user settles on it.
    pub fn completion(&self, finish: bool) -> Option<Completion> {
        let index = self.highlighted()?;
        let label = &self.items.get(index)?.label;
        let text = match self.kind {
            Kind::Commands(_) => format!("/{label}"),
            Kind::Choices { .. } => label.clone(),
            Kind::Suggestions(ref completions) => {
                let completion = completions.get(index)?;
                if finish { completion.1.clone() } else { completion.0.clone() }
            },
        };
        Some(Completion { text, scope: self.scope() })
    }

    pub const fn has_matches(&self) -> bool {
        !self.visible.is_empty()
    }

    fn highlighted(&self) -> Option<usize> {
        self.visible.get(self.selected?).copied()
    }

    /// The Commands, as opposed to a list opened by running one.
    pub const fn is_root(&self) -> bool {
        matches!(self.kind, Kind::Commands(_))
    }

    fn keep_in_view(&mut self, selected: usize) {
        let last_start = self.visible.len().saturating_sub(VISIBLE_ROWS);
        let mut first = self.first_row.min(last_start);
        if selected < first {
            first = selected;
        } else if selected >= first.saturating_add(VISIBLE_ROWS) {
            first = selected.saturating_add(1).saturating_sub(VISIBLE_ROWS);
        } else {
            // The selection is already in view.
        }
        self.first_row = first;
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = isize::try_from(self.visible.len()).unwrap_or(isize::MAX);
        if count == 0 {
            return;
        }
        let current = isize::try_from(self.selected.unwrap_or(0)).unwrap_or(isize::MAX);
        let selected = usize::try_from(current.saturating_add(delta).rem_euclid(count)).unwrap_or(0);
        self.selected = Some(selected);
        self.keep_in_view(selected);
    }

    fn new(kind: Kind, title: String, items: Vec<Row>, empty_message: String) -> Self {
        let mut picker = Self {
            kind,
            title,
            items,
            empty_message,
            query: String::new(),
            visible: Vec::new(),
            selected: None,
            first_row: 0,
        };
        picker.refilter();
        picker
    }

    const fn prefix(&self) -> &'static str {
        match self.kind {
            Kind::Commands(_) => "/",
            Kind::Choices { .. } | Kind::Suggestions(_) => "",
        }
    }

    fn refilter(&mut self) {
        let query = self.query.to_lowercase();
        self.visible = self
            .items
            .iter()
            .enumerate()
            .filter(|&(_, item)| item.label.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        self.selected = (!self.visible.is_empty()).then_some(0);
        self.first_row = 0;
    }

    pub const fn scope(&self) -> Scope {
        match self.kind {
            Kind::Suggestions(_) => Scope::Mention,
            Kind::Commands(_) | Kind::Choices { .. } => Scope::Prompt,
        }
    }

    /// The Command to run and the Choice to run it with, unless it needs an idle agent and
    /// one is busy.
    pub fn select(&self, busy: bool) -> Option<(String, Option<String>)> {
        let index = self.highlighted()?;
        if busy && self.items.get(index)?.needs_idle {
            return None;
        }
        match self.kind {
            Kind::Commands(ref ids) => Some((ids.get(index)?.clone(), None)),
            Kind::Choices { ref command, ref keys } => Some((command.clone(), Some(keys.get(index)?.clone()))),
            Kind::Suggestions(_) => None,
        }
    }

    /// Filters by `text`, and says whether the picker still belongs to it.
    pub fn set_text(&mut self, text: &str) -> bool {
        let query = match self.prefix() {
            "" => text.trim(),
            prefix => match text.strip_prefix(prefix) {
                Some(rest) if !rest.contains(char::is_whitespace) => rest,
                _ => return false,
            },
        };
        if query != self.query {
            self.query = query.to_owned();
            self.refilter();
        }
        true
    }

    pub fn suggestions(suggestions: Suggestions) -> Self {
        let (items, completions) = suggestions
            .items
            .into_iter()
            .map(|s| {
                let row = Row { label: s.label, description: s.description, needs_idle: false };
                (row, (s.partial, s.full))
            })
            .unzip();
        Self::new(Kind::Suggestions(completions), suggestions.title, items, suggestions.empty_message)
    }

    /// The rows to show, with the selection relative to them.
    pub fn view(&self) -> PickerView {
        let end = self.first_row.saturating_add(VISIBLE_ROWS).min(self.visible.len());
        PickerView {
            title: self.title.clone(),
            prefix: self.prefix(),
            rows: self
                .visible
                .get(self.first_row..end)
                .unwrap_or_default()
                .iter()
                .filter_map(|&index| self.items.get(index))
                .cloned()
                .collect(),
            selected: self.selected.map(|selected| selected.saturating_sub(self.first_row)),
            empty_message: if self.items.is_empty() { self.empty_message.clone() } else { "no matches".to_owned() },
        }
    }
}

#[cfg(test)]
pub mod testing {
    use crate::command::{Choice, Choices, Command};
    use thoth_core::builtin::files::{Suggestion, Suggestions};

    pub fn fruit_commands() -> Vec<Command> {
        let command = |id: &str, description: &str, needs_idle| Command {
            id: id.into(),
            description: description.into(),
            needs_idle,
        };
        vec![command("apple", "red", false), command("banana", "yellow", true), command("grape", "purple", false)]
    }

    pub fn fruit_choices() -> Choices {
        Choices {
            command: "fruit".into(),
            title: "Fruit".into(),
            items: vec![
                Choice::new("a", "apple", "red"),
                Choice::new("b", "banana", "yellow").needing_idle(),
                Choice::new("g", "grape", "purple"),
            ],
            empty_message: "no fruit".into(),
        }
    }

    pub fn many_choices() -> Choices {
        Choices {
            command: "many".into(),
            title: "Many".into(),
            items: (0..20).map(|i| Choice::new(i.to_string(), format!("item{i:02}"), "")).collect(),
            empty_message: String::new(),
        }
    }

    pub fn paths() -> Suggestions {
        let path = |p: &str| Suggestion {
            label: p.into(),
            description: String::new(),
            partial: format!("@{p}"),
            full: format!("@{p} "),
        };
        Suggestions { title: "Paths".into(), items: vec![path("a.rs"), path("b.rs")], empty_message: "no files".into() }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use thoth_core::test::assert_empty;

    fn picker() -> Picker {
        Picker::commands(fruit_commands())
    }

    fn picked(picker: &Picker, busy: bool) -> Option<String> {
        picker.select(busy).map(|(command, _)| command)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fake client's script holds Results, and the caller compares Options"
    )]
    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn starts_on_the_first_item() {
        assert_eq!(picked(&picker(), false), some("apple"));
    }

    #[test]
    fn typing_filters_by_substring_ignoring_case() {
        let mut picker = picker();
        assert!(picker.set_text("/AN"));
        assert_eq!(picked(&picker, false), some("banana"));
        assert!(picker.set_text("/ap"));
        assert_eq!(picked(&picker, false), some("apple"));
        assert!(picker.set_text("/e"));
        assert_eq!(picker.visible.len(), 2, "apple and grape");
    }

    #[test]
    fn a_command_is_run_by_its_id_and_a_choice_by_its_key() {
        let mut picker = picker();
        picker.set_text("/gr");
        assert_eq!(picker.select(false), Some(("grape".into(), None)));
        let mut choices = Picker::choices(fruit_choices());
        choices.set_text("gr");
        assert_eq!(choices.select(false), Some(("fruit".into(), some("g"))));
    }

    #[test]
    fn no_matches_selects_nothing() {
        let mut picker = picker();
        picker.set_text("/zzz");
        assert!(!picker.has_matches());
        assert!(picker.select(false).is_none());
        assert!(picker.completion(false).is_none());
    }

    #[test]
    fn the_selection_wraps_around() {
        let mut picker = picker();
        picker.move_selection(-1);
        assert_eq!(picked(&picker, false), some("grape"));
        picker.move_selection(1);
        assert_eq!(picked(&picker, false), some("apple"));
    }

    #[test]
    fn changing_the_query_resets_the_selection() {
        let mut picker = picker();
        picker.move_selection(1);
        picker.set_text("/a");
        assert_eq!(picked(&picker, false), some("apple"));
    }

    #[test]
    fn text_that_leaves_the_command_form_closes_the_command_menu() {
        let mut picker = picker();
        assert!(!picker.set_text("hello"));
        assert!(!picker.set_text("/apple pie"));
        assert!(picker.set_text("/"));
    }

    #[test]
    fn a_command_completes_with_its_slash() {
        let mut picker = picker();
        picker.move_selection(2);
        assert_eq!(picker.completion(false).map(|c| c.text).as_deref(), Some("/grape"));
    }

    #[test]
    fn busy_blocks_only_items_that_need_an_idle_agent() {
        let mut picker = picker();
        picker.move_selection(1);
        assert!(picker.select(true).is_none());
        assert_eq!(picked(&picker, false), some("banana"));
        picker.move_selection(1);
        assert_eq!(picked(&picker, true), some("grape"));
    }

    #[test]
    fn choices_take_any_text_as_the_query() {
        let mut picker = Picker::choices(fruit_choices());
        assert!(picker.set_text("gr ape"));
        assert!(!picker.has_matches());
        assert!(picker.set_text("gra"));
        assert!(picker.has_matches());
        assert!(!picker.is_root());
        assert_eq!(picker.completion(false).unwrap().text, "grape");
    }

    #[test]
    fn suggestions_complete_partly_while_typing_and_fully_when_settled() {
        let mut picker = Picker::suggestions(paths());
        picker.set_text("b");
        assert_eq!(picker.scope(), Scope::Mention);
        assert_eq!(picker.completion(false).unwrap().text, "@b.rs");
        assert_eq!(picker.completion(true).unwrap().text, "@b.rs ");
        assert!(picker.select(false).is_none(), "suggestions are not chosen");
    }

    fn shown(picker: &Picker) -> (Vec<String>, Option<usize>) {
        let view = picker.view();
        (view.rows.into_iter().map(|row| row.label).collect(), view.selected)
    }

    #[test]
    fn only_a_window_of_rows_is_in_the_view() {
        let picker = Picker::choices(many_choices());
        let (rows, selected) = shown(&picker);
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert_eq!((rows[0].as_str(), selected), ("item00", Some(0)));
    }

    #[test]
    fn the_window_follows_the_selection_when_it_wraps() {
        let mut picker = Picker::choices(many_choices());
        picker.move_selection(-1);
        let (rows, selected) = shown(&picker);
        assert_eq!(rows.last().unwrap(), "item19");
        assert_eq!(selected, Some(VISIBLE_ROWS - 1));
    }

    #[test]
    fn the_window_scrolls_only_when_the_selection_reaches_an_edge() {
        let mut picker = Picker::choices(many_choices());
        picker.move_selection(3);
        assert_eq!(shown(&picker).0[0], "item00");
        picker.move_selection(5);
        assert_eq!(shown(&picker).0[0], "item01");
        picker.move_selection(-5);
        assert_eq!(shown(&picker).0[0], "item01", "moving up keeps the window until the top row");
        picker.move_selection(-3);
        assert_eq!(shown(&picker).0[0], "item00");
    }

    #[test]
    fn an_empty_result_says_so_and_an_empty_menu_says_why() {
        let mut picker = picker();
        picker.set_text("/zzz");
        let view = picker.view();
        assert_empty(&view.rows);
        assert_eq!(view.empty_message, "no matches");

        let empty = Picker::choices(Choices { items: Vec::new(), ..fruit_choices() });
        assert_eq!(empty.view().empty_message, "no fruit");
    }
}
