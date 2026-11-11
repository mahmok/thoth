use std::path::Path;

use globset::{Glob, GlobMatcher};
use ignore::{DirEntry, Walk, WalkBuilder};

/// How deep a walk goes.
pub(super) const MAX_DEPTH: usize = 20;
/// Directories a walk never enters.
const SKIPPED_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// A glob that narrows a search.
pub(super) struct PathFilter(GlobMatcher);

impl PathFilter {
    pub(super) fn accepts(&self, start: &Path, entry: &DirEntry) -> bool {
        let below = entry.path().strip_prefix(start).unwrap_or_else(|_| entry.path());
        self.0.is_match(below) || self.0.is_match(entry.file_name())
    }

    pub(super) fn new(glob: &str) -> Result<Self, String> {
        Glob::new(glob)
            .map(|compiled| Self(compiled.compile_matcher()))
            .map_err(|e| format!("invalid glob `{glob}`: {e}"))
    }
}

pub(super) fn walk(start: &Path, max_depth: usize) -> Walk {
    WalkBuilder::new(start)
        .hidden(false)
        .require_git(false)
        .max_depth(Some(max_depth))
        .sort_by_file_path(Ord::cmp)
        .filter_entry(|entry| entry.depth() == 0 || !is_skipped(entry))
        .build()
}

fn is_skipped(entry: &DirEntry) -> bool {
    SKIPPED_DIRS.iter().any(|skip| entry.file_name() == *skip)
}
