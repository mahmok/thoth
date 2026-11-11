use crate::command::{Choices, Command};
use thoth_core::builtin::files::Suggestions;

use super::{Completion, Picker, PickerView, Scope};
use crate::mention::{self, Mention};

/// Where the pickers' lists come from: the Session, asked only when a picker opens.
pub trait Catalog {
    fn commands(&mut self) -> Option<Vec<Command>>;

    fn suggestions(&mut self, trigger: char) -> Option<Suggestions>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A key a picker understands.
pub enum PickerKey {
    Down,
    Enter,
    Esc,
    Tab,
    Up,
}

/// What a key did to a picker.
enum Step {
    Complete(Completion),
    Handled,
    Ignored,
    Run(String, Option<String>),
}

/// The prompt as the terminal has it: its text, and the cursor counted in characters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptText {
    pub cursor: usize,
    pub text: String,
}

impl PromptText {
    pub fn at_end(text: impl Into<String>) -> Self {
        let owned = text.into();
        let cursor = owned.chars().count();
        Self { text: owned, cursor }
    }
}

/// What a key did, as far as the prompt and the Session are concerned.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The prompt is now this.
    Edit(PromptText),
    Handled,
    /// Something was chosen: the prompt should be emptied and the Command run in the Session.
    Run {
        command: String,
        choice: Option<String>,
    },
    /// No picker wanted the key.
    Unclaimed,
}

/// Owns the one open picker and the Esc dismissal, and keeps them in step with the prompt.
#[derive(Default)]
pub struct PickerHost {
    dismissed: Option<Scope>,
    open: Option<Picker>,
    open_trigger: Option<char>,
}

impl PickerHost {
    pub fn close(&mut self) {
        self.open = None;
    }

    pub const fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Handles a key for the open picker and says what to do about it.
    pub fn key(
        &mut self,
        key: PickerKey,
        busy: bool,
        prompt: &PromptText,
        triggers: &[char],
        catalog: &mut dyn Catalog,
    ) -> Outcome {
        match self.step(key, busy) {
            Step::Ignored => Outcome::Unclaimed,
            Step::Handled => Outcome::Handled,
            Step::Run(command, choice) => Outcome::Run { command, choice },
            Step::Complete(Completion { text, scope }) => {
                let edited = match scope {
                    Scope::Prompt => Some(PromptText::at_end(text)),
                    Scope::Mention => mention::replace_mention(&prompt.text, prompt.cursor, triggers, &text)
                        .map(|(replaced, position)| PromptText { text: replaced, cursor: position }),
                };
                let Some(change) = edited else {
                    return Outcome::Handled;
                };
                self.prompt_changed(&change, triggers, catalog);
                Outcome::Edit(change)
            },
        }
    }

    /// Opens the Choices a Command listed, such as the models after `/model`.
    pub fn open_choices(&mut self, choices: Choices) {
        self.open = Some(Picker::choices(choices));
        self.open_trigger = None;
    }

    /// Opens, filters or closes the picker to match the prompt: the command picker while it
    /// starts with `/`, a completion picker while a trigger word is being typed.
    pub fn prompt_changed(&mut self, prompt: &PromptText, triggers: &[char], catalog: &mut dyn Catalog) {
        let mention = mention::mention_at(&prompt.text, prompt.cursor, triggers);
        self.sync_with(&prompt.text, mention.as_ref(), catalog);
    }

    /// Moves or acts on the open picker for a key.
    fn step(&mut self, key: PickerKey, busy: bool) -> Step {
        let Some(picker) = self.open.as_mut() else {
            return Step::Ignored;
        };
        let scope = picker.scope();
        match key {
            PickerKey::Up => picker.move_selection(-1),
            PickerKey::Down => picker.move_selection(1),
            PickerKey::Tab => {
                return picker.completion(false).map_or(Step::Handled, Step::Complete);
            },
            PickerKey::Esc => {
                self.open = None;
                self.dismissed = Some(scope);
            },
            PickerKey::Enter if picker.has_matches() && scope == Scope::Mention => {
                let completion = picker.completion(true);
                self.open = None;
                return completion.map_or(Step::Handled, Step::Complete);
            },
            PickerKey::Enter if picker.has_matches() => {
                return match picker.select(busy) {
                    Some((command, choice)) => {
                        self.open = None;
                        Step::Run(command, choice)
                    },
                    None => Step::Handled,
                };
            },
            // Choices swallow Enter even with nothing to pick; the other pickers let it send.
            PickerKey::Enter if scope == Scope::Prompt && !picker.is_root() => {},
            PickerKey::Enter => return Step::Ignored,
        }
        Step::Handled
    }

    /// Keeps the open picker in step with the prompt text.
    fn sync_with(&mut self, text: &str, mention: Option<&Mention>, catalog: &mut dyn Catalog) {
        if let Some(picker) = self.open.as_mut() {
            let follows = match (picker.scope(), mention) {
                (Scope::Prompt, _) => picker.set_text(text),
                (Scope::Mention, Some(m)) if self.open_trigger == Some(m.trigger) => picker.set_text(&m.query),
                (Scope::Mention, _) => false,
            };
            if follows {
                return;
            }
            self.open = None;
        }

        let command_form = text.starts_with('/') && !text.contains(char::is_whitespace);
        if !command_form && self.dismissed == Some(Scope::Prompt) {
            self.dismissed = None;
        }
        if mention.is_none() && self.dismissed == Some(Scope::Mention) {
            self.dismissed = None;
        }

        if command_form {
            if self.dismissed != Some(Scope::Prompt)
                && let Some(commands) = catalog.commands()
            {
                let mut picker = Picker::commands(commands);
                let _changed = picker.set_text(text);
                self.open = Some(picker);
            }
        } else if let Some(m) = mention
            && self.dismissed != Some(Scope::Mention)
            && let Some(suggestions) = catalog.suggestions(m.trigger)
        {
            let mut picker = Picker::suggestions(suggestions);
            let _changed = picker.set_text(&m.query);
            self.open_trigger = Some(m.trigger);
            self.open = Some(picker);
        } else {
            // There is nothing to open.
        }
    }

    pub fn view(&self) -> Option<PickerView> {
        self.open.as_ref().map(Picker::view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::testing::{fruit_choices, fruit_commands, paths};

    /// Fruit for `/`, paths for `@`, and a count of how often each was asked for.
    #[derive(Default)]
    struct Fake {
        asked: Vec<String>,
    }

    impl Catalog for Fake {
        fn commands(&mut self) -> Option<Vec<Command>> {
            self.asked.push("/".into());
            Some(fruit_commands())
        }

        fn suggestions(&mut self, trigger: char) -> Option<Suggestions> {
            self.asked.push(trigger.to_string());
            (trigger == '@').then(paths)
        }
    }

    fn mention(query: &str) -> Mention {
        Mention { trigger: '@', query: query.into(), len: query.chars().count().saturating_add(1) }
    }

    fn sync(host: &mut PickerHost, text: &str, mention: Option<&Mention>) {
        host.sync_with(text, mention, &mut Fake::default());
    }

    fn host_with(text: &str) -> PickerHost {
        let mut host = PickerHost::default();
        sync(&mut host, text, None);
        host
    }

    fn host_mentioning(text: &str, query: &str) -> PickerHost {
        let mut host = PickerHost::default();
        sync(&mut host, text, Some(&mention(query)));
        host
    }

    #[test]
    fn the_catalog_is_asked_only_when_a_picker_opens() {
        let mut host = PickerHost::default();
        let mut catalog = Fake::default();
        for text in ["/", "/a", "/ap"] {
            host.sync_with(text, None, &mut catalog);
        }
        assert_eq!(catalog.asked, ["/"]);
    }

    #[test]
    fn a_trigger_without_suggestions_opens_nothing() {
        let mut host = PickerHost::default();
        let issue = Mention { trigger: '#', query: String::new(), len: 1 };
        sync(&mut host, "#", Some(&issue));
        assert!(!host.is_open());
    }

    #[test]
    fn a_mention_opens_the_file_menu_and_ends_with_the_mention() {
        let mut host = host_mentioning("see @a", "a");
        assert!(host.is_open());
        sync(&mut host, "see @", Some(&mention("")));
        assert!(host.is_open());
        sync(&mut host, "see @a.rs ", None);
        assert!(!host.is_open());
    }

    #[test]
    fn enter_on_a_mention_completes_it_and_tab_keeps_the_menu_open() {
        let mut host = host_mentioning("@b", "b");
        let Step::Complete(tab) = host.step(PickerKey::Tab, false) else { panic!("expected a completion") };
        assert_eq!((tab.text.as_str(), tab.scope), ("@b.rs", Scope::Mention));
        assert!(host.is_open());
        let Step::Complete(enter) = host.step(PickerKey::Enter, false) else { panic!("expected a completion") };
        assert_eq!(enter.text, "@b.rs ");
        assert!(!host.is_open());
    }

    #[test]
    fn enter_with_no_file_matches_sends_the_prompt() {
        let mut host = host_mentioning("@zzz", "zzz");
        assert!(matches!(host.step(PickerKey::Enter, false), Step::Ignored));
    }

    #[test]
    fn escape_closes_a_mention_until_the_mention_is_gone() {
        let mut host = host_mentioning("@a", "a");
        assert!(matches!(host.step(PickerKey::Esc, false), Step::Handled));
        sync(&mut host, "@ab", Some(&mention("ab")));
        assert!(!host.is_open(), "typing on does not reopen it");
        sync(&mut host, "@ab ", None);
        sync(&mut host, "@ab x @", Some(&mention("")));
        assert!(host.is_open());
    }

    #[test]
    fn a_command_picker_wins_over_a_mention() {
        let mut host = PickerHost::default();
        sync(&mut host, "/", Some(&mention("x")));
        let Step::Complete(tab) = host.step(PickerKey::Tab, false) else { panic!("expected a completion") };
        assert_eq!(tab.scope, Scope::Prompt);
    }

    #[test]
    fn a_leading_slash_opens_the_menu_and_a_space_closes_it() {
        let mut host = PickerHost::default();
        sync(&mut host, "hello", None);
        assert!(!host.is_open());
        sync(&mut host, "/", None);
        assert!(host.is_open());
        sync(&mut host, "/app", None);
        assert!(host.is_open());
        sync(&mut host, "/apple pie", None);
        assert!(!host.is_open());
    }

    #[test]
    fn keys_are_ignored_while_no_menu_is_open() {
        let mut host = PickerHost::default();
        assert!(matches!(host.step(PickerKey::Enter, false), Step::Ignored));
        assert!(matches!(host.step(PickerKey::Esc, false), Step::Ignored));
    }

    #[test]
    fn enter_runs_the_highlighted_command_and_closes_the_picker() {
        let mut host = host_with("/");
        assert!(matches!(
            host.step(PickerKey::Enter, false),
            Step::Run(command, None) if command == "apple"
        ));
        assert!(!host.is_open());
    }

    #[test]
    fn enter_on_an_item_that_needs_an_idle_agent_does_nothing_while_busy() {
        let mut host = host_with("/");
        host.step(PickerKey::Down, true);
        assert!(matches!(host.step(PickerKey::Enter, true), Step::Handled));
        assert!(host.is_open());
        assert!(matches!(
            host.step(PickerKey::Enter, false),
            Step::Run(command, None) if command == "banana"
        ));
    }

    #[test]
    fn tab_completes_without_closing() {
        let mut host = host_with("/b");
        let Step::Complete(text) = host.step(PickerKey::Tab, false) else { panic!("expected a completion") };
        assert_eq!(text.text, "/banana");
        assert!(host.is_open());
    }

    #[test]
    fn enter_with_no_matches_falls_through_on_the_command_picker_only() {
        let mut host = host_with("/zzz");
        assert!(matches!(host.step(PickerKey::Enter, false), Step::Ignored));
        host.open_choices(fruit_choices());
        sync(&mut host, "zzz", None);
        assert!(matches!(host.step(PickerKey::Enter, false), Step::Handled));
    }

    #[test]
    fn escape_closes_and_stays_closed_until_the_text_leaves_the_command_form() {
        let mut host = host_with("/m");
        assert!(matches!(host.step(PickerKey::Esc, false), Step::Handled));
        assert!(!host.is_open());
        sync(&mut host, "/mo", None);
        assert!(!host.is_open(), "typing the same command does not reopen it");
        sync(&mut host, "", None);
        sync(&mut host, "/", None);
        assert!(host.is_open());
    }

    #[test]
    fn enter_on_a_choice_runs_its_command_with_its_key() {
        let mut host = PickerHost::default();
        host.open_choices(fruit_choices());
        sync(&mut host, "grape", None);
        assert!(matches!(
            host.step(PickerKey::Enter, false),
            Step::Run(command, Some(key)) if command == "fruit" && key == "g"
        ));
    }

    #[test]
    fn a_tab_on_a_mention_replaces_just_the_word_before_the_cursor() {
        let mut host = PickerHost::default();
        let prompt = PromptText { text: "see @a and more".into(), cursor: 6 };
        let mut catalog = Fake::default();
        host.prompt_changed(&prompt, &['@'], &mut catalog);
        let outcome = host.key(PickerKey::Enter, false, &prompt, &['@'], &mut catalog);
        assert_eq!(outcome, Outcome::Edit(PromptText { text: "see @a.rs  and more".into(), cursor: 10 }));
    }
}
