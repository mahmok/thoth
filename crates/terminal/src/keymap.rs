use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key a picker understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKey {
    Down,
    Enter,
    Esc,
    Tab,
    Up,
}

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Cancels what is running.
    Cancel,
    /// Hands the key to the open picker.
    Picker(PickerKey),
    /// Sends the prompt, unless a picker takes the key first.
    Submit,
    PageDown,
    PageUp,
    Quit,
    ScrollDown,
    ScrollUp,
}

/// Where the user is, which decides what a key does and which hints are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Nothing has the keyboard: the prompt does.
    Always,
    /// A picker is open and takes its own keys before anything else.
    Picker,
    /// A login is running.
    Login,
    /// A login waits for the user to paste a key.
    Secret,
}

/// A key bound to an effect.
#[derive(Debug, Clone, Copy)]
struct Binding {
    code: KeyCode,
    effect: Effect,
    modifiers: KeyModifiers,
}

impl Binding {
    const fn plain(code: KeyCode, effect: Effect) -> Self {
        Self { code, effect, modifiers: KeyModifiers::NONE }
    }

    const fn keyed(code: KeyCode, modifiers: KeyModifiers, effect: Effect) -> Self {
        Self { code, effect, modifiers }
    }

    #[cfg(test)]
    const fn event(self) -> KeyEvent {
        KeyEvent::new(self.code, self.modifiers)
    }
}

/// One thing the user can do, with the keys that do it and the wording of its hint.
#[derive(Debug)]
struct Group {
    bindings: &'static [Binding],
    keys: &'static str,
    label: &'static str,
    show_hint: bool,
}

/// The hints of one Context.
#[derive(Debug)]
struct Hints {
    groups: &'static [Group],
    /// Said before the keys, when the context needs explaining.
    lead: Option<&'static str>,
}

const SEND: Binding = Binding::plain(KeyCode::Enter, Effect::Submit);
const CANCEL: Binding = Binding::plain(KeyCode::Esc, Effect::Cancel);
const QUIT: Binding = Binding::keyed(KeyCode::Char('d'), KeyModifiers::CONTROL, Effect::Quit);
const SCROLL_UP: Binding = Binding::plain(KeyCode::Up, Effect::ScrollUp);
const SCROLL_DOWN: Binding = Binding::plain(KeyCode::Down, Effect::ScrollDown);
const PAGE_UP: Binding = Binding::plain(KeyCode::PageUp, Effect::PageUp);
const PAGE_DOWN: Binding = Binding::plain(KeyCode::PageDown, Effect::PageDown);

const PICK_UP: Binding = Binding::plain(KeyCode::Up, Effect::Picker(PickerKey::Up));
const PICK_DOWN: Binding = Binding::plain(KeyCode::Down, Effect::Picker(PickerKey::Down));
const PICK_TAB: Binding = Binding::plain(KeyCode::Tab, Effect::Picker(PickerKey::Tab));
const PICK_ESC: Binding = Binding::plain(KeyCode::Esc, Effect::Picker(PickerKey::Esc));

const ALWAYS_GROUPS: &[Group] = &[
    Group { keys: "Enter", label: "send", bindings: &[SEND], show_hint: false },
    Group { keys: "/", label: "commands", bindings: &[], show_hint: true },
    Group { keys: "@", label: "files", bindings: &[], show_hint: true },
    Group { keys: "drag", label: "select & copy", bindings: &[], show_hint: false },
    Group { keys: "click card", label: "expand", bindings: &[], show_hint: false },
    Group {
        keys: "\u{2191}\u{2193}/PgUp/PgDn",
        label: "scroll",
        bindings: &[SCROLL_UP, SCROLL_DOWN, PAGE_UP, PAGE_DOWN],
        show_hint: false,
    },
    Group { keys: "Esc", label: "cancel", bindings: &[CANCEL], show_hint: true },
    Group { keys: "CTRL+D", label: "quit", bindings: &[QUIT], show_hint: true },
];

const PICKER_GROUPS: &[Group] = &[
    Group { keys: "\u{2191}\u{2193}", label: "select", bindings: &[PICK_UP, PICK_DOWN], show_hint: true },
    Group { keys: "Enter", label: "run", bindings: &[SEND], show_hint: true },
    Group { keys: "Tab", label: "complete", bindings: &[PICK_TAB], show_hint: true },
    Group { keys: "Esc", label: "close", bindings: &[PICK_ESC], show_hint: true },
];

const LOGIN_GROUPS: &[Group] = &[Group { keys: "Esc", label: "cancel login", bindings: &[CANCEL], show_hint: true }];

const SECRET_GROUPS: &[Group] = &[
    Group { keys: "Enter", label: "save the key", bindings: &[SEND], show_hint: true },
    Group { keys: "Esc", label: "cancel login", bindings: &[CANCEL], show_hint: true },
];

const HINTS: &[(Context, Hints)] = &[
    (Context::Always, Hints { groups: ALWAYS_GROUPS, lead: None }),
    (Context::Picker, Hints { groups: PICKER_GROUPS, lead: None }),
    (Context::Login, Hints { groups: LOGIN_GROUPS, lead: Some("Logging in...") }),
    (Context::Secret, Hints { groups: SECRET_GROUPS, lead: None }),
];

const fn hints_of(context: Context) -> &'static Hints {
    match context {
        Context::Always => &HINTS[0].1,
        Context::Picker => &HINTS[1].1,
        Context::Login => &HINTS[2].1,
        Context::Secret => &HINTS[3].1,
    }
}

/// The groups a context owns, without the ones that hold everywhere.
const fn own_groups(context: Context) -> &'static [Group] {
    match context {
        Context::Always => ALWAYS_GROUPS,
        Context::Picker => PICKER_GROUPS,
        Context::Login => LOGIN_GROUPS,
        Context::Secret => SECRET_GROUPS,
    }
}

/// The groups that may answer a key in this context: its own first, then the ones that hold
/// everywhere. A context therefore overrides a general key simply by binding it again.
fn groups_for(context: Context) -> impl Iterator<Item = &'static Group> {
    let general = (context != Context::Always).then_some(ALWAYS_GROUPS).into_iter().flatten();
    own_groups(context).iter().chain(general)
}

/// What this key does here, if anything. Keys with no binding are left for the prompt.
pub fn effect_for(key: KeyEvent, context: Context) -> Option<Effect> {
    groups_for(context)
        .flat_map(|group| group.bindings)
        .find(|binding| binding.code == key.code && binding.modifiers == key.modifiers)
        .map(|binding| binding.effect)
}

/// The hint line for this context, in one place so no hint can name a key that does nothing.
pub fn hints(context: Context) -> String {
    let hints = hints_of(context);
    let groups = hints
        .groups
        .iter()
        .filter(|group| group.show_hint)
        .map(|group| format!("{}: {}", group.keys, group.label))
        .collect::<Vec<_>>()
        .join(" | ");
    match hints.lead {
        Some(lead) => format!("{lead} | {groups}"),
        None => groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXTS: [Context; 4] = [Context::Always, Context::Picker, Context::Login, Context::Secret];

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn every_binding_resolves_to_its_effect_in_its_context() {
        for context in CONTEXTS {
            for group in own_groups(context) {
                for binding in group.bindings {
                    let found = effect_for(binding.event(), context);
                    assert_eq!(found, Some(binding.effect), "{:?} in {context:?}", binding.code);
                }
            }
        }
    }

    #[test]
    fn a_context_overrides_the_general_binding() {
        assert_eq!(effect_for(press(KeyCode::Up), Context::Always), Some(Effect::ScrollUp));
        assert_eq!(effect_for(press(KeyCode::Up), Context::Picker), Some(Effect::Picker(PickerKey::Up)));
        assert_eq!(effect_for(press(KeyCode::Esc), Context::Always), Some(Effect::Cancel));
        assert_eq!(effect_for(press(KeyCode::Esc), Context::Picker), Some(Effect::Picker(PickerKey::Esc)));
    }

    #[test]
    fn a_context_key_still_needs_its_modifiers() {
        // The picker used to match on the key code alone, so CTRL+Up moved its selection.
        assert_eq!(effect_for(KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL), Context::Picker), None);
    }

    #[test]
    fn keys_the_prompt_owns_are_left_for_it() {
        for code in [KeyCode::Char('a'), KeyCode::Backspace, KeyCode::Left, KeyCode::Delete, KeyCode::Tab] {
            assert_eq!(effect_for(press(code), Context::Always), None, "{code:?}");
        }
    }

    #[test]
    fn modifiers_are_part_of_every_binding() {
        assert_eq!(effect_for(ctrl(KeyCode::Char('d')), Context::Always), Some(Effect::Quit));
        assert_eq!(effect_for(press(KeyCode::Char('d')), Context::Always), None);
        assert_eq!(effect_for(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL), Context::Always), None);
    }

    #[test]
    fn no_key_is_bound_twice_within_a_context() {
        for context in CONTEXTS {
            let seen: Vec<(KeyCode, KeyModifiers)> = own_groups(context)
                .iter()
                .flat_map(|group| group.bindings)
                .map(|binding| (binding.code, binding.modifiers))
                .collect();
            for (index, key) in seen.iter().enumerate() {
                assert!(!seen[index + 1..].contains(key), "{key:?} is bound twice in {context:?}");
            }
        }
    }

    #[test]
    fn every_effect_can_be_reached() {
        let reached: Vec<Effect> = CONTEXTS
            .into_iter()
            .flat_map(own_groups)
            .flat_map(|group| group.bindings)
            .map(|binding| binding.effect)
            .collect();
        // PickerKey::Enter is not its own binding: Enter is Submit, which asks the picker first.
        for effect in [
            Effect::Cancel,
            Effect::PageDown,
            Effect::PageUp,
            Effect::Quit,
            Effect::ScrollDown,
            Effect::ScrollUp,
            Effect::Submit,
            Effect::Picker(PickerKey::Down),
            Effect::Picker(PickerKey::Esc),
            Effect::Picker(PickerKey::Tab),
            Effect::Picker(PickerKey::Up),
        ] {
            assert!(reached.contains(&effect), "{effect:?} has no key");
        }
    }

    #[test]
    fn the_footer_shows_the_condensed_hints() {
        assert_eq!(hints(Context::Always), "/: commands | @: files | Esc: cancel | CTRL+D: quit");
    }

    #[test]
    fn the_picker_hints_name_only_keys_it_takes() {
        assert_eq!(hints(Context::Picker), "\u{2191}\u{2193}: select | Enter: run | Tab: complete | Esc: close");
        for group in PICKER_GROUPS {
            for binding in group.bindings {
                assert_eq!(effect_for(binding.event(), Context::Picker), Some(binding.effect), "{}", group.keys);
            }
        }
    }

    #[test]
    fn the_login_hints_say_what_is_happening() {
        assert_eq!(hints(Context::Login), "Logging in... | Esc: cancel login");
        assert_eq!(hints(Context::Secret), "Enter: save the key | Esc: cancel login");
    }
}
