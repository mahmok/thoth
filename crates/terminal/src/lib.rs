//! The terminal interface of Thoth.

#![cfg_attr(test, expect(unused_results, reason = "tests call things for their effect and ignore what they return"))]

/// The terminal app.
mod app;
/// Opening URLs.
mod browser;
/// The chat pane.
mod chat;
/// The Harness and the Session being viewed.
mod client;
/// Copying to the clipboard.
mod clipboard;
/// Slash commands.
mod command;
/// A left-button drag.
mod drag;
mod header;
/// Key bindings.
mod keymap;
/// `@` and `/` triggers in the prompt.
mod mention;
/// Terminal modes entered and restored.
mod modes;
mod picker;
/// The picker popup.
mod popup;
/// The input box.
mod prompt;
/// Selecting text from rows.
mod select;
/// The token summary line.
mod stats;
#[cfg(test)]
/// The shared test harness and the widget fixtures.
mod testing;
mod theme;
/// Short-lived messages drawn over the screen.
mod toast;
mod units;

extern crate alloc;

use thoth_core::application::harness::Harness;

#[inline]
/// Runs the app until the user quits.
///
/// # Errors
///
/// Fails if the terminal cannot be set up or drawn to.
pub fn run(harness: Harness) -> color_eyre::Result<()> {
    let client = client::TerminalClient::open(harness).map_err(|e| color_eyre::eyre::eyre!(e))?;
    app::TerminalApp::new(client).run()
}
