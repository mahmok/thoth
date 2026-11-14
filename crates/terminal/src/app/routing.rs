//! Event routing: one rule for every key press. A key resolves to at most one effect, that
//! effect is performed, and its outcome is observed.

use core::mem;

use super::{TRIGGERS, TerminalApp};
use crate::browser;
use crate::chat::ChatAction;
use crate::clipboard;
use crate::keymap::{self, Context, Effect};
use crate::picker::{Outcome, PickerKey, PromptText};
use crossterm::event::{Event, KeyEvent, KeyEventKind};
use thoth_core::harness::Event as HarnessEvent;
use thoth_core::session::event::Event as SessionEvent;
use thoth_core::session::view::Login as LoginView;
use thoth_core::transcript::{Change as TranscriptChange, Content, Item, Speaker};

impl TerminalApp {
    /// Handles one terminal event.
    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => {
                if let Some(text) = self.prompt.handle_mouse(mouse) {
                    let _unused = clipboard::copy(&text);
                } else {
                    match self.chat.handle_mouse(mouse, &self.harness.view().transcript) {
                        Some(ChatAction::Copy(text)) => {
                            let _unused = clipboard::copy(&text);
                        },
                        Some(ChatAction::OpenUrl(url)) => browser::open(&url),
                        None => {},
                    }
                }
            },
            Event::Paste(pasted) => {
                self.prompt.handle_paste(&pasted);
                self.prompt_changed();
            },
            Event::FocusGained | Event::FocusLost | Event::Key(_) | Event::Resize(..) => {},
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match keymap::effect_for(key, self.context()) {
            Some(effect) => self.perform(effect),
            None => {
                self.prompt.handle_key(key);
                self.prompt_changed();
            },
        }
    }

    /// Where the user is: which context answers a key, and which hints are shown.
    pub(super) fn context(&self) -> Context {
        let login = self.harness.login_view();
        if self.pickers.is_open() {
            Context::Picker
        } else if login.awaiting_secret {
            Context::Secret
        } else if login.active {
            Context::Login
        } else {
            Context::Always
        }
    }

    fn last_user_message(&self) -> Option<String> {
        self.harness.view().transcript.items().iter().rev().find_map(|item| match item.content {
            Content::Message(ref message) if message.speaker == Speaker::You => Some(message.text.clone()),
            Content::Message(_) | Content::Tool(_) => None,
        })
    }

    fn on_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Transcript(TranscriptChange::Cleared) => self.chat.reset(),
            SessionEvent::Transcript(TranscriptChange::Added(item)) if is_said(&item) => {
                self.chat.scroll_down(usize::MAX);
            },
            SessionEvent::Transcript(
                TranscriptChange::Added(_) | TranscriptChange::Appended { .. } | TranscriptChange::Replaced(_),
            )
            | SessionEvent::Busy(_)
            | SessionEvent::Stats(_)
            | SessionEvent::Settings(_) => {},
        }
    }

    fn on_login(&mut self, login: &LoginView, before: &LoginView) {
        self.prompt.set_masked(login.awaiting_secret);
        if before.awaiting_secret && !login.awaiting_secret {
            // Whatever part of a secret was typed goes with the question.
            self.set_input("");
        }
        if let Some(code) = login.device_code.as_ref()
            && before.device_code.as_ref() != Some(code)
        {
            let _unused = clipboard::copy(code);
        }
    }

    /// Does what a key effect asks. Every key goes through here, so its outcome has one rule:
    /// only Submit has somewhere to fall back to, and a picker that refuses a key it was sent
    /// has simply not taken it.
    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Quit => self.quit = true,
            Effect::Cancel => self.harness.cancel(),
            Effect::Submit => {
                if self.press(PickerKey::Enter) == Outcome::Unclaimed {
                    self.submit();
                }
            },
            Effect::Picker(key) => {
                let _taken = self.press(key);
            },
            Effect::ScrollUp if self.prompt.text().is_empty() => self.recall_last_input(),
            Effect::ScrollUp => self.chat.scroll_up(1),
            Effect::ScrollDown => self.chat.scroll_down(1),
            Effect::PageUp => self.chat.scroll_up(self.chat.page_size()),
            Effect::PageDown => self.chat.scroll_down(self.chat.page_size()),
        }
    }

    /// Takes what the harness has to report. Returns whether there was anything, so the screen
    /// is only redrawn when it has changed.
    pub fn poll_events(&mut self) -> bool {
        let current = self.harness.session_id();
        let events = self.harness.poll();
        let mut changed = !events.is_empty();
        for event in events {
            match event {
                HarnessEvent::Login(login) => {
                    let before = mem::replace(&mut self.seen_login, login.clone());
                    self.on_login(&login, &before);
                },
                HarnessEvent::Session { id, event: inner } if id == current => self.on_event(inner),
                HarnessEvent::Session { .. } | HarnessEvent::SessionClosed(_) => {},
                HarnessEvent::Notice { speaker, text } => {
                    self.harness.session_mut().say(speaker, text);
                },
            }
        }
        // What the notices added to the viewed session.
        let said = self.harness.session_mut().poll();
        changed |= !said.is_empty();
        for event in said {
            self.on_event(event);
        }
        changed
    }

    /// Hands a key to the open picker, and does what it decided.
    fn press(&mut self, key: PickerKey) -> Outcome {
        let prompt = self.prompt_text();
        let mut catalog = super::commands::SessionCatalog(&self.harness);
        let view = self.harness.view();
        let outcome = self.pickers.key(key, view.busy, &prompt, TRIGGERS, &mut catalog);
        match outcome {
            Outcome::Unclaimed | Outcome::Handled => {},
            Outcome::Edit(ref edited) => self.prompt.set_text(&edited.text, edited.cursor),
            Outcome::Run { ref command, ref choice } => self.execute(command, choice.as_deref()),
        }
        outcome
    }

    /// Keeps the picker in step with the prompt. A secret being typed opens none.
    pub(super) fn prompt_changed(&mut self) {
        if self.harness.login_view().awaiting_secret {
            return self.pickers.close();
        }
        let prompt = self.prompt_text();
        let mut catalog = super::commands::SessionCatalog(&self.harness);
        self.pickers.prompt_changed(&prompt, TRIGGERS, &mut catalog);
    }

    fn prompt_text(&self) -> PromptText {
        PromptText { text: self.prompt.text(), cursor: self.prompt.cursor() }
    }

    /// Puts the user's latest message back in the empty prompt, with any pasted text in full.
    fn recall_last_input(&mut self) {
        let Some(shown) = self.last_user_message() else {
            return self.chat.scroll_up(1);
        };
        let text = match self.submitted.as_ref() {
            Some(sent) if sent.shown == shown => sent.full.clone(),
            _ => shown,
        };
        self.set_input(&text);
    }

    pub(super) fn set_input(&mut self, text: &str) {
        self.prompt.set_text(text, text.chars().count());
        self.prompt_changed();
    }

    fn submit(&mut self) {
        self.submitted = self.prompt.submission();
        let Some(submission) = self.submitted.as_ref() else {
            return;
        };
        let (shown, full) = (submission.shown.clone(), submission.full.clone());
        if self.harness.submit(shown, full) {
            self.set_input("");
        }
    }
}

/// Whether a new item is something the user was told, which brings the chat down to it. Agent
/// output and Tool cards arrive while the user may be reading back, so they do not.
const fn is_said(item: &Item) -> bool {
    match item.content {
        Content::Message(ref message) => !matches!(message.speaker, Speaker::Agent | Speaker::Thinking),
        Content::Tool(_) => false,
    }
}
