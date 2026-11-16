#[cfg(not(windows))]
mod bash;
pub mod delegate;
mod edit_file;
mod find_files;
mod list_directory;
pub mod message;
#[cfg(windows)]
mod powershell;
mod read_file;
mod search_files;
mod shell;
pub mod tool;
pub mod tool_set;
mod walk;
mod write_file;

use crate::workspace::Workspace;
use tool_set::ToolSet;

use crate::agent::tools::invoke_skill::InvokeSkill;
#[cfg(not(windows))]
use bash::Bash;
use edit_file::EditFile;
use find_files::FindFiles;
use list_directory::ListDirectory;
#[cfg(windows)]
use powershell::PowerShell;
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
        .and_then(|()| tools.register(WriteFile::new(workspace.clone())));
    #[cfg(not(windows))]
    let registered = registered.and_then(|()| tools.register(Bash::new(workspace)));
    #[cfg(windows)]
    let registered = registered.and_then(|()| tools.register(PowerShell::new(workspace)));
    let registered = registered.and_then(|()| tools.register(InvokeSkill::default()));
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
    use serde_json::Value;

    #[test]
    fn only_the_tools_that_change_nothing_are_read_only() {
        let tree = TempTree::new("builtin-traits");
        let tools = builtin(tree.workspace());
        let shell = if cfg!(windows) { "powershell" } else { "bash" };
        let read_only: Vec<&str> =
            ["read_file", "list_directory", "search_files", "find_files", "edit_file", "write_file", shell]
                .into_iter()
                .filter(|name| tools.traits(name, "{}").read_only)
                .collect();
        assert_eq!(read_only, ["read_file", "list_directory", "search_files", "find_files"]);
        assert!(!tools.traits("unknown", "{}").read_only);
    }

    #[test]
    fn builtin_offers_the_file_tools() {
        let tree = TempTree::new("builtin");
        let names: Vec<String> = builtin(tree.workspace()).definitions().map(|d| d.name.clone()).collect();
        assert_eq!(
            names,
            [
                "read_file",
                "list_directory",
                "search_files",
                "find_files",
                "edit_file",
                "write_file",
                if cfg!(windows) { "powershell" } else { "bash" },
                "invoke_skill"
            ]
        );
    }

    #[test]
    fn every_tool_schema_accepts_a_sample_of_its_own_arguments() {
        let tree = TempTree::new("schema-samples");
        let tools = builtin(tree.workspace());
        for spec in tools.definitions() {
            let (sample, required) = sample_arguments(&spec.parameters);
            assert!(
                tools.accepts(&spec.name, &sample),
                "{}: arguments built from its own schema were rejected",
                spec.name
            );
            let fields: Vec<String> = sample.as_object().expect("a schema is an object").keys().cloned().collect();
            for field in fields {
                let mut missing = sample.clone();
                drop(missing.as_object_mut().expect("an object").remove(&field));
                let omitted = tools.accepts(&spec.name, &missing);
                assert_eq!(
                    !omitted,
                    required.contains(&field),
                    "{}: `{field}` is listed as required={} but omitting it parsed={omitted}",
                    spec.name,
                    required.contains(&field)
                );
            }
        }
    }

    /// An argument value of the kind a schema names, and the fields it calls required.
    fn sample_arguments(schema: &Value) -> (Value, Vec<String>) {
        fn value(schema: &Value) -> Value {
            match schema.get("type").and_then(Value::as_str) {
                Some("array") => Value::Array(Vec::new()),
                Some("boolean") => Value::Bool(false),
                Some("integer" | "number") => Value::from(1),
                Some("object") => Value::Object(serde_json::Map::new()),
                Some("string") => Value::from("x"),
                _ => Value::Null,
            }
        }
        let empty = serde_json::Map::new();
        let properties = schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        let sample = properties.iter().map(|(name, field)| (name.clone(), value(field))).collect();
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        (Value::Object(sample), required)
    }
}
