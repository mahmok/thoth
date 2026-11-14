//! The terminal app: its state, and the modules that each act on one concern.

/// Slash command execution and what it changes.
mod commands;
/// Terminal setup, the frame loop and shutdown.
mod lifecycle;
/// Whole-screen layout and widget assembly.
mod rendering;
/// Event routing and what each key effect does.
mod routing;

use super::chat::ChatView;
use super::client::Client;
use super::picker::PickerHost;
use super::popup::Popup;
use super::prompt::{Prompt, Submission};
use super::toast::Toast;
use core::time::Duration;
use thoth_core::builtin::files::FILE_TRIGGER;
use thoth_core::session::view::Login as LoginView;

/// The characters that start a word the picker can complete.
const TRIGGERS: &[char] = &[FILE_TRIGGER];

/// The terminal app.
pub struct TerminalApp {
    chat: ChatView,
    harness: Client,
    pickers: PickerHost,
    popup: Popup,
    prompt: Prompt,
    quit: bool,
    /// The login as the last event this app handled left it.
    seen_login: LoginView,
    submitted: Option<Submission>,
    toast: Toast,
}

impl TerminalApp {
    pub fn new(harness: Client) -> Self {
        let mut prompt = Prompt::new();
        let seen_login = harness.login_view().clone();
        prompt.set_masked(seen_login.awaiting_secret);
        Self {
            harness,
            seen_login,
            chat: ChatView::default(),
            prompt,
            pickers: PickerHost::default(),
            popup: Popup::default(),
            submitted: None,
            quit: false,
            toast: Toast::default(),
        }
    }

    /// Shows a toast for `timeout`, replacing any already up. `now` is passed in so tests can
    /// move time without sleeping.
    pub fn notify(&mut self, text: impl Into<String>, timeout: Duration, now: u64) {
        self.toast.show(text, timeout, now);
    }

    /// Takes a toast down once its time is up. Returns whether the screen has to be redrawn.
    pub fn expire_toast(&mut self, now: u64) -> bool {
        self.toast.expire(now)
    }
}

#[cfg(test)]
mod tests;
