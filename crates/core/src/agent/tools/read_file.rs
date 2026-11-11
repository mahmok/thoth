use core::fmt::Write as _;
use serde::Deserialize;
use serde_json::{Value, json};

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits, blocking};
use crate::workspace::{MAX_READ_BYTES, ReadPolicy, Workspace, truncate};

/// The largest file `read_file` loads. It is read whole before being cut to what the model may see.
const MAX_FILE_BYTES: u64 = 20_000_000;
#[derive(Debug)]

/// The `read_file` Tool.
pub(super) struct ReadFile {
    workspace: Workspace,
}

impl ReadFile {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[derive(Debug, Deserialize)]
/// Arguments of `read_file`.
pub(super) struct ReadFileArgs {
    /// Last line to return, 1-based and inclusive.
    #[serde(default)]
    end_line: Option<usize>,
    path: String,
    /// First line to return, 1-based.
    #[serde(default)]
    start_line: Option<usize>,
}

impl Tool for ReadFile {
    type Args = ReadFileArgs;

    fn description(&self) -> &'static str {
        "Read a text file, from the working directory or anywhere else on disk (absolute paths work). Long files are cut off with a note that says which line to continue from; use start_line/end_line to read a part of a file."
    }

    fn name(&self) -> &'static str {
        "read_file"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path: relative to the working directory, or absolute." },
                "start_line": { "type": "integer", "description": "First line to return (1-based)." },
                "end_line": { "type": "integer", "description": "Last line to return (1-based, inclusive)." }
            },
            "required": ["path"]
        })
    }

    async fn run(&self, args: ReadFileArgs, _cx: &ToolContext) -> ToolResult {
        let workspace = self.workspace.clone();
        blocking(move || read(&workspace, &args)).await
    }

    fn summary(&self, args: &ReadFileArgs) -> String {
        args.path.clone()
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits::read_only()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

/// Runs `read_file`: a file, or a range of its lines.
fn read(workspace: &Workspace, args: &ReadFileArgs) -> Result<String, String> {
    let text = workspace.read(&args.path, ReadPolicy::anywhere_up_to(MAX_FILE_BYTES)).map_err(|refusal| {
        let message = refusal.to_string();
        if message.contains("is too big") {
            format!("{message}; look inside it with search_files, or with bash (head, tail, sed -n 'A,Bp')")
        } else {
            message
        }
    })?;
    let ranged = args.start_line.is_some() || args.end_line.is_some();
    if !ranged && text.len() <= MAX_READ_BYTES {
        return Ok(text);
    }

    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = args.start_line.unwrap_or(1).max(1);
    if let Some(end) = args.end_line
        && end < start
    {
        return Err(format!("end_line {end} is before start_line {start}"));
    }
    if start > total {
        return Err(format!(
            "start_line {start} is past the end: `{}` has {total} line{}",
            args.path,
            if total == 1 { "" } else { "s" }
        ));
    }
    let end = args.end_line.map_or(total, |end| end.min(total));

    // As many whole lines as fit. The model is told where to carry on, so it does not have to
    // work that out from a byte count.
    let mut body = String::new();
    let mut shown_end = start.saturating_sub(1);
    for line in lines.get(start.saturating_sub(1)..end).unwrap_or_default() {
        if shown_end >= start && body.len().saturating_add(line.len()).saturating_add(1) > MAX_READ_BYTES {
            break;
        }
        if shown_end >= start {
            body.push('\n');
        }
        body.push_str(line);
        shown_end = shown_end.saturating_add(1);
    }
    if shown_end == start && body.len() > MAX_READ_BYTES {
        // One line that is longer than everything allowed.
        body = truncate(body, MAX_READ_BYTES);
    }
    if shown_end < total {
        let _written = write!(
            body,
            "\n[showing lines {start}-{shown_end} of {total}; use start_line={} to read more]",
            shown_end.saturating_add(1)
        );
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;
    use std::fs::File;

    fn tool(tree: &TempTree) -> ReadFile {
        ReadFile::new(tree.workspace())
    }

    #[tokio::test]
    async fn reads_a_file() {
        let tree = TempTree::new("rf-read").file("a.txt", "one\ntwo\n");
        let result = run(tool(&tree), json!({ "path": "a.txt" })).await;
        assert_eq!(result, ToolResult::ok("one\ntwo\n"));
    }

    #[tokio::test]
    async fn reads_a_line_range() {
        let tree = TempTree::new("rf-range").file("a.txt", "one\ntwo\nthree\nfour\n");
        let result = run(tool(&tree), json!({ "path": "a.txt", "start_line": 2i32, "end_line": 4i32 })).await;
        assert_eq!(result, ToolResult::ok("two\nthree\nfour"));
    }

    #[tokio::test]
    async fn errors_are_results_the_model_can_read() {
        let tree = TempTree::new("rf-err");
        let missing = run(tool(&tree), json!({ "path": "nope.txt" })).await;
        assert!(missing.is_error);
        let directory = run(tool(&tree), json!({ "path": "." })).await;
        assert!(directory.is_error);
    }

    #[tokio::test]
    async fn reads_files_outside_the_workspace() {
        let outside = TempTree::new("rf-outside").file("o.txt", "far away");
        let tree = TempTree::new("rf-inside");
        let path = outside.path().join("o.txt");
        let result = run(tool(&tree), json!({ "path": path.to_str().unwrap() })).await;
        assert_eq!(result, ToolResult::ok("far away"));
    }

    #[tokio::test]
    async fn long_files_are_truncated() {
        let tree = TempTree::new("rf-long").file("big.txt", &"x".repeat(MAX_READ_BYTES + 10));
        let result = run(tool(&tree), json!({ "path": "big.txt" })).await;
        assert!(result.content.contains("[truncated"));
    }

    #[test]
    fn schema_example_parses_into_the_typed_arguments() {
        let args: ReadFileArgs =
            serde_json::from_value(json!({ "path": "a", "start_line": 1i32, "end_line": 2i32 })).unwrap();
        assert_eq!((args.start_line, args.end_line), (Some(1), Some(2)));
    }

    #[tokio::test]
    async fn a_range_that_leaves_more_says_where_to_carry_on() {
        let tree = TempTree::new("rf-more").file("a.txt", "one\ntwo\nthree\nfour\n");
        let result = run(tool(&tree), json!({ "path": "a.txt", "end_line": 2i32 })).await;
        assert_eq!(result, ToolResult::ok("one\ntwo\n[showing lines 1-2 of 4; use start_line=3 to read more]"));
        let rest = run(tool(&tree), json!({ "path": "a.txt", "start_line": 3i32 })).await;
        assert_eq!(rest, ToolResult::ok("three\nfour"));
    }

    #[tokio::test]
    async fn a_range_past_the_end_or_backwards_is_an_error() {
        let tree = TempTree::new("rf-bad-range").file("a.txt", "one\ntwo\n");
        let past = run(tool(&tree), json!({ "path": "a.txt", "start_line": 9i32 })).await;
        assert!(past.is_error);
        assert!(past.content.contains("has 2 lines"), "{}", past.content);
        let back = run(tool(&tree), json!({ "path": "a.txt", "start_line": 2i32, "end_line": 1i32 })).await;
        assert!(back.is_error);
    }

    #[tokio::test]
    async fn a_long_file_is_cut_at_a_line_and_says_which_line_to_continue_from() {
        let body: String = (1i32..=10_000i32).fold(String::new(), |mut acc, i| {
            let _written = writeln!(acc, "line {i}");
            acc
        });
        let tree = TempTree::new("rf-lines").file("a.txt", &body);
        let result = run(tool(&tree), json!({ "path": "a.txt" })).await;
        assert!(!result.is_error);
        let (shown, note) = result.content.rsplit_once("\n[showing").unwrap();
        let last = shown.lines().last().unwrap();
        assert!(last.starts_with("line ") && last.ends_with(char::is_numeric), "{last}");
        assert!(note.contains("of 10000; use start_line="), "{note}");
        let next: usize = last.trim_start_matches("line ").parse::<usize>().unwrap() + 1;
        assert!(note.contains(&format!("start_line={next}")), "{note}");
    }

    #[tokio::test]
    async fn a_range_is_cut_the_same_way() {
        let body: String = (1i32..=10_000i32).fold(String::new(), |mut acc, i| {
            let _written = writeln!(acc, "line {i}");
            acc
        });
        let tree = TempTree::new("rf-range-long").file("a.txt", &body);
        let result = run(tool(&tree), json!({ "path": "a.txt", "start_line": 5_000i32 })).await;
        assert!(result.content.starts_with("line 5000\n"));
        assert!(
            result.content.contains("showing lines 5000-"),
            "{}",
            result.content.get(result.content.len() - 90..).unwrap_or(&result.content)
        );
    }

    #[tokio::test]
    async fn a_file_too_big_to_load_says_how_to_look_inside_it() {
        let tree = TempTree::new("rf-huge");
        let file = File::create(tree.path().join("huge.log")).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        let result = run(tool(&tree), json!({ "path": "huge.log" })).await;
        assert!(result.is_error);
        assert!(result.content.contains("too big") && result.content.contains("search_files"), "{}", result.content);
    }
}
