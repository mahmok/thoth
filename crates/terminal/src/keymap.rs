use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The slash commands and their help.
const COMMANDS: &[Command] = &[
    Command { keys: "Enter", label: "send", bindings: &[ActionBinding::plain(KeyCode::Enter, Action::Submit)] },
    Command { keys: "Esc", label: "cancel", bindings: &[ActionBinding::plain(KeyCode::Esc, Action::Cancel)] },
    Command { keys: "/", label: "commands", bindings: &[] },
    Command { keys: "@", label: "files", bindings: &[] },
    Command { keys: "drag", label: "select & copy", bindings: &[] },
    Command { keys: "click card", label: "expand", bindings: &[] },
    Command {
        keys: "\u{2191}\u{2193}/PgUp/PgDn",
        label: "scroll",
        bindings: &[
            ActionBinding::plain(KeyCode::Up, Action::ScrollUp),
            ActionBinding::plain(KeyCode::Down, Action::ScrollDown),
            ActionBinding::plain(KeyCode::PageUp, Action::PageUp),
            ActionBinding::plain(KeyCode::PageDown, Action::PageDown),
        ],
    },
    Command {
        keys: "CTRL+D",
        label: "quit",
        bindings: &[ActionBinding::new(KeyCode::Char('d'), KeyModifiers::CONTROL, Action::Quit)],
    },
];

/// The key hints shown with a picker.
pub const PICKER_HINTS: &str = "\u{2191}\u{2193}: select | Enter: run | Tab: complete | Esc: close";

/// The key hints shown during a login.
pub const LOGIN_HINTS: &str = "Logging in... | Esc: cancel login";

/// The key hints shown while a secret is typed.
pub const SECRET_HINTS: &str = "Enter: save the key | Esc: cancel login";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A key action.
pub enum Action {
    Cancel,
    PageDown,
    PageUp,
    Quit,
    ScrollDown,
    ScrollUp,
    Submit,
}

/// A key bound to an action.
struct ActionBinding {
    action: Action,
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl ActionBinding {
    const fn new(code: KeyCode, modifiers: KeyModifiers, action: Action) -> Self {
        Self { action, code, modifiers }
    }

    const fn plain(code: KeyCode, action: Action) -> Self {
        Self::new(code, KeyModifiers::NONE, action)
    }
}

/// One thing the user can do, as shown in the hints line. A command can be reached by several keys.
struct Command {
    bindings: &'static [ActionBinding],
    keys: &'static str,
    label: &'static str,
}

/// The action bound to this key press, if any. Keys with no binding are left for the prompt.
pub fn action_for(key: KeyEvent) -> Option<Action> {
    COMMANDS
        .iter()
        .flat_map(|command| command.bindings)
        .find(|binding| binding.code == key.code && binding.modifiers == key.modifiers)
        .map(|binding| binding.action)
}

pub fn hints() -> String {
    COMMANDS.iter().map(|command| format!("{}: {}", command.keys, command.label)).collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn keys_map_to_their_actions() {
        assert_eq!(action_for(press(KeyCode::Enter)), Some(Action::Submit));
        assert_eq!(action_for(ctrl(KeyCode::Char('d'))), Some(Action::Quit));
        assert_eq!(action_for(press(KeyCode::Up)), Some(Action::ScrollUp));
        assert_eq!(action_for(press(KeyCode::Down)), Some(Action::ScrollDown));
        assert_eq!(action_for(press(KeyCode::PageUp)), Some(Action::PageUp));
        assert_eq!(action_for(press(KeyCode::PageDown)), Some(Action::PageDown));
    }

    #[test]
    fn typing_keys_are_left_for_the_prompt() {
        for code in [KeyCode::Char('a'), KeyCode::Backspace, KeyCode::Left, KeyCode::Tab, KeyCode::Delete] {
            assert_eq!(action_for(press(code)), None, "{code:?}");
        }
    }

    #[test]
    fn modifiers_are_part_of_the_binding() {
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(action_for(ctrl_enter), None);
    }

    #[test]
    fn no_key_is_bound_twice() {
        let keys: Vec<(KeyCode, KeyModifiers)> =
            COMMANDS.iter().flat_map(|c| c.bindings).map(|b| (b.code, b.modifiers)).collect();
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[index + 1..].contains(key), "{key:?} is bound twice");
        }
    }

    #[test]
    fn every_action_has_a_key() {
        let actions: Vec<Action> = COMMANDS.iter().flat_map(|c| c.bindings).map(|b| b.action).collect();
        for action in
            [Action::Submit, Action::Quit, Action::ScrollUp, Action::ScrollDown, Action::PageUp, Action::PageDown]
        {
            assert!(actions.contains(&action), "{action:?} has no key");
        }
    }

    #[test]
    fn the_hints_line_lists_every_command() {
        assert_eq!(
            hints(),
            "Enter: send | Esc: cancel | /: commands | @: files | drag: select & copy | click card: expand | \u{2191}\u{2193}/PgUp/PgDn: scroll | CTRL+D: quit"
        );
    }
}
