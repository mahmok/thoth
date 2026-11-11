use core::fmt::Write as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits, blocking};
use crate::workspace::{Workspace, current_dir};

/// The most entries listed.
const MAX_ENTRIES: usize = 200;
#[derive(Debug)]

/// The `list_directory` Tool.
pub(super) struct ListDirectory {
    workspace: Workspace,
}

impl ListDirectory {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[derive(Debug, Deserialize)]
/// Arguments of `list_directory`.
pub(super) struct ListDirectoryArgs {
    #[serde(default = "current_dir")]
    path: String,
}

impl Tool for ListDirectory {
    type Args = ListDirectoryArgs;

    fn description(&self) -> &'static str {
        "List the entries of a directory, in the working directory or anywhere else on disk (absolute paths work). Directories end with '/'. \
         It lists one level only; to find files deeper down, use find_files."
    }

    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path: relative to the working directory, or absolute. Defaults to '.'." }
            }
        })
    }

    async fn run(&self, args: ListDirectoryArgs, _cx: &ToolContext) -> ToolResult {
        let workspace = self.workspace.clone();
        blocking(move || list(&workspace, &args)).await
    }

    fn summary(&self, args: &ListDirectoryArgs) -> String {
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

/// Runs `list_directory`.
fn list(workspace: &Workspace, args: &ListDirectoryArgs) -> Result<String, String> {
    let dir = workspace.locate(&args.path)?;
    if !dir.is_dir() {
        return Err(format!("`{}` is not a directory", args.path));
    }
    let mut entries: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| format!("cannot read `{}`: {e}", args.path))?
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|t| t.is_dir()) { format!("{name}/") } else { name }
        })
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Ok("(empty directory)".to_owned());
    }
    let total = entries.len();
    entries.truncate(MAX_ENTRIES);
    let mut out = entries.join("\n");
    if total > MAX_ENTRIES {
        let _written = write!(out, "\n[showing {MAX_ENTRIES} of {total} entries]");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;
    use std::fs;

    #[tokio::test]
    async fn lists_entries_with_directories_marked() {
        let tree = TempTree::new("ld-list").file("a.txt", "").file("sub/b.txt", "");
        let result = run(ListDirectory::new(tree.workspace()), json!({})).await;
        assert_eq!(result, ToolResult::ok("a.txt\nsub/"));
    }

    #[tokio::test]
    async fn lists_a_subdirectory() {
        let tree = TempTree::new("ld-sub").file("sub/b.txt", "");
        let result = run(ListDirectory::new(tree.workspace()), json!({ "path": "sub" })).await;
        assert_eq!(result, ToolResult::ok("b.txt"));
    }

    #[tokio::test]
    async fn empty_and_missing_directories() {
        let tree = TempTree::new("ld-empty").file("d/x", "");
        fs::remove_file(tree.path().join("d/x")).unwrap();
        let empty = run(ListDirectory::new(tree.workspace()), json!({ "path": "d" })).await;
        assert_eq!(empty, ToolResult::ok("(empty directory)"));
        let missing = run(ListDirectory::new(tree.workspace()), json!({ "path": "nope" })).await;
        assert!(missing.is_error);
    }

    #[tokio::test]
    async fn lists_directories_outside_the_workspace_like_the_other_read_tools() {
        let outside = TempTree::new("ld-outside").file("far.txt", "");
        let tree = TempTree::new("ld-inside");
        let result =
            run(ListDirectory::new(tree.workspace()), json!({ "path": outside.path().to_str().unwrap() })).await;
        assert_eq!(result, ToolResult::ok("far.txt"));
    }
}
