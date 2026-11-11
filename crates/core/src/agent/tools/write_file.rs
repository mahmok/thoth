use crate::fsutil;
use std::fs;

use serde::Deserialize;
use serde_json::{Value, json};

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits, blocking};
use crate::workspace::Workspace;
#[derive(Debug)]

/// The `write_file` Tool.
pub(super) struct WriteFile {
    workspace: Workspace,
}

impl WriteFile {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[derive(Debug, Deserialize)]
/// Arguments of `write_file`.
pub(super) struct WriteFileArgs {
    content: String,
    path: String,
}

impl Tool for WriteFile {
    type Args = WriteFileArgs;

    fn description(&self) -> &'static str {
        "Create a text file with the given content, making any missing parent directories. \
         If the file already exists it is replaced whole, so for a change to an existing file read it \
         and use edit_file instead. Paths are relative to the working directory and must stay inside it."
    }

    fn name(&self) -> &'static str {
        "write_file"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to write: relative to the working directory." },
                "content": { "type": "string", "description": "The complete content of the file." }
            },
            "required": ["path", "content"]
        })
    }

    async fn run(&self, args: WriteFileArgs, _cx: &ToolContext) -> ToolResult {
        let workspace = self.workspace.clone();
        blocking(move || write(&workspace, &args)).await
    }

    fn summary(&self, args: &WriteFileArgs) -> String {
        args.path.clone()
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

/// Runs `write_file`: creates or replaces a file.
fn write(workspace: &Workspace, args: &WriteFileArgs) -> Result<String, String> {
    let path = args.path.as_str();
    let full = workspace.resolve_for_write(path)?;
    if full.is_dir() {
        return Err(format!("`{path}` is a directory"));
    }
    let existed = full.exists();
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create the folder for `{path}`: {e}"))?;
    }
    fsutil::write_replacing(&full, args.content.as_bytes()).map_err(|e| format!("cannot write `{path}`: {e}"))?;
    let shown = workspace.display_path(&full);
    let lines = args.content.lines().count();
    let verb = if existed { "replaced" } else { "created" };
    Ok(format!("{verb} {shown} ({lines} lines, {} bytes)", args.content.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;

    fn tool(tree: &TempTree) -> WriteFile {
        WriteFile::new(tree.workspace())
    }

    #[tokio::test]
    async fn creates_a_file_and_its_parent_directories() {
        let tree = TempTree::new("wf-new");
        let result = run(tool(&tree), json!({ "path": "a/b/new.txt", "content": "one\ntwo\n" })).await;
        assert_eq!(result, ToolResult::ok("created a/b/new.txt (2 lines, 8 bytes)"));
        assert_eq!(fs::read_to_string(tree.path().join("a/b/new.txt")).unwrap(), "one\ntwo\n");
    }

    #[tokio::test]
    async fn replaces_an_existing_file_and_says_so() {
        let tree = TempTree::new("wf-replace").file("a.txt", "old");
        let result = run(tool(&tree), json!({ "path": "a.txt", "content": "new" })).await;
        assert!(result.content.starts_with("replaced a.txt"), "{}", result.content);
        assert_eq!(fs::read_to_string(tree.path().join("a.txt")).unwrap(), "new");
    }

    #[tokio::test]
    async fn refuses_directories_and_paths_outside_the_workspace() {
        let tree = TempTree::new("wf-refuse").file("d/x", "");
        let dir = run(tool(&tree), json!({ "path": "d", "content": "x" })).await;
        assert!(dir.is_error, "{}", dir.content);
        let escape = run(tool(&tree), json!({ "path": "../x.txt", "content": "x" })).await;
        assert!(escape.is_error);
        assert!(!tree.path().join("../x.txt").exists());
    }
}
