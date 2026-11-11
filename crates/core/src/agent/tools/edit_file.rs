use crate::fsutil;
use alloc::collections::BTreeSet;
use core::fmt::Write as _;
use core::ops::Range;
use std::fs;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use similar::TextDiff;

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits, blocking};
use crate::workspace::{Workspace, truncate};

/// Lines of context around a change in the diff.
const CONTEXT_LINES: usize = 3;
/// The diff only confirms the edit; the model knows what it asked for, and the result is
/// sent again with every later step.
const MAX_DIFF_BYTES: usize = 5_000;
/// How many lines of the closest match an error shows.
const MAX_HINT_LINES: usize = 12;
/// How many places in the file are compared with a `oldText` that was not found.
const MAX_CANDIDATES: usize = 5;
/// How alike (0 to 1) text must be to be offered as the closest match.
const MIN_SIMILARITY: f32 = 0.6;
/// Files with more lines than this get no closest-match search.
const MAX_SEARCHED_LINES: usize = 20_000;
/// How much of a malformed `edits` string an error quotes back to the model.
const MAX_QUOTED_CHARS: usize = 60;

/// Surgical edits: each one replaces a single exact occurrence of `oldText`.
#[derive(Debug)]
pub(super) struct EditFile {
    workspace: Workspace,
}

impl EditFile {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

/// The arguments models send in practice are more varied than the schema: one edit without the
/// list, and `old_string` or `old_text` for `oldText`. They are all accepted.
#[derive(Debug, Deserialize)]
pub(super) struct EditFileArgs {
    #[serde(default, deserialize_with = "edits_list")]
    edits: Option<Vec<Edit>>,
    #[serde(
        default,
        rename = "newText",
        alias = "new_text",
        alias = "new_string",
        alias = "new_str",
        alias = "newString",
        alias = "newStr"
    )]
    new_text: Option<String>,
    #[serde(
        default,
        rename = "oldText",
        alias = "old_text",
        alias = "old_string",
        alias = "old_str",
        alias = "oldString",
        alias = "oldStr"
    )]
    old_text: Option<String>,
    path: String,
}

#[derive(Debug, Clone, Deserialize)]
/// One edit of `edit_file`.
pub(super) struct Edit {
    #[serde(
        rename = "newText",
        alias = "new_text",
        alias = "new_string",
        alias = "new_str",
        alias = "newString",
        alias = "newStr"
    )]
    new_text: String,
    #[serde(
        rename = "oldText",
        alias = "old_text",
        alias = "old_string",
        alias = "old_str",
        alias = "oldString",
        alias = "oldStr"
    )]
    old_text: String,
    #[serde(default, rename = "replaceAll", alias = "replace_all")]
    replace_all: bool,
}

impl EditFileArgs {
    fn edits(&self) -> Result<Vec<Edit>, String> {
        match (self.edits.as_ref(), self.old_text.as_ref(), self.new_text.as_ref()) {
            (Some(edits), _, _) if !edits.is_empty() => Ok(edits.clone()),
            (Some(_), _, _) => Err("no edits given".into()),
            (None, Some(old), Some(new)) => {
                Ok(vec![Edit { old_text: old.clone(), new_text: new.clone(), replace_all: false }])
            },
            (None, _, _) => Err("no edits given: pass `edits`, a list of { oldText, newText } objects".into()),
        }
    }
}

impl Tool for EditFile {
    type Args = EditFileArgs;

    fn description(&self) -> &'static str {
        "Make surgical edits to an existing text file; to create a file use write_file. Each edit replaces one exact occurrence of oldText with newText. \
         For one change, pass path, oldText and newText directly. For several changes in one file, pass path and edits: an array of { oldText, newText } objects (a real array, not a string). \
         Copy oldText exactly from the file as read_file shows it, including indentation. \
         Every oldText is matched against the original file, must occur exactly once (or set replaceAll to change every occurrence), and must not overlap another edit. \
         Include enough surrounding lines in oldText to make it unique. The file is left unchanged if any edit fails, and the error lists every edit that failed. \
         Paths are relative to the working directory and must stay inside it."
    }

    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to edit: relative to the working directory." },
                "oldText": { "type": "string", "description": "For a single change: exact text to find. Must match exactly once in the file. Use this with newText instead of edits." },
                "newText": { "type": "string", "description": "For a single change: text to replace oldText with (may be empty to delete)." },
                "edits": {
                    "type": "array",
                    "minItems": 1,
                    "description": "For several changes: edits to apply, all matched against the original file. An array of objects, not a string.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": { "type": "string", "description": "Exact text to find. Must match exactly once in the file." },
                            "newText": { "type": "string", "description": "Text to replace it with (may be empty to delete)." },
                            "replaceAll": { "type": "boolean", "description": "Replace every occurrence of oldText instead of requiring exactly one. Default false." }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path"]
        })
    }

    async fn run(&self, args: EditFileArgs, _cx: &ToolContext) -> ToolResult {
        let workspace = self.workspace.clone();
        blocking(move || edit(&workspace, &args)).await
    }

    fn summary(&self, args: &EditFileArgs) -> String {
        let count = args.edits().map_or(0, |edits| edits.len());
        format!("{} ({count} edit{})", args.path, plural(count))
    }

    #[inline]
    fn traits(&self) -> ToolTraits {
        ToolTraits::default()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

/// Where one edit lands in the original text.
struct Span {
    /// The edit's number in the call, from 1.
    edit: usize,
    end: usize,
    new: String,
    start: usize,
}

/// The line endings of a file, so that model-supplied text can be made to match.
enum Eol {
    Crlf,
    Lf,
    /// Both kinds occur: the text is tried as given, then in each kind.
    Mixed,
}

impl Eol {
    /// The `oldText` and `newText` to use, and where `oldText` starts in `text`. Matches may
    /// overlap, so that an ambiguous `oldText` is never taken for a unique one.
    fn locate(&self, text: &str, edit: &Edit) -> (String, String, Vec<usize>) {
        let lf = |s: &str| s.replace("\r\n", "\n");
        let crlf = |s: &str| lf(s).replace('\n', "\r\n");
        let versions: Vec<(String, String)> = match *self {
            Self::Lf => vec![(lf(&edit.old_text), lf(&edit.new_text))],
            Self::Crlf => vec![(crlf(&edit.old_text), crlf(&edit.new_text))],
            Self::Mixed => vec![
                (edit.old_text.clone(), edit.new_text.clone()),
                (lf(&edit.old_text), lf(&edit.new_text)),
                (crlf(&edit.old_text), crlf(&edit.new_text)),
            ],
        };
        for version in &versions {
            let starts = find_all(text, &version.0);
            if !starts.is_empty() {
                return (version.0.clone(), version.1.clone(), starts);
            }
        }
        let (old, new) = versions.into_iter().next().unwrap_or_default();
        (old, new, Vec::new())
    }

    fn of(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let bare_lf = text.replace("\r\n", "").contains('\n');
        match (crlf, bare_lf) {
            (true, false) => Self::Crlf,
            (true, true) => Self::Mixed,
            _ => Self::Lf,
        }
    }
}

/// The closest match to text that was not found.
struct Closest {
    first_line: usize,
    last_line: usize,
    shown: String,
    similarity: f32,
    whitespace_only: bool,
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// `edits` is a list; some models send it as text holding the list. Text that is valid JSON is
/// parsed, and text that is not gets an error that says so, because the model cannot tell from
/// "expected a sequence" that the JSON inside the text is what is broken.
fn edits_list<'de, D>(deserializer: D) -> Result<Option<Vec<Edit>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => match serde_json::from_str::<Vec<Edit>>(&text) {
            Ok(edits) => Ok(Some(edits)),
            Err(parse_error) => {
                let start: String = text.chars().take(MAX_QUOTED_CHARS).collect();
                Err(D::Error::custom(format!(
                    "`edits` must be a JSON array of {{ oldText, newText }} objects, but it was \
                     sent as text that is not valid JSON ({parse_error}); the text starts `{start}`. Send \
                     `edits` as a real array, not as a string. For a single change, skip `edits` \
                     and pass `oldText` and `newText` directly"
                )))
            },
        },
        Some(other) => Vec::<Edit>::deserialize(other).map(Some).map_err(D::Error::custom),
    }
}

/// Applies the edits of an `edit_file` call: all of them or none.
fn edit(workspace: &Workspace, args: &EditFileArgs) -> Result<String, String> {
    let edits = args.edits()?;
    let path = args.path.as_str();
    let full = workspace.resolve(path)?;
    if !full.is_file() {
        return Err(format!("`{path}` is not a file"));
    }
    let bytes = fs::read(&full).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    if bytes.contains(&0) {
        return Err(format!("`{path}` looks like a binary file"));
    }
    let text = String::from_utf8(bytes).map_err(|_utf8_error| format!("`{path}` is not valid UTF-8"))?;
    let eol = Eol::of(&text);

    // Locate every edit against the original text before changing anything, and keep going
    // after a failure: the model should learn about all of them at once.
    let mut spans: Vec<Span> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let number = index.saturating_add(1);
        if edit.old_text.is_empty() {
            problems.push(format!("edit {number}: oldText must not be empty (to create a file, use write_file)"));
            continue;
        }
        let (old, new, starts) = eol.locate(&text, edit);
        match *starts.as_slice() {
            [] => problems.push(format!(
                "edit {number}: {}",
                not_found(&text, path, &edit.old_text)
            )),
            [start] => spans.push(Span {
                edit: number,
                start,
                end: start.saturating_add(old.len()),
                new,
            }),
            ref many if edit.replace_all => {
                let mut free_from = 0;
                for &start in many {
                    if start < free_from {
                        continue;
                    }
                    free_from = start.saturating_add(old.len());
                    spans.push(Span {
                        edit: number,
                        start,
                        end: free_from,
                        new: new.clone(),
                    });
                }
            }
            ref many => problems.push(format!(
                "edit {number}: oldText matches {} times in `{path}` (at {}); include more surrounding lines to make it unique, or set replaceAll to change every occurrence",
                many.len(),
                line_list(&text, many)
            )),
        }
    }

    spans.sort_by_key(|span| span.start);
    for pair in spans.windows(2) {
        if let (Some(first), Some(second)) = (pair.first(), pair.get(1))
            && first.end > second.start
        {
            problems.push(format!(
                "edits {} and {} overlap (around line {}); merge them into one edit",
                first.edit,
                second.edit,
                line_of(&text, second.start)
            ));
        }
    }

    if !problems.is_empty() {
        return Err(format!(
            "no edits were applied and the file is unchanged. {} problem{}:\n{}",
            problems.len(),
            plural(problems.len()),
            problems.join("\n")
        ));
    }

    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for span in &spans {
        output.push_str(unchanged(&text, cursor..span.start, span.edit)?);
        output.push_str(&span.new);
        cursor = span.end;
    }
    output.push_str(unchanged(&text, cursor..text.len(), spans.last().map_or(0, |span| span.edit))?);

    if output == text {
        return Err("no changes: every newText equals its oldText".into());
    }
    fsutil::write_replacing(&full, output.as_bytes()).map_err(|e| format!("cannot write `{path}`: {e}"))?;

    let count = spans.len();
    let shown = workspace.display_path(&full);
    let diff = unified_diff(&text, &output, &shown.to_string());
    Ok(truncate(format!("edited {shown} ({count} change{})\n{diff}", plural(count)), MAX_DIFF_BYTES))
}

/// The part of `text` between two edits. A range that does not fit the text is an error and not
/// a shorter file: the output replaces the file.
fn unchanged(text: &str, range: Range<usize>, edit: usize) -> Result<&str, String> {
    text.get(range).ok_or_else(|| format!("edit {edit} does not fit the file; read the file again"))
}

/// Every place `needle` starts, including ones that overlap an earlier one.
fn find_all(text: &str, needle: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(found) = text.get(from..).and_then(|rest| rest.find(needle)) {
        let at = from.saturating_add(found);
        starts.push(at);
        from = at.saturating_add(text.get(at..).and_then(|rest| rest.chars().next()).map_or(1, char::len_utf8));
    }
    starts
}

fn line_of(text: &str, byte: usize) -> usize {
    text.get(..byte).unwrap_or_default().matches('\n').count().saturating_add(1)
}

fn line_list(text: &str, starts: &[usize]) -> String {
    let mut lines: Vec<String> = starts.iter().take(6).map(|&at| format!("line {}", line_of(text, at))).collect();
    if starts.len() > 6 {
        lines.push("...".into());
    }
    lines.join(", ")
}

/// Why `old` was not found, with the closest text in the file when there is some. Most failures
/// are a difference in whitespace or a file that changed since it was read, and showing the
/// real text lets the model fix the call in one step.
#[expect(clippy::float_arithmetic, reason = "the similarity is shown as a percentage")]
fn not_found(text: &str, path: &str, old: &str) -> String {
    let mut message = format!("oldText not found in `{path}`");
    match closest(text, old) {
        Some(near) => {
            let _written = write!(message, ". The closest text is at lines {}-{} ({:.0}% alike{}):\n{}",
                near.first_line,
                near.last_line,
                near.similarity * 100.0,
                if near.whitespace_only {
                    "; it differs only in whitespace such as indentation, tabs or trailing spaces"
                } else {
                    ""
                },
                near.shown);
        }
        None => message.push_str(
            ". Nothing similar is in the file; it may have changed since you read it, so read it again and copy the text exactly",
        ),
    }
    message
}

/// The lines of `text` most like `old`, to show when `old` was not found.
fn closest(text: &str, old: &str) -> Option<Closest> {
    let file: Vec<&str> = text.lines().collect();
    let wanted: Vec<&str> = old.lines().collect();
    if file.is_empty() || wanted.is_empty() || file.len() > MAX_SEARCHED_LINES {
        return None;
    }
    let wanted_trim: Vec<&str> = wanted.iter().map(|line| line.trim()).collect();
    // The longest line says the most about where the text should be.
    let (anchor_at, anchor) = wanted_trim.iter().enumerate().max_by_key(|&(_, line)| line.len())?;
    if anchor.is_empty() {
        return None;
    }

    let mut scored: Vec<(f32, usize)> = file
        .iter()
        .enumerate()
        .filter_map(|(index, raw_line)| {
            let line = raw_line.trim();
            let near_in_length =
                line.len().saturating_mul(2) >= anchor.len() && line.len() <= anchor.len().saturating_mul(2);
            near_in_length.then(|| (line_similarity(line, anchor), index))
        })
        .filter(|&(score, _)| score >= MIN_SIMILARITY)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let starts: BTreeSet<usize> =
        scored.iter().take(MAX_CANDIDATES).map(|&(_, index)| index.saturating_sub(anchor_at)).collect();

    let wanted_joined = wanted_trim.join("\n");
    let (similarity, start) = starts
        .into_iter()
        .map(|start| {
            let end = start.saturating_add(wanted.len()).min(file.len());
            let window: Vec<&str> = file.get(start..end).unwrap_or_default().iter().map(|l| l.trim()).collect();
            (line_similarity(&window.join("\n"), &wanted_joined), start)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))?;
    if similarity < MIN_SIMILARITY {
        return None;
    }
    let end = start.saturating_add(wanted.len()).min(file.len());
    let window = file.get(start..end).unwrap_or_default();
    let shortened: Vec<String> =
        window.iter().take(MAX_HINT_LINES).map(|line| line.chars().take(200).collect::<String>()).collect();
    let mut shown = shortened.join("\n");
    if window.len() > MAX_HINT_LINES {
        shown.push_str("\n...");
    }
    Some(Closest {
        first_line: start.saturating_add(1),
        last_line: end,
        similarity,
        whitespace_only: window.iter().map(|l| l.trim()).eq(wanted_trim.iter().copied()),
        shown,
    })
}

/// How alike two texts are, from 0 to 1. Long texts are compared by line, which is cheaper.
fn line_similarity(a: &str, b: &str) -> f32 {
    if a.len().max(b.len()) > 4_000 { TextDiff::from_lines(a, b).ratio() } else { TextDiff::from_chars(a, b).ratio() }
}

/// A unified diff between the original and edited text, labelled with the file path.
fn unified_diff(before: &str, after: &str, path: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;

    fn tool(tree: &TempTree) -> EditFile {
        EditFile::new(tree.workspace())
    }

    #[tokio::test]
    async fn replaces_a_unique_span() {
        let tree = TempTree::new("ef-one").file("a.txt", "one\ntwo\nthree\n");
        let result =
            run(tool(&tree), json!({ "path": "a.txt", "edits": [{ "oldText": "two", "newText": "2" }] })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(fs::read_to_string(tree.path().join("a.txt")).unwrap(), "one\n2\nthree\n");
        assert!(result.content.contains("-two\n+2\n"), "{}", result.content);
        assert!(result.content.contains("--- a/a.txt\n+++ b/a.txt"), "{}", result.content);
    }

    #[tokio::test]
    async fn long_diffs_are_truncated() {
        let original: String = (0i32..5_000i32).fold(String::new(), |mut acc, i| {
            let _written = writeln!(acc, "line {i}");
            acc
        });
        let tree = TempTree::new("ef-big").file("a.txt", &original);
        let replaced: String = (0i32..5_000i32).fold(String::new(), |mut acc, i| {
            let _written = writeln!(acc, "row {i} changed");
            acc
        });
        let edits = json!([{ "oldText": original, "newText": replaced }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("[truncated"));
        assert!(result.content.len() < 6_000);
    }

    #[tokio::test]
    async fn applies_several_edits_against_the_original() {
        let tree = TempTree::new("ef-many").file("a.txt", "alpha beta gamma");
        let edits = json!([
            { "oldText": "alpha", "newText": "A" },
            { "oldText": "gamma", "newText": "" }
        ]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(fs::read_to_string(tree.path().join("a.txt")).unwrap(), "A beta ");
    }

    #[tokio::test]
    async fn missing_text_leaves_the_file_untouched() {
        let tree = TempTree::new("ef-missing").file("a.txt", "keep me");
        let edits = json!([
            { "oldText": "keep", "newText": "changed" },
            { "oldText": "absent", "newText": "x" }
        ]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.is_error);
        assert!(result.content.contains("edit 2"), "{}", result.content);
        assert_eq!(fs::read_to_string(tree.path().join("a.txt")).unwrap(), "keep me");
    }

    #[tokio::test]
    async fn ambiguous_text_is_refused() {
        let tree = TempTree::new("ef-ambiguous").file("a.txt", "x x");
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": [{ "oldText": "x", "newText": "y" }] })).await;
        assert!(result.is_error);
        assert!(result.content.contains("matches 2 times"), "{}", result.content);
        assert_eq!(fs::read_to_string(tree.path().join("a.txt")).unwrap(), "x x");
    }

    #[tokio::test]
    async fn overlapping_edits_are_refused() {
        let tree = TempTree::new("ef-overlap").file("a.txt", "abcdef");
        let edits = json!([
            { "oldText": "abcd", "newText": "1" },
            { "oldText": "cdef", "newText": "2" }
        ]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.is_error);
        assert!(result.content.contains("overlap"), "{}", result.content);
    }

    #[tokio::test]
    async fn crlf_files_keep_crlf_endings() {
        let tree = TempTree::new("ef-crlf").file("a.txt", "one\r\ntwo\r\n");
        let edits = json!([{ "oldText": "one\ntwo", "newText": "1\n2\nthree" }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(fs::read_to_string(tree.path().join("a.txt")).unwrap(), "1\r\n2\r\nthree\r\n");
    }

    #[tokio::test]
    async fn empty_old_text_and_no_op_edits_are_errors() {
        let tree = TempTree::new("ef-noop").file("a.txt", "same");
        let empty = run(tool(&tree), json!({ "path": "a.txt", "edits": [{ "oldText": "", "newText": "x" }] })).await;
        assert!(empty.is_error);
        let noop =
            run(tool(&tree), json!({ "path": "a.txt", "edits": [{ "oldText": "same", "newText": "same" }] })).await;
        assert!(noop.is_error);
    }

    #[tokio::test]
    async fn refuses_paths_outside_the_workspace() {
        let outside = TempTree::new("ef-outside").file("o.txt", "far");
        let tree = TempTree::new("ef-inside");
        let path = outside.path().join("o.txt");
        let result = run(
            tool(&tree),
            json!({ "path": path.to_str().unwrap(), "edits": [{ "oldText": "far", "newText": "near" }] }),
        )
        .await;
        assert!(result.is_error);
        assert_eq!(fs::read_to_string(path).unwrap(), "far");
    }

    #[tokio::test]
    async fn rejects_missing_files_and_empty_edit_lists() {
        let tree = TempTree::new("ef-bad");
        let missing =
            run(tool(&tree), json!({ "path": "nope.txt", "edits": [{ "oldText": "a", "newText": "b" }] })).await;
        assert!(missing.is_error);
        let none = run(tool(&tree), json!({ "path": "x", "edits": [] })).await;
        assert!(none.is_error);
    }

    fn read(tree: &TempTree, name: &str) -> String {
        fs::read_to_string(tree.path().join(name)).unwrap()
    }

    #[tokio::test]
    async fn every_failing_edit_is_reported_at_once_and_nothing_is_written() {
        let tree = TempTree::new("ef-all").file("a.txt", "alpha\nbeta\ngamma\n");
        let edits = json!([
            { "oldText": "alpha", "newText": "A" },
            { "oldText": "missing one", "newText": "x" },
            { "oldText": "missing two", "newText": "y" }
        ]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.is_error);
        assert!(result.content.contains("edit 2:"), "{}", result.content);
        assert!(result.content.contains("edit 3:"), "{}", result.content);
        assert!(!result.content.contains("edit 1:"), "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "alpha\nbeta\ngamma\n");
    }

    #[tokio::test]
    async fn a_whitespace_difference_is_named_and_the_real_text_is_shown() {
        let tree = TempTree::new("ef-ws").file("a.rs", "fn main() {\n\tlet x = 1;\n\tlet y = 2;\n}\n");
        let edits = json!([{ "oldText": "    let x = 1;\n    let y = 2;", "newText": "" }]);
        let result = run(tool(&tree), json!({ "path": "a.rs", "edits": edits })).await;
        assert!(result.is_error);
        assert!(
            result.content.contains("only in whitespace") && result.content.contains("lines 2-3"),
            "{}",
            result.content
        );
        assert!(result.content.contains("\tlet x = 1;"), "the real text: {}", result.content);
    }

    #[tokio::test]
    async fn a_small_difference_points_at_the_closest_text() {
        let body = "line one\nlet total = compute(a, b);\nline three\n";
        let tree = TempTree::new("ef-near").file("a.rs", body);
        let edits = json!([{ "oldText": "let total = compute(a, c);", "newText": "x" }]);
        let result = run(tool(&tree), json!({ "path": "a.rs", "edits": edits })).await;
        assert!(result.is_error);
        assert!(result.content.contains("lines 2-2") && result.content.contains("compute(a, b)"), "{}", result.content);
    }

    #[tokio::test]
    async fn nothing_similar_says_to_read_the_file_again() {
        let tree = TempTree::new("ef-far").file("a.txt", "apple\n");
        let edits = json!([{ "oldText": "zzzzzzzzzzzzzzzz", "newText": "x" }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.content.contains("read it again"), "{}", result.content);
    }

    #[tokio::test]
    async fn matches_that_overlap_each_other_are_still_ambiguous() {
        let tree = TempTree::new("ef-overlapping-matches").file("a.txt", "}\n}\n}\n");
        let edits = json!([{ "oldText": "}\n}", "newText": "x" }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.is_error);
        assert!(result.content.contains("matches 2 times"), "{}", result.content);
        assert!(result.content.contains("line 1, line 2"), "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "}\n}\n}\n");
    }

    #[tokio::test]
    async fn the_overlap_error_names_the_edits() {
        let tree = TempTree::new("ef-overlap-names").file("a.txt", "abcdef");
        let edits = json!([
            { "oldText": "abcd", "newText": "1" },
            { "oldText": "cdef", "newText": "2" }
        ]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.content.contains("edits 1 and 2 overlap"), "{}", result.content);
    }

    #[tokio::test]
    async fn replace_all_changes_every_occurrence() {
        let tree = TempTree::new("ef-all-occurrences").file("a.txt", "foo bar foo bar foo");
        let edits = json!([{ "oldText": "foo", "newText": "baz", "replaceAll": true }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "baz bar baz bar baz");
    }

    #[tokio::test]
    async fn argument_names_models_use_instead_are_accepted() {
        let tree = TempTree::new("ef-alias").file("a.txt", "one two");
        let edits = json!([{ "old_string": "one", "new_string": "1" }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "1 two");
    }

    #[tokio::test]
    async fn a_single_edit_without_the_list_is_accepted() {
        let tree = TempTree::new("ef-single").file("a.txt", "one two");
        let result = run(tool(&tree), json!({ "path": "a.txt", "old_text": "two", "new_text": "2" })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "one 2");
    }

    #[tokio::test]
    async fn a_list_of_edits_sent_as_json_text_is_accepted() {
        let tree = TempTree::new("ef-text-list").file("a.txt", "one two");
        let edits = json!([{ "oldText": "one", "newText": "1" }]).to_string();
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "1 two");
    }

    #[tokio::test]
    async fn one_change_can_be_sent_without_the_list() {
        let tree = TempTree::new("ef-flat").file("a.txt", "one two");
        let result = run(tool(&tree), json!({ "path": "a.txt", "oldText": "two", "newText": "2" })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "one 2");
    }

    #[test]
    fn the_schema_asks_only_for_the_path_and_offers_the_single_change_form() {
        let tree = TempTree::new("ef-params");
        let schema = tool(&tree).parameters();
        assert_eq!(schema["required"], json!(["path"]));
        assert!(schema["properties"]["oldText"].is_object());
        assert!(schema["properties"]["newText"].is_object());
    }

    #[tokio::test]
    async fn a_list_sent_as_broken_json_text_says_the_text_is_not_valid_json() {
        let tree = TempTree::new("ef-broken-text").file("a.txt", "one two");
        // What a model sent in practice: `": "` collapsed into `>`.
        let edits = r#"[{"oldText">one", "newText">1"}]"#;
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.is_error);
        assert!(result.content.contains("not valid JSON"), "{}", result.content);
        assert!(result.content.contains(r#"[{"oldText">one"#), "{}", result.content);
        assert!(result.content.contains("`oldText` and `newText` directly"), "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "one two");
    }

    #[tokio::test]
    async fn a_broken_text_is_quoted_only_in_part() {
        let tree = TempTree::new("ef-long-text").file("a.txt", "x");
        let edits = format!("[{{\"oldText\">{}", "z".repeat(5_000));
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": edits })).await;
        assert!(result.is_error);
        assert!(!result.content.contains(&"z".repeat(MAX_QUOTED_CHARS + 1)), "{}", result.content);
    }

    #[tokio::test]
    async fn bad_arguments_come_back_with_the_schema() {
        let tree = TempTree::new("ef-schema").file("a.txt", "x");
        let result = run(tool(&tree), json!({ "file": "a.txt" })).await;
        assert!(result.is_error);
        assert!(result.content.contains("oldText"), "{}", result.content);
    }

    #[tokio::test]
    async fn a_file_with_both_line_endings_matches_either_kind() {
        let tree = TempTree::new("ef-mixed").file("a.txt", "a\r\nb\r\nc\nd\n");
        let crlf = json!([{ "oldText": "a\nb", "newText": "A\nB" }]);
        let result = run(tool(&tree), json!({ "path": "a.txt", "edits": crlf })).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(read(&tree, "a.txt"), "A\r\nB\r\nc\nd\n");
        let lf = json!([{ "oldText": "c\nd", "newText": "C\nD" }]);
        let second = run(tool(&tree), json!({ "path": "a.txt", "edits": lf })).await;
        assert!(!second.is_error, "{}", second.content);
        assert_eq!(read(&tree, "a.txt"), "A\r\nB\r\nC\nD\n");
    }
}
