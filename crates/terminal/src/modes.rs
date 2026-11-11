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
        let _ = execute!(modes.out, EnableMouseCapture, EnableBracketedPaste);
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
    use alloc::fmt;
    use core::panic::AssertUnwindSafe;
    use std::io::IoSlice;
    use std::panic;

    const MOUSE_ON: &str = "\x1b[?1000h";
    const MOUSE_OFF: &str = "\x1b[?1000l";
    const PASTE_ON: &str = "\x1b[?2004h";
    const PASTE_OFF: &str = "\x1b[?2004l";

    /// A writer whose first flush fails, like a terminal that goes away while the modes are being set.
    struct FlushFailsOnce {
        bytes: Vec<u8>,
        failed: bool,
    }

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

    #[test]
    fn entering_turns_both_modes_on() {
        let mut out = Vec::new();
        let modes = Modes::enter(&mut out).unwrap();
        let before_drop = written(modes.out);
        assert!(before_drop.contains(MOUSE_ON) && before_drop.contains(PASTE_ON), "{before_drop:?}");
        assert!(!before_drop.contains(MOUSE_OFF) && !before_drop.contains(PASTE_OFF));
    }

    #[test]
    fn dropping_turns_both_modes_off() {
        let mut out = Vec::new();
        drop(Modes::enter(&mut out).unwrap());
        let all = written(&out);
        assert!(all.contains(MOUSE_OFF) && all.contains(PASTE_OFF), "{all:?}");
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
        let all = written(&out);
        assert!(all.contains(MOUSE_OFF) && all.contains(PASTE_OFF), "{all:?}");
    }

    #[test]
    fn a_failed_start_still_switches_everything_back_off() {
        let mut out = FlushFailsOnce { bytes: Vec::new(), failed: false };
        assert!(Modes::enter(&mut out).is_err());
        let all = written(&out.bytes);
        assert!(all.contains(MOUSE_ON), "{all:?}");
        assert!(all.contains(MOUSE_OFF) && all.contains(PASTE_OFF), "{all:?}");
    }
}
