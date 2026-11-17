//! Terminal setup, the frame loop and shutdown. A rendering change must not be able to affect
//! what happens here, and nothing here draws a widget.

use core::time::Duration;
use std::io;
use std::io::stdout;
use std::time::Instant;

use crossterm::event;
use ratatui::DefaultTerminal;
use thoth_core::clock::now_ms;

use super::TerminalApp;
use crate::modes::Modes;

/// How often the harness is asked what happened in the background.
const TICK: Duration = Duration::from_millis(50);
/// How often the screen is redrawn for what changes by itself: the clock on a running Tool card
/// shows tenths of a second.
const ANIMATION_TICK: Duration = Duration::from_millis(100);

impl TerminalApp {
    /// Whether something on screen changes with time alone.
    fn animating(&self) -> bool {
        self.harness.view().busy || self.harness.view().transcript.has_running_tool()
    }

    /// Draws and handles events until the user quits.
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut dirty = true;
        let mut drawn = Instant::now();
        loop {
            let now = now_ms();
            dirty |= self.poll_events();
            // A toast takes itself down once its time is up.
            dirty |= self.expire_toast(now);
            if self.quit {
                return Ok(());
            }
            // Draw when something changed, or when a running Tool card's clock needs updating.
            if self.animating() && drawn.elapsed() >= ANIMATION_TICK {
                dirty = true;
            }
            if dirty {
                let _frame = terminal.draw(|frame| self.render(frame, now_ms()))?;
                dirty = false;
                drawn = Instant::now();
            }

            // Take everything that is waiting before drawing again, so a burst of
            // input (a paste, a mouse drag) costs one redraw rather than one per event.
            let mut wait = TICK;
            while event::poll(wait)? {
                self.handle_event(event::read()?, now_ms());
                dirty = true;
                wait = Duration::ZERO;
            }
        }
    }

    /// Runs the app in the terminal, restoring it however the loop ends.
    pub fn run(mut self) -> color_eyre::Result<()> {
        color_eyre::install()?;
        ratatui::run(|terminal| {
            let modes = Modes::enter(stdout())?;
            let result = self.event_loop(terminal);
            drop(modes);
            discard_pending_input();
            result
        })?;
        Ok(())
    }
}

/// Mouse reports already on their way when the modes were switched off would otherwise
/// reach the shell as typed text.
fn discard_pending_input() {
    while event::poll(Duration::from_millis(50)).unwrap_or(false) {
        let _unused = event::read();
    }
}
