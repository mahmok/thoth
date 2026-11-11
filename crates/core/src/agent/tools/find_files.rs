use core::fmt::Write as _;
use serde::Deserialize;
use serde_json::{Value, json};

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits, blocking};
use super::walk::{MAX_DEPTH, PathFilter, walk};
use crate::workspace::{Workspace, current_dir};

/// The most files listed.
const MAX_RESULTS: usize = 200;

/// Finds files by name or path, with a glob.
#[derive(Debug)]
pub(super) struct FindFiles {
    workspace: Workspace,
}

impl FindFiles {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[derive(Debug, Deserialize)]
/// Arguments of `find_files`.
pub(super) struct FindFilesArgs {
    #[serde(default = "current_dir")]
    path: String,
    pattern: String,
}

impl Tool for FindFiles {
    type Args = FindFilesArgs;

    fn description(&self) -> &'static str {
        "Find files whose path or name matches a glob, such as \"*.rs\", \"Cargo.toml\" or \"src/**/test_*.py\", under a directory. \
         The directory can be in the working directory or anywhere else on disk. .gitignore'd files and build directories are skipped. \
         Returns one path per line; use search_files to look inside files."
    }

    fn name(&self) -> &'static str {
        "find_files"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob to match against file paths and names, such as \"*.rs\"." },
                "path": { "type": "string", "description": "Directory to search: relative to the working directory, or absolute. Defaults to '.'." }
            },
            "required": ["pattern"]
        })
    }

    async fn run(&self, args: FindFilesArgs, _cx: &ToolContext) -> ToolResult {
        let workspace = self.workspace.clone();
        blocking(move || find(&workspace, &args)).await
    }

    fn summary(&self, args: &FindFilesArgs) -> String {
        format!("{} in {}", args.pattern, args.path)
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits::read_only()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
}

/// Runs `find_files`: files whose path matches a glob.
fn find(workspace: &Workspace, args: &FindFilesArgs) -> Result<String, String> {
    if args.pattern.is_empty() {
        return Err("pattern must not be empty".to_owned());
    }
    let filter = PathFilter::new(&args.pattern)?;
    let start = workspace.locate(&args.path)?;
    if !start.is_dir() {
        return Err(format!("`{}` is not a directory", args.path));
    }
    let mut found: Vec<String> = Vec::new();
    let mut more = 0usize;
    for entry in walk(&start, MAX_DEPTH).filter_map(Result::ok) {
        let is_file = entry.file_type().is_some_and(|kind| !kind.is_dir() && !kind.is_symlink());
        if !is_file || !filter.accepts(&start, &entry) {
            continue;
        }
        if found.len() < MAX_RESULTS {
            found.push(workspace.display_path(entry.path()).to_string());
        } else {
            more = more.saturating_add(1);
        }
    }
    if found.is_empty() {
        return Ok(format!("no files match \"{}\"", args.pattern));
    }
    let mut out = found.join("\n");
    if more > 0 {
        let _written = write!(
            out,
            "\n[showing {MAX_RESULTS} of {} files; use a narrower pattern or path]",
            MAX_RESULTS.saturating_add(more)
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;

    fn tree() -> TempTree {
        TempTree::new("ff")
            .file("Cargo.toml", "")
            .file("src/lib.rs", "")
            .file("src/deep/mod.rs", "")
            .file("src/notes.md", "")
            .file("target/skip.rs", "")
            .file(".gitignore", "generated/\n")
            .file("generated/out.rs", "")
    }

    #[tokio::test]
    async fn a_bare_pattern_matches_names_at_any_depth() {
        let tree = tree();
        let result = run(FindFiles::new(tree.workspace()), json!({ "pattern": "*.rs" })).await;
        assert_eq!(result, ToolResult::ok("src/deep/mod.rs\nsrc/lib.rs"));
    }

    #[tokio::test]
    async fn a_pattern_with_directories_matches_paths_below_the_search_directory() {
        let tree = tree();
        let result = run(FindFiles::new(tree.workspace()), json!({ "pattern": "deep/*.rs", "path": "src" })).await;
        assert_eq!(result, ToolResult::ok("src/deep/mod.rs"));
    }

    #[tokio::test]
    async fn exact_names_and_misses() {
        let tree = tree();
        let found = run(FindFiles::new(tree.workspace()), json!({ "pattern": "Cargo.toml" })).await;
        assert_eq!(found, ToolResult::ok("Cargo.toml"));
        let none = run(FindFiles::new(tree.workspace()), json!({ "pattern": "*.zig" })).await;
        assert_eq!(none, ToolResult::ok("no files match \"*.zig\""));
    }

    #[tokio::test]
    async fn bad_input_is_an_error_and_the_list_is_capped() {
        let tree = tree();
        let tool = || FindFiles::new(tree.workspace());
        assert!(run(tool(), json!({ "pattern": "" })).await.is_error);
        assert!(run(tool(), json!({ "pattern": "[" })).await.is_error);
        assert!(run(tool(), json!({ "pattern": "*", "path": "Cargo.toml" })).await.is_error);

        let mut many = TempTree::new("ff-many");
        for index in 0..MAX_RESULTS + 5 {
            many = many.file(&format!("f{index:03}.txt"), "");
        }
        let result = run(FindFiles::new(many.workspace()), json!({ "pattern": "*.txt" })).await;
        assert!(
            result.content.ends_with("[showing 200 of 205 files; use a narrower pattern or path]"),
            "{}",
            result.content
        );
    }
}
