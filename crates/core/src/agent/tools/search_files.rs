use core::fmt::Write as _;
use core::str;
use std::fs;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{Value, json};

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits, blocking};
use super::walk::{MAX_DEPTH, PathFilter, walk};
use crate::workspace::{Workspace, current_dir};

/// The most matching lines listed.
const MAX_MATCHES: usize = 100;
/// One file may not use up the whole budget, or the files after it would never show.
const MAX_MATCHES_PER_FILE: usize = 25;
/// Files larger than this are skipped.
const MAX_FILE_BYTES: u64 = 1_000_000;
/// Longer lines are cut to this.
const MAX_LINE_CHARS: usize = 200;

/// Searches file contents under a path for text or a regular expression.
#[derive(Debug)]
pub(super) struct SearchFiles {
    workspace: Workspace,
}

impl SearchFiles {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[derive(Debug, Deserialize)]
/// Arguments of `search_files`.
pub(super) struct SearchFilesArgs {
    #[serde(default)]
    glob: Option<String>,
    #[serde(default, alias = "case_insensitive")]
    ignore_case: bool,
    #[serde(default = "current_dir")]
    path: String,
    query: String,
    #[serde(default)]
    regex: bool,
}

impl Tool for SearchFiles {
    type Args = SearchFilesArgs;

    fn description(&self) -> &'static str {
        "Search file contents under a path. The query is literal, case-sensitive text unless regex is true (a Rust regular expression) or ignore_case is true. \
         Use glob (such as \"*.rs\" or \"src/**/*.ts\") to search only some files. The path can be in the working directory or anywhere else on disk (absolute paths work). \
         Returns path:line: text matches; .gitignore'd files, build directories and files over 1 MB are skipped."
    }

    fn name(&self) -> &'static str {
        "search_files"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Text to search for, or a regular expression when regex is true." },
                "path": { "type": "string", "description": "File or directory to search: relative to the working directory, or absolute. Defaults to '.'." },
                "regex": { "type": "boolean", "description": "Treat query as a regular expression. Default false." },
                "ignore_case": { "type": "boolean", "description": "Match upper and lower case alike. Default false." },
                "glob": { "type": "string", "description": "Only search files whose path or name matches this glob, such as \"*.rs\"." }
            },
            "required": ["query"]
        })
    }

    async fn run(&self, args: SearchFilesArgs, _cx: &ToolContext) -> ToolResult {
        let workspace = self.workspace.clone();
        blocking(move || search(&workspace, &args)).await
    }

    fn summary(&self, args: &SearchFilesArgs) -> String {
        format!("\"{}\" in {}", args.query, args.path)
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits::read_only()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

/// The lines of the answer, and how many of them are matches (the rest are notes).
#[derive(Default)]
struct Hits {
    lines: Vec<String>,
    listed: usize,
}

/// Runs `search_files`: lines matching a text or a regular expression.
fn search(workspace: &Workspace, args: &SearchFilesArgs) -> Result<String, String> {
    if args.query.is_empty() {
        return Err("query must not be empty".to_owned());
    }
    let pattern = if args.regex { args.query.clone() } else { regex::escape(&args.query) };
    let matcher = RegexBuilder::new(&pattern)
        .case_insensitive(args.ignore_case)
        .build()
        .map_err(|e| format!("invalid regular expression: {e}"))?;
    let filter = args.glob.as_deref().map(PathFilter::new).transpose()?;
    let start = workspace.locate(&args.path)?;

    let files = walk(&start, MAX_DEPTH)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| !kind.is_dir() && !kind.is_symlink()))
        .filter(|entry| filter.as_ref().is_none_or(|glob| entry.depth() == 0 || glob.accepts(&start, entry)));

    let mut hits = Hits::default();
    let (mut searched, mut skipped) = (0usize, 0usize);
    for entry in files {
        match search_file(entry.path(), workspace, &matcher, &mut hits) {
            Some(()) => searched = searched.saturating_add(1),
            None => skipped = skipped.saturating_add(1),
        }
        if hits.listed >= MAX_MATCHES {
            break;
        }
    }

    if hits.lines.is_empty() {
        let skipped_note =
            if skipped > 0 { format!(", skipped {skipped} binary, unreadable or over-1-MB") } else { String::new() };
        return Ok(format!("no matches for \"{}\" (searched {searched} files{skipped_note})", args.query));
    }
    let mut out = hits.lines.join("\n");
    if hits.listed >= MAX_MATCHES {
        let _written = write!(out, "\n[stopped after {MAX_MATCHES} matches; narrow the search with path or glob]");
    }
    Ok(out)
}

/// Adds the matching lines of one file. `None` if the file was not searched.
fn search_file(path: &Path, workspace: &Workspace, matcher: &Regex, hits: &mut Hits) -> Option<()> {
    if fs::metadata(path).map_or(true, |m| m.len() > MAX_FILE_BYTES) {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    // Binary and non-UTF-8 files are skipped.
    let text = str::from_utf8(&bytes).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let shown_path = workspace.display_path(path);
    let room = MAX_MATCHES_PER_FILE.min(MAX_MATCHES.saturating_sub(hits.listed));
    let (mut listed, mut more) = (0usize, 0usize);
    for (index, line) in text.lines().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }
        if listed < room {
            let shown: String = line.trim().chars().take(MAX_LINE_CHARS).collect();
            hits.lines.push(format!("{shown_path}:{}: {shown}", index.saturating_add(1)));
            listed = listed.saturating_add(1);
        } else {
            more = more.saturating_add(1);
        }
    }
    hits.listed = hits.listed.saturating_add(listed);
    if more > 0 && listed == MAX_MATCHES_PER_FILE {
        hits.lines.push(format!("{shown_path}: [+{more} more matches in this file]"));
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;

    /// A `path:line: text` match, not a note.
    fn is_hit(line: &str) -> bool {
        line.split(':').nth(1).is_some_and(|number| number.parse::<usize>().is_ok())
    }

    fn tree() -> TempTree {
        TempTree::new("sf")
            .file("a.txt", "one\ntwo\n")
            .file("sub/b.txt", "needle here\nnothing\n")
            .file("target/skip.txt", "needle in target\n")
    }

    #[tokio::test]
    async fn finds_matches_and_skips_build_directories() {
        let tree = tree();
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "needle" })).await;
        assert_eq!(result, ToolResult::ok("sub/b.txt:1: needle here"));
    }

    #[tokio::test]
    async fn gitignored_files_are_skipped_but_hidden_ones_are_searched() {
        let tree = TempTree::new("sf-ignore")
            .file(".gitignore", "generated/\n")
            .file("generated/out.txt", "needle generated\n")
            .file(".hidden.txt", "needle hidden\n");
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "needle" })).await;
        assert_eq!(result, ToolResult::ok(".hidden.txt:1: needle hidden"));
    }

    #[tokio::test]
    async fn a_skipped_directory_can_still_be_searched_directly() {
        let tree = tree();
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "needle", "path": "target" })).await;
        assert_eq!(result, ToolResult::ok("target/skip.txt:1: needle in target"));
    }

    #[tokio::test]
    async fn searches_a_single_file() {
        let tree = tree();
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "two", "path": "a.txt" })).await;
        assert_eq!(result, ToolResult::ok("a.txt:2: two"));
    }

    #[tokio::test]
    async fn reports_no_matches_and_bad_queries() {
        let tree = tree();
        let none = run(SearchFiles::new(tree.workspace()), json!({ "query": "zzz" })).await;
        assert_eq!(none, ToolResult::ok("no matches for \"zzz\" (searched 2 files)"));
        let empty = run(SearchFiles::new(tree.workspace()), json!({ "query": "" })).await;
        assert!(empty.is_error);
    }

    #[tokio::test]
    async fn one_file_cannot_use_up_all_the_matches() {
        let tree = TempTree::new("sf-per-file").file("big.txt", &"hit\n".repeat(150));
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "hit" })).await;
        assert_eq!(result.content.lines().filter(|l| is_hit(l)).count(), MAX_MATCHES_PER_FILE);
        assert!(result.content.contains("[+125 more matches in this file]"), "{}", result.content);
    }

    #[tokio::test]
    async fn stops_after_the_match_limit_and_still_reaches_later_files() {
        let mut tree = TempTree::new("sf-many");
        for index in 0i32..6i32 {
            tree = tree.file(&format!("f{index}.txt"), &"hit\n".repeat(40));
        }
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "hit" })).await;
        assert!(result.content.contains("[stopped after 100"), "{}", result.content);
        assert!(result.content.contains("f3.txt:"), "later files are reached");
        let listed = result.content.lines().filter(|l| is_hit(l)).count();
        assert_eq!(listed, MAX_MATCHES);
    }

    #[tokio::test]
    async fn a_regular_expression_and_ignoring_case_are_options() {
        let tree = TempTree::new("sf-regex").file("a.rs", "fn Foo(x: u8) {}\nlet y = 1;\n");
        let search = |args: Value| run(SearchFiles::new(tree.workspace()), args);

        let literal = search(json!({ "query": "fn \\w+\\(" })).await;
        assert!(literal.content.starts_with("no matches"), "{}", literal.content);
        let regex = search(json!({ "query": "fn \\w+\\(", "regex": true })).await;
        assert_eq!(regex, ToolResult::ok("a.rs:1: fn Foo(x: u8) {}"));
        let cased = search(json!({ "query": "FN FOO", "ignore_case": true })).await;
        assert_eq!(cased, ToolResult::ok("a.rs:1: fn Foo(x: u8) {}"));
        let bad = search(json!({ "query": "(", "regex": true })).await;
        assert!(bad.is_error && bad.content.contains("invalid regular expression"));
    }

    #[tokio::test]
    async fn a_glob_limits_which_files_are_searched() {
        let tree = TempTree::new("sf-glob")
            .file("src/a.rs", "needle\n")
            .file("src/deep/b.rs", "needle\n")
            .file("notes.md", "needle\n");
        let search = |glob: &str| run(SearchFiles::new(tree.workspace()), json!({ "query": "needle", "glob": glob }));
        let rust = search("*.rs").await;
        assert_eq!(rust, ToolResult::ok("src/a.rs:1: needle\nsrc/deep/b.rs:1: needle"));
        let deep = search("src/deep/*.rs").await;
        assert_eq!(deep, ToolResult::ok("src/deep/b.rs:1: needle"));
        let bad = search("[").await;
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn searches_outside_the_workspace() {
        let outside = TempTree::new("sf-outside").file("deep/n.txt", "needle here\n");
        let tree = tree();
        let path = outside.path().to_str().unwrap();
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "needle", "path": path })).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("n.txt:1: needle here"), "{}", result.content);
    }

    #[tokio::test]
    async fn a_missing_path_is_an_error() {
        let tree = tree();
        let result = run(SearchFiles::new(tree.workspace()), json!({ "query": "x", "path": "/no/such/place" })).await;
        assert!(result.is_error);
    }
}
