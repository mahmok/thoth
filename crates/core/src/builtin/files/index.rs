use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::path::Path;
use std::sync::Mutex;
use std::thread;

use ignore::WalkBuilder;

use super::{Suggestion, Suggestions};
use super::{open_reference, reference};
use crate::sync::MutexExt as _;
use crate::workspace::Workspace;

/// The most files indexed.
const MAX_ENTRIES: usize = 20_000;

/// A path and whether it is a directory.
type Entry = (String, bool);

#[derive(Clone, Debug)]
/// The Workspace files, scanned in the background.
pub(super) struct FileIndex {
    entries: Arc<Mutex<Arc<[Entry]>>>,
    scanning: Arc<AtomicBool>,
    workspace: Workspace,
}

impl FileIndex {
    fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::Acquire)
    }

    pub(super) fn new(workspace: &Workspace) -> Self {
        let index = Self { workspace: workspace.clone(), entries: Arc::default(), scanning: Arc::default() };
        index.refresh();
        index
    }

    pub(super) fn refresh(&self) {
        if self.scanning.swap(true, Ordering::AcqRel) {
            return;
        }
        let index = self.clone();
        let _detached = thread::spawn(move || {
            let found = scan(&index.workspace);
            *index.entries.lock_unpoisoned() = Arc::from(found);
            index.scanning.store(false, Ordering::Release);
        });
    }

    fn snapshot(&self) -> Arc<[Entry]> {
        self.entries.lock_unpoisoned().clone()
    }

    #[cfg(any(test, feature = "testing"))]
    pub(super) fn wait(&self) {
        while self.is_scanning() {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Lists the Workspace files and directories, skipping heavy ones.
fn scan(workspace: &Workspace) -> Vec<Entry> {
    let root = workspace.root().to_path_buf();
    let walk = WalkBuilder::new(&root)
        .hidden(false)
        .require_git(false)
        .sort_by_file_path(Ord::cmp)
        .filter_entry(|entry| !is_heavy(entry.path()))
        .build();
    walk.filter_map(Result::ok)
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(&root).ok()?;
            if relative.as_os_str().is_empty() {
                return None;
            }
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            let mut path =
                relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            if is_dir {
                path.push('/');
            }
            Some((path, is_dir))
        })
        .take(MAX_ENTRIES)
        .collect()
}

fn is_heavy(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(".git" | "node_modules") => true,
        Some("target") => path.parent().is_some_and(|parent| parent.join("Cargo.toml").is_file()),
        _ => false,
    }
}

/// The Workspace's files and directories as of the last scan, offered as `@` references.
pub(super) fn suggestions(index: &FileIndex) -> Suggestions {
    let items = index
        .snapshot()
        .iter()
        .map(|&(ref path, is_dir)| Suggestion {
            label: path.clone(),
            description: if is_dir { "directory" } else { "" }.to_owned(),
            partial: open_reference(path),
            full: format!("{} ", reference(path)),
        })
        .collect();
    let empty_message = if index.is_scanning() { "indexing files\u{2026}" } else { "no files" };
    Suggestions { title: "Files".to_owned(), items, empty_message: empty_message.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::assert_not_empty;
    use crate::test::temp_tree::TempTree;

    fn labels(tree: &TempTree) -> Vec<String> {
        let index = FileIndex::new(&tree.workspace());
        index.wait();
        suggestions(&index).items.into_iter().map(|item| item.label).collect()
    }

    #[test]
    fn a_suggestion_completes_to_a_reference_quoted_when_it_has_spaces() {
        let tree = TempTree::new("quoted").file("my notes.txt", "");
        let index = FileIndex::new(&tree.workspace());
        index.wait();
        let found = suggestions(&index).items.remove(0);
        assert_eq!(found.partial, "@\"my notes.txt");
        assert_eq!(found.full, "@\"my notes.txt\" ");
    }

    #[test]
    fn lists_files_and_directories_relative_to_the_workspace() {
        let tree = TempTree::new("files").file("a.txt", "").file("src/b.rs", "");
        assert_eq!(labels(&tree), ["a.txt", "src/", "src/b.rs"]);
    }

    #[test]
    fn build_output_is_skipped_without_a_gitignore() {
        let tree = TempTree::new("heavy")
            .file("Cargo.toml", "")
            .file("target/debug/x", "")
            .file("web/node_modules/y", "")
            .file("src/main.rs", "");
        assert_eq!(labels(&tree), ["Cargo.toml", "src/", "src/main.rs", "web/"]);
    }

    #[test]
    fn a_target_directory_outside_a_cargo_project_is_kept() {
        let tree = TempTree::new("java").file("target/classes/A.class", "");
        assert_eq!(labels(&tree), ["target/", "target/classes/", "target/classes/A.class"]);
    }

    #[test]
    fn the_menu_uses_the_last_scan_while_a_new_one_runs() {
        let tree = TempTree::new("stale").file("a.txt", "");
        let index = FileIndex::new(&tree.workspace());
        index.wait();
        let extended = tree.file("b.txt", "");
        index.refresh();
        // Whether or not the rescan has finished, the picker never comes up empty.
        assert_not_empty(&suggestions(&index).items);
        index.wait();
        assert_eq!(suggestions(&index).items.len(), 2);
        drop(extended);
    }

    #[test]
    fn gitignored_paths_and_dot_git_are_left_out_but_hidden_files_stay() {
        let tree = TempTree::new("ignored")
            .file(".gitignore", "target/\n*.log\n")
            .file("target/x", "")
            .file("a.log", "")
            .file(".git/config", "")
            .file(".env", "")
            .file("keep.rs", "");
        assert_eq!(labels(&tree), [".env", ".gitignore", "keep.rs"]);
    }
}
