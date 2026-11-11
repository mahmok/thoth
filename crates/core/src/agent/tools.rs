mod bash;
pub mod delegate;
mod edit_file;
mod find_files;
mod list_directory;
pub mod message;
mod read_file;
mod search_files;
pub mod tool;
pub mod tool_set;
mod walk;
mod write_file;

use crate::workspace::Workspace;
use tool_set::ToolSet;

use crate::agent::tools::invoke_skill::InvokeSkill;
use bash::Bash;
use edit_file::EditFile;
use find_files::FindFiles;
use list_directory::ListDirectory;
use read_file::ReadFile;
use search_files::SearchFiles;
use write_file::WriteFile;

#[must_use]
#[expect(clippy::expect_used, reason = "the built-in tool names are written in this function and are unique")]
#[inline]
/// The built-in Tools, working in `workspace`.
///
/// # Panics
///
/// Panics if two built-in Tools share a name.
pub fn builtin(workspace: Workspace) -> ToolSet {
    let mut tools = ToolSet::new();
    let registered = tools
        .register(ReadFile::new(workspace.clone()))
        .and_then(|()| tools.register(ListDirectory::new(workspace.clone())))
        .and_then(|()| tools.register(SearchFiles::new(workspace.clone())))
        .and_then(|()| tools.register(FindFiles::new(workspace.clone())))
        .and_then(|()| tools.register(EditFile::new(workspace.clone())))
        .and_then(|()| tools.register(WriteFile::new(workspace.clone())))
        .and_then(|()| tools.register(Bash::new(workspace)))
        .and_then(|()| tools.register(InvokeSkill::default()));
    registered.expect("built-in tool names are unique");
    tools
}

#[cfg(test)]
pub mod delegation_fixtures;

pub mod invoke_skill;
#[cfg(test)]
mod testing;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::temp_tree::TempTree;

    #[test]
    fn only_the_tools_that_change_nothing_are_read_only() {
        let tree = TempTree::new("builtin-traits");
        let tools = builtin(tree.workspace());
        let read_only: Vec<&str> =
            ["read_file", "list_directory", "search_files", "find_files", "edit_file", "write_file", "bash"]
                .into_iter()
                .filter(|name| tools.traits(name, "{}").read_only)
                .collect();
        assert_eq!(read_only, ["read_file", "list_directory", "search_files", "find_files"]);
        assert!(!tools.traits("unknown", "{}").read_only);
    }

    #[test]
    fn builtin_offers_the_file_tools() {
        let tree = TempTree::new("builtin");
        let names: Vec<String> = builtin(tree.workspace()).definitions().iter().map(|d| d.name.clone()).collect();
        assert_eq!(
            names,
            ["read_file", "list_directory", "search_files", "find_files", "edit_file", "write_file", "bash"]
        );
    }
}
