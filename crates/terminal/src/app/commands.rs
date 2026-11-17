//! Command execution: what running a slash command changes, and nothing about how it is drawn
//! or which key asked for it.

use super::TerminalApp;
use crate::client::TerminalClient;
use crate::command::{self, Ran};
use crate::picker::Catalog;
use thoth_core::adapters::files::{FILE_TRIGGER, Suggestions};

impl TerminalApp {
    /// Puts the outcome of a command where it belongs.
    fn apply(&mut self, ran: Ran) {
        match ran {
            Ran::Done => {},
            Ran::Insert(text) => self.set_input(&text),
            Ran::Choices(choices) => self.pickers.open_choices(choices),
            Ran::Quit => self.quit = true,
        }
    }

    /// Runs a command the picker chose, with the prompt emptied first.
    pub(super) fn execute(&mut self, command: &str, choice: Option<&str>) {
        self.prompt.set_text("", 0);
        // A refusal leaves nothing to undo: the picker already checked what it could,
        // and whatever went stale since shows when the user looks again.
        let before = self.harness.session_id();
        if let Ok(ran) = command::run(&mut self.harness, command, choice) {
            self.apply(ran);
        }
        if self.harness.session_id() != before {
            self.chat.reset();
        }
        self.prompt_changed();
    }
}

/// The pickers' lists, asked of the Session only when a picker opens.
pub(super) struct SessionCatalog<'client>(pub(super) &'client TerminalClient);

impl Catalog for SessionCatalog<'_> {
    fn commands(&mut self) -> Option<Vec<command::Command>> {
        Some(command::list(self.0))
    }

    fn suggestions(&mut self, trigger: char) -> Option<Suggestions> {
        (trigger == FILE_TRIGGER).then(|| self.0.suggest_files())
    }
}
