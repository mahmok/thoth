use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;
use std::io::{self, Write};

/// Terminal modes held while the app runs.
pub struct Modes<W: Write> {
    out: W,
}

impl<W: Write> Modes<W> {
    pub fn enter(out: W) -> io::Result<Self> {
        // Build the guard first, so a failure below still runs `drop`.
        let mut modes = Self { out };
        execute!(modes.out, EnableMouseCapture, EnableBracketedPaste)?;
        Ok(modes)
    }
}

impl<W: Write> Drop for Modes<W> {
    fn drop(&mut self) {
        let _unused = execute!(self.out, DisableMouseCapture, DisableBracketedPaste);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use alloc::fmt;
    use core::panic::AssertUnwindSafe;
    #[cfg(unix)]
    use std::io::IoSlice;
    use std::panic;

    #[cfg(unix)]
    const MOUSE_ON: &str = "\x1b[?1000h";
    #[cfg(unix)]
    const MOUSE_OFF: &str = "\x1b[?1000l";
    const PASTE_ON: &str = "\x1b[?2004h";
    const PASTE_OFF: &str = "\x1b[?2004l";

    /// A writer whose first flush fails, like a terminal that goes away while the modes are being set.
    #[cfg(unix)]
    struct FlushFailsOnce {
        bytes: Vec<u8>,
        failed: bool,
    }

    #[cfg(unix)]
    impl Write for FlushFailsOnce {
        fn by_ref(&mut self) -> &mut Self
        where
            Self: Sized,
        {
            self
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.failed {
                return Ok(());
            }
            self.failed = true;
            Err(io::Error::other("terminal went away"))
        }

        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            self.bytes.extend_from_slice(buf);
            Ok(())
        }

        fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
            self.bytes.extend_from_slice(fmt::format(args).as_bytes());
            Ok(())
        }

        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
            let mut total = 0usize;
            for buf in bufs {
                self.bytes.extend_from_slice(buf);
                total = total.saturating_add(buf.len());
            }
            Ok(total)
        }
    }

    fn written(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// The modes that were switched on. Mouse capture is a `WinAPI` call on Windows, so it writes no
    /// ANSI there; bracketed paste is written on every platform.
    fn assert_entered(text: &str) {
        assert!(text.contains(PASTE_ON), "{text:?}");
        #[cfg(unix)]
        assert!(text.contains(MOUSE_ON), "{text:?}");
    }

    /// The modes that were switched off, paired with [`assert_entered`].
    fn assert_left(text: &str) {
        assert!(text.contains(PASTE_OFF), "{text:?}");
        #[cfg(unix)]
        assert!(text.contains(MOUSE_OFF), "{text:?}");
    }

    #[test]
    fn entering_turns_both_modes_on() {
        let mut out = Vec::new();
        let modes = Modes::enter(&mut out).unwrap();
        let before_drop = written(modes.out);
        assert_entered(&before_drop);
        assert!(!before_drop.contains(PASTE_OFF));
        #[cfg(unix)]
        assert!(!before_drop.contains(MOUSE_OFF));
    }

    #[test]
    fn dropping_turns_both_modes_off() {
        let mut out = Vec::new();
        drop(Modes::enter(&mut out).unwrap());
        let all = written(&out);
        assert_left(&all);
        assert!(all.rfind(PASTE_OFF) > all.rfind(PASTE_ON), "off comes after on");
        #[cfg(unix)]
        assert!(all.rfind(MOUSE_OFF) > all.rfind(MOUSE_ON), "off comes after on");
    }

    #[test]
    fn a_panic_still_turns_the_modes_off() {
        let mut out = Vec::new();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let _modes = Modes::enter(&mut out).unwrap();
            panic!("boom");
        }));
        assert!(result.is_err());
        assert_left(&written(&out));
    }

    /// The mouse modes are WinAPI calls on Windows and crossterm flushes before each, so an ANSI
    /// writer cannot observe this failure there.
    #[cfg(unix)]
    #[test]
    fn a_failed_start_still_switches_everything_back_off() {
        let mut out = FlushFailsOnce { bytes: Vec::new(), failed: false };
        assert!(Modes::enter(&mut out).is_err());
        let all = written(&out.bytes);
        assert_entered(&all);
        assert_left(&all);
    }
}
