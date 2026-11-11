use crate::utils::numbers;
use anyhow::Result;
use core::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Directories never searched for instruction files: build output and vendored code.
const SKIPPED_DIRS: [&str; 3] = [".git", "node_modules", "target"];

#[derive(Clone, Debug)]
/// An AGENTS.md file.
struct AgentFile {
    content: String,
    /// The file itself. The model is told which file each block of instructions came from.
    path: PathBuf,
}

impl AgentFile {
    /// The directory the instructions apply to.
    fn directory(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new(""))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// How much context the Conversation may use.
pub struct Budget {
    /// When to summarise old turns. `None` turns it off.
    pub compaction: Option<Compaction>,
    /// When to drop old Tool output. `None` keeps it all.
    pub elision: Option<Elision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// When and how old tool output is dropped.
pub struct Elision {
    /// Text below this many tokens is never elided.
    pub smallest: usize,
    /// Tokens of the newest bulky text to keep in full.
    pub target: usize,
    /// Elide only when the bulky text totals more tokens than this.
    pub trigger: usize,
}

impl Elision {
    /// Elides all old tool output.
    pub const EVERYTHING: Self = Self { trigger: 0, target: 0, smallest: 0 };
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// When and how old turns are summarised.
pub struct Compaction {
    /// Tokens at which compaction starts.
    pub budget: usize,
    /// Tokens of the newest turns kept as they are.
    pub keep_recent: usize,
    /// Share of the model's context window at which it starts, if lower.
    pub limit_share: f32,
}

impl Default for Budget {
    #[inline]
    fn default() -> Self {
        Self {
            elision: Some(Elision { trigger: 25_000, target: 12_500, smallest: 250 }),
            compaction: Some(Compaction { budget: 100_000, limit_share: 0.75, keep_recent: 20_000 }),
        }
    }
}

impl Compaction {
    #[must_use]
    #[inline]
    /// The context size at which compaction starts: the lower of the budget and the model's share.
    pub fn threshold(&self, context_limit: Option<u32>) -> usize {
        context_limit.map_or(self.budget, |limit| {
            self.budget.min(numbers::scaled(usize::try_from(limit).unwrap_or(usize::MAX), self.limit_share))
        })
    }
}

#[derive(Default, Clone, Debug)]
/// What an Agent knows before the first prompt.
pub struct Context {
    agent_files: Vec<AgentFile>,
    budget: Budget,
    directory: PathBuf,
    system_prompt: String,
}

impl Context {
    #[must_use]
    #[inline]
    pub const fn budget(&self) -> &Budget {
        &self.budget
    }

    fn get_agent_files_for_directory(&self, directory: &Path) -> Vec<&AgentFile> {
        let mut files: Vec<_> =
            self.agent_files.iter().filter(|file| directory.starts_with(file.directory())).collect();

        files.sort_by_key(|file| file.directory().components().count());
        files
    }

    #[must_use]
    #[inline]
    /// The system prompt: the working directory, tool hints and the AGENTS.md files.
    pub fn get_combined_system_prompt(&self) -> String {
        let mut prompt = format!(
            "{}Current working directory: {}\nFile paths are relative to it. The tools that only read (read_file, list_directory, find_files, search_files) may also use absolute paths elsewhere on disk; write_file and edit_file must stay inside it.",
            if self.system_prompt.is_empty() { String::new() } else { format!("{}\n\n", self.system_prompt) },
            self.directory.display()
        );
        prompt.push_str(&render(&self.get_agent_files_for_directory(self.directory.as_path())));
        prompt
    }

    #[must_use]
    #[inline]
    pub fn get_instructions(&self, directory: &Path) -> String {
        render(&self.get_agent_files_for_directory(directory))
    }

    #[must_use]
    #[inline]
    /// Creates a context for the directory `path`.
    pub fn new(path: &Path) -> Self {
        Self {
            agent_files: Vec::new(),
            system_prompt: String::new(),
            directory: path.to_path_buf(),
            budget: Budget::default(),
        }
    }

    #[inline]
    /// Loads the AGENTS.md files under the directory.
    ///
    /// # Errors
    ///
    /// Fails if an AGENTS.md file cannot be read.
    pub fn with_agent_files(mut self) -> Result<Self> {
        let agent_files = load_agents_files(&[self.directory.as_path()])?;
        self.agent_files = agent_files;
        Ok(self)
    }

    #[must_use]
    #[inline]
    pub const fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    #[inline]
    #[must_use]
    pub fn with_system_prompt<IntoString>(mut self, prompt: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        self.system_prompt = prompt.into();
        self
    }
}

/// The instruction files as a block of a system prompt, each named by its path and outermost
/// first. Empty when there are none.
fn render(files: &[&AgentFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut block = String::from("\n\n<project_context>\nProject-specific instructions and guidelines:");
    for file in files {
        let _written = write!(
            block,
            "\n\n<project_instructions path=\"{}\">\n{}\n</project_instructions>",
            file.path.display(),
            file.content.trim()
        );
    }
    block.push_str("\n</project_context>");
    block
}

fn is_agents_md_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        && path
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("agent") || stem.eq_ignore_ascii_case("agents"))
}

fn load_agents_files(paths: &[&Path]) -> Result<Vec<AgentFile>> {
    let mut agents_files = Vec::new();
    for path in paths {
        if path.is_dir() {
            load_agent_files_recursively(path, &mut agents_files)?;
        } else if path.is_file() && is_agents_md_file(path) {
            agents_files.push(read_agent_file(path)?);
        } else {
            // Anything else is not an instruction file.
        }
    }
    Ok(agents_files)
}

/// Collects AGENTS.md files under `dir`, skipping build and VCS directories.
fn load_agent_files_recursively(dir: &Path, agents_files: &mut Vec<AgentFile>) -> Result<()> {
    for result in fs::read_dir(dir)? {
        let entry = result?;
        let path = entry.path();
        // `file_type` does not follow symlinks, so a link can neither loop nor leave the tree.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !SKIPPED_DIRS.iter().any(|skip| entry.file_name() == *skip) {
                load_agent_files_recursively(&path, agents_files)?;
            }
        } else if !file_type.is_symlink() && is_agents_md_file(&path) {
            agents_files.push(read_agent_file(&path)?);
        } else {
            // A link is never followed, and any other file is not an instruction file.
        }
    }
    Ok(())
}

fn read_agent_file(path: &Path) -> Result<AgentFile> {
    Ok(AgentFile { content: fs::read_to_string(path)?, path: path.to_path_buf() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::temp_tree::TempTree;

    #[test]
    fn compaction_starts_at_the_budget_or_a_share_of_the_context_limit_whichever_is_smaller() {
        let compaction = Budget::default().compaction.unwrap();
        assert_eq!(compaction.threshold(None), 100_000);
        assert_eq!(compaction.threshold(Some(1_000_000)), 100_000);
        assert_eq!(compaction.threshold(Some(32_000)), 24_000);
    }

    #[test]
    fn without_a_base_prompt_the_prompt_starts_with_the_working_directory() {
        let tree = TempTree::new("ctx-no-base");
        let prompt = Context::new(tree.path()).get_combined_system_prompt();
        assert!(prompt.starts_with(&format!("Current working directory: {}", tree.path().display())), "{prompt}");
    }

    #[test]
    fn without_agent_files_the_prompt_is_just_the_base() {
        let tree = TempTree::new("ctx-none");
        let context = Context::new(tree.path()).with_system_prompt("base").with_agent_files().unwrap();
        assert_eq!(
            context.get_combined_system_prompt(),
            format!(
                "base\n\nCurrent working directory: {}\nFile paths are relative to it. The tools that only read (read_file, list_directory, find_files, search_files) may also use absolute paths elsewhere on disk; write_file and edit_file must stay inside it.",
                tree.path().display()
            )
        );
    }

    #[test]
    fn build_and_vendor_directories_are_not_searched() {
        let tree = TempTree::new("ctx-skip")
            .file("AGENTS.md", "root rules")
            .file("target/AGENTS.md", "generated")
            .file("node_modules/pkg/AGENTS.md", "vendored");
        let context = Context::new(tree.path()).with_agent_files().unwrap();
        let prompt = context.get_combined_system_prompt();
        assert!(prompt.contains("root rules"), "{prompt}");
        assert!(!prompt.contains("generated") && !prompt.contains("vendored"), "{prompt}");
    }

    #[test]
    fn applicable_agent_files_follow_the_base_outermost_first() {
        let tree = TempTree::new("ctx-nested")
            .file("AGENTS.md", "root rules\n")
            .file("sub/AGENTS.md", "sub rules")
            .file("other/AGENTS.md", "unrelated");
        let context = Context::new(tree.path()).with_system_prompt("base").with_agent_files().unwrap();

        // From the root, only the root's own instructions apply.
        let root_prompt = context.get_combined_system_prompt();
        assert!(root_prompt.starts_with("base\n\nCurrent working directory:"), "{root_prompt}");
        assert!(root_prompt.contains("root rules"), "{root_prompt}");
        assert!(!root_prompt.contains("sub rules"), "{root_prompt}");

        // From inside `sub/`, both the root and `sub/` instructions apply, outermost first.
        let instructions = context.get_instructions(&tree.path().join("sub"));
        let root = instructions.find("root rules").unwrap();
        let sub = instructions.find("sub rules").unwrap();
        assert!(root < sub, "{instructions}");
        assert!(!instructions.contains("unrelated"), "{instructions}");
    }

    #[test]
    fn each_instruction_file_is_named_by_its_path_inside_a_project_context_block() {
        let tree = TempTree::new("ctx-format").file("AGENTS.md", "root rules\n");
        let context = Context::new(tree.path()).with_agent_files().unwrap();
        let path = tree.path().join("AGENTS.md");
        let prompt = context.get_combined_system_prompt();
        let block = format!(
            "<project_context>\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"{}\">\nroot rules\n</project_instructions>\n</project_context>",
            path.display()
        );
        assert!(prompt.ends_with(&block), "{prompt}");
        assert!(!prompt.contains("# Instructions from"));
    }

    #[test]
    fn nested_files_each_get_their_own_tag() {
        let tree = TempTree::new("ctx-tags").file("AGENTS.md", "root").file("sub/AGENTS.md", "inner");
        let context = Context::new(tree.path()).with_agent_files().unwrap();
        let text = context.get_instructions(&tree.path().join("sub"));
        assert_eq!(text.matches("<project_instructions path=").count(), 2, "{text}");
        assert_eq!(text.matches("<project_context>").count(), 1, "{text}");
        assert!(text.find("root").unwrap() < text.find("inner").unwrap());
    }
}
