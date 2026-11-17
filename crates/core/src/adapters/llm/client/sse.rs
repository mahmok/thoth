use alloc::collections::VecDeque;
use core::mem;
use reqwest::Response;

use super::ClientError;

#[derive(Default)]
/// Splits bytes into server-sent event payloads.
pub(super) struct Framer {
    buffer: Vec<u8>,
    scanned: usize,
}

impl Framer {
    pub(super) fn finish(&mut self) -> Option<String> {
        self.scanned = 0;
        payload(&mem::take(&mut self.buffer))
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut payloads = Vec::new();
        let (mut start, mut from) = (0, self.scanned);
        while let Some(offset) = self.buffer.get(from..).and_then(|rest| rest.iter().position(|&b| b == b'\n')) {
            let end = from.saturating_add(offset);
            payloads.extend(self.buffer.get(start..end).and_then(payload));
            start = end.saturating_add(1);
            from = start;
        }
        // The consumed lines go in one move, not one per line.
        drop(self.buffer.drain(..start));
        self.scanned = self.buffer.len();
        payloads
    }
}

/// A response read as events.
pub(super) struct EventStream {
    ended: bool,
    framer: Framer,
    ready: VecDeque<String>,
    response: Response,
}

impl EventStream {
    pub(super) fn new(response: Response) -> Self {
        Self { response, framer: Framer::default(), ready: VecDeque::default(), ended: false }
    }

    /// The next event payload, or `None` when the stream ends.
    pub(super) async fn next(&mut self) -> Result<Option<String>, ClientError> {
        loop {
            if let Some(data) = self.ready.pop_front() {
                return Ok(Some(data));
            }
            if self.ended {
                return Ok(None);
            }
            if let Some(bytes) = self.response.chunk().await? {
                self.ready.extend(self.framer.push(&bytes));
            } else {
                self.ended = true;
                self.ready.extend(self.framer.finish());
            }
        }
    }
}

fn payload(line: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    let data = text.trim().strip_prefix("data:")?.trim();
    (!data.is_empty()).then(|| data.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::assert_empty;
    use core::fmt::Write as _;

    #[test]
    fn payloads_are_reassembled_across_chunk_boundaries() {
        let mut framer = Framer::default();
        assert_empty(framer.push(b"event: x\nda"));
        assert_eq!(framer.push(b"ta: {\"a\":1}\n\ndata: {\"b\""), ["{\"a\":1}"]);
        assert_eq!(framer.push(b":2}\n"), ["{\"b\":2}"]);
    }

    #[test]
    fn crlf_endings_and_other_lines_are_handled() {
        let mut framer = Framer::default();
        let got = framer.push(b"event: e\r\ndata: one\r\n: comment\r\ndata:two\r\ndata:\r\n");
        assert_eq!(got, ["one", "two"]);
    }

    #[test]
    fn an_unterminated_last_line_is_flushed_at_the_end() {
        let mut framer = Framer::default();
        assert_empty(framer.push(b"data: [DONE]"));
        assert_eq!(framer.finish().as_deref(), Some("[DONE]"));
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn a_long_line_in_many_chunks_and_many_lines_in_one_chunk_come_out_whole() {
        let mut framer = Framer::default();
        let long = "x".repeat(10_000);
        let line = format!("data: {long}\n");
        let mut out = Vec::new();
        for chunk in line.as_bytes().chunks(7) {
            out.extend(framer.push(chunk));
        }
        assert_eq!(out, [long]);

        let many: String = (0i32..1_000i32).fold(String::new(), |mut acc, i| {
            let _written = writeln!(acc, "data: {i}");
            acc
        });
        let got = framer.push(many.as_bytes());
        assert_eq!(got.len(), 1_000);
        assert_eq!((got[0].as_str(), got[999].as_str()), ("0", "999"));
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn multibyte_text_split_across_chunks_survives() {
        let bytes = "data: \u{e9}\n".as_bytes();
        let mut framer = Framer::default();
        assert_empty(framer.push(&bytes[..7]));
        assert_eq!(framer.push(&bytes[7..]), ["\u{e9}"]);
    }
}
