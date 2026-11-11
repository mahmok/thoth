/// The index of Workspace files.
mod index;

use crate::workspace::{MAX_READ_BYTES, ReadPolicy, Workspace, truncate};
use core::fmt::Write as _;

use index::FileIndex;

/// Larger files are not read at all; smaller ones are cut down to `MAX_READ_BYTES`.
const MAX_FILE_BYTES: u64 = 128 * 1024;

/// Punctuation that ends a path in running text.
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}'];

/// The character that starts a file reference.
pub const FILE_TRIGGER: char = '@';

/// One way to finish the word being typed after the trigger character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub description: String,
    /// Replaces the word, trigger included, when the user settles on it.
    pub full: String,
    pub label: String,
    /// Replaces the word, trigger included, while the user goes on typing it.
    pub partial: String,
}

/// What a completion offers for the trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestions {
    pub empty_message: String,
    pub items: Vec<Suggestion>,
    pub title: String,
}

/// `@path` references: offers the Workspace's files while one is typed and
/// appends the referenced files when the prompt is sent.
#[derive(Debug)]
pub struct FileReferences {
    index: FileIndex,
    workspace: Workspace,
}

impl FileReferences {
    /// The files `text` refers to, as blocks to append to a prompt.
    pub fn attach(&self, text: &str) -> String {
        attachments(text, |path| {
            let content = self.workspace.read(path, ReadPolicy::inside(MAX_FILE_BYTES)).ok()?;
            Some(truncate(content, MAX_READ_BYTES))
        })
    }

    #[must_use]
    #[inline]
    pub fn new(workspace: Workspace) -> Self {
        let index = FileIndex::new(&workspace);
        Self { index, workspace }
    }

    /// The Workspace's files, to complete a reference being typed.
    pub fn suggestions(&self) -> Suggestions {
        self.index.refresh();
        index::suggestions(&self.index)
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    pub fn wait(&self) {
        self.index.wait();
    }
}

/// A complete reference to `path`; paths with whitespace are quoted.
fn reference(path: &str) -> String {
    if path.contains(char::is_whitespace) { format!("@\"{path}\"") } else { format!("@{path}") }
}

fn open_reference(path: &str) -> String {
    if path.contains(char::is_whitespace) { format!("@\"{path}") } else { format!("@{path}") }
}

/// The paths of the `@` references in `text`.
fn paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    let mut at_token_start = true;
    while let Some(c) = rest.chars().next() {
        if at_token_start && let Some(after) = rest.strip_prefix('@') {
            let (path, consumed) = token(after);
            if !path.is_empty() {
                found.push(path);
                rest = after.get(consumed..).unwrap_or_default();
                at_token_start = false;
                continue;
            }
        }
        at_token_start = c.is_whitespace();
        rest = rest.get(c.len_utf8()..).unwrap_or_default();
    }
    found
}

fn token(after: &str) -> (String, usize) {
    if let Some(quoted) = after.strip_prefix('"') {
        return match quoted.split_once('"') {
            Some((path, _)) => (path.to_owned(), path.len().saturating_add(2)),
            None => (String::new(), 0),
        };
    }
    let (path, _) = after.split_once(char::is_whitespace).unwrap_or((after, ""));
    (path.to_owned(), path.len())
}

/// The files referenced in `text`, as blocks to append to the prompt.
fn attachments(text: &str, read: impl Fn(&str) -> Option<String>) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::new();
    for path in paths(text) {
        let trimmed = path.trim_end_matches(TRAILING_PUNCTUATION).to_owned();
        let hit = [path, trimmed]
            .into_iter()
            .filter(|candidate| !candidate.is_empty())
            .find_map(|candidate| read(&candidate).map(|content| (candidate, content)));
        if let Some((found, content)) = hit {
            if seen.contains(&found) {
                continue;
            }
            let _written = write!(out, "\n\n<file path=\"{found}\">\n{content}\n</file>");
            seen.push(found);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::temp_tree::TempTree;

    #[test]
    fn references_quote_paths_with_spaces() {
        assert_eq!(reference("a/b.rs"), "@a/b.rs");
        assert_eq!(reference("my notes.txt"), "@\"my notes.txt\"");
        assert_eq!(open_reference("my notes.txt"), "@\"my notes.txt");
    }

    fn read(path: &str) -> Option<String> {
        match path {
            "a.rs" => Some("fn a() {}".into()),
            "my notes.txt" => Some("notes".into()),
            _ => None,
        }
    }

    #[test]
    fn referenced_files_are_appended_with_boundaries() {
        assert_eq!(
            attachments("see @a.rs and @\"my notes.txt\"", read),
            "\n\n<file path=\"a.rs\">\nfn a() {}\n</file>\n\n<file path=\"my notes.txt\">\nnotes\n</file>"
        );
    }

    #[test]
    fn a_large_attachment_is_cut_to_the_size_the_model_could_read() {
        let tree = TempTree::new("attach-big").file("big.txt", &"x".repeat(MAX_READ_BYTES + 100));
        let attached = FileReferences::new(tree.workspace()).attach("@big.txt");
        assert!(attached.contains("[truncated"), "{}", attached.len());
        assert!(attached.len() < MAX_READ_BYTES + 400);
    }

    #[test]
    fn unreadable_paths_and_plain_text_add_nothing() {
        assert_eq!(attachments("@dir/ @missing me@a.rs", read), "");
        assert_eq!(attachments("no refs", read), "");
    }

    #[test]
    fn repeats_and_trailing_punctuation_are_handled() {
        assert_eq!(attachments("@a.rs, then @a.rs.", read), "\n\n<file path=\"a.rs\">\nfn a() {}\n</file>");
    }
}
