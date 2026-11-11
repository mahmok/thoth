use std::io::{self, Write as _};
use std::process::{Command, Stdio};

/// Programs that put text on the system clipboard, tried in order until one works.
const NATIVE: &[(&str, &[&str])] = &[
    ("pbcopy", &[]),
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("clip", &[]),
];

/// Copies with the system clipboard tool when there is one, and always asks the terminal too
/// (OSC 52), which is what reaches the local clipboard over SSH.
pub fn copy(text: &str) -> io::Result<()> {
    let native = NATIVE.iter().any(|&(program, args)| pipe(program, args, text));
    let terminal = copy_through_terminal(text);
    if native { Ok(()) } else { terminal }
}

fn pipe(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) =
        Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else {
        return false;
    };
    let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok_and(|status| status.success()) && wrote
}

fn copy_through_terminal(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

/// Encodes bytes as base64.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3).saturating_mul(4));
    for chunk in input.chunks(3) {
        let byte = |at: usize| u32::from(chunk.get(at).copied().unwrap_or(0));
        let n = (byte(0) << 16i32) | (byte(1) << 8i32) | byte(2);
        for i in 0..4 {
            if i <= chunk.len() {
                let index =
                    usize::try_from((n >> 18usize.saturating_sub(6usize.saturating_mul(i))) & 63).unwrap_or(usize::MAX);
                out.push(ALPHABET.get(index).copied().map_or('=', char::from));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_is_not_a_copy() {
        assert!(!pipe("thoth-no-such-clipboard-tool", &[], "x"));
    }

    #[test]
    fn a_program_that_reads_its_input_counts_as_a_copy() {
        assert!(pipe("cat", &[], "x"));
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"hello world"), "aGVsbG8gd29ybGQ=");
    }
}
