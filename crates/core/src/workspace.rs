use alloc::sync::Arc;
use core::fmt;
use core::fmt::Formatter;
use std::env;
use std::fs;
use std::io;
use std::path;
use std::path::Component;
use std::path::{Path, PathBuf};

/// The most text of one file that goes into the conversation, whether the model read it or
/// the user attached it. Everything in the conversation is sent again with every step.
pub const MAX_READ_BYTES: usize = 50_000;

#[derive(Debug, Clone)]
/// The working directory Tools may touch.
pub struct Workspace {
    root: Arc<Path>,
}

impl Workspace {
    #[must_use]
    #[inline]
    pub fn display_path<'path>(&self, path: &'path Path) -> path::Display<'path> {
        path.strip_prefix(&*self.root).unwrap_or(path).display()
    }

    #[inline]
    /// The Workspace rooted at the current directory.
    ///
    /// # Errors
    ///
    /// Fails if the current directory cannot be read.
    pub fn from_current_dir() -> io::Result<Self> {
        Self::new(env::current_dir()?)
    }

    pub fn locate(&self, path: &str) -> Result<PathBuf, String> {
        self.root.join(path).canonicalize().map_err(|e| format!("cannot access `{path}`: {e}"))
    }

    #[inline]
    /// Creates a Workspace rooted at `root`.
    ///
    /// # Errors
    ///
    /// Fails if `root` cannot be resolved.
    pub fn new<PathLike>(root: PathLike) -> io::Result<Self>
    where
        PathLike: AsRef<Path>,
    {
        Ok(Self { root: Arc::from(root.as_ref().canonicalize()?) })
    }

    #[inline]
    /// Reads a text file under the policy: confined or not, with a size limit.
    ///
    /// # Errors
    ///
    /// Refused if the path is outside the working directory (when confined), is not a file, is too big or cannot be read.
    pub fn read(&self, path: &str, policy: ReadPolicy) -> Result<String, Refusal> {
        let located = if policy.confined { self.resolve(path) } else { self.locate(path) };
        let full = located.map_err(Refusal)?;
        let meta = fs::metadata(&full).map_err(|e| Refusal(format!("cannot read `{path}`: {e}")))?;
        if !meta.is_file() {
            return Err(Refusal(format!("`{path}` is not a file")));
        }
        if policy.max_bytes.is_some_and(|max| meta.len() > max) {
            return Err(Refusal(format!("`{path}` is too big ({} bytes)", meta.len())));
        }
        let bytes = fs::read(&full).map_err(|e| Refusal(format!("cannot read `{path}`: {e}")))?;
        if bytes.contains(&0) {
            return Err(Refusal(format!("`{path}` looks like a binary file")));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[inline]
    /// The full path of `path`, if it stays inside the working directory.
    ///
    /// # Errors
    ///
    /// Fails if `path` is outside the working directory.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let full = self.locate(path)?;
        if full.starts_with(&*self.root) { Ok(full) } else { Err(format!("`{path}` is outside the working directory")) }
    }

    /// Like [`resolve`](Self::resolve), but the path may name a file that does not exist yet. The
    /// nearest existing ancestor is resolved (so symlinks cannot lead out), and what is missing
    /// below it must be plain names: no `..`.
    #[inline]
    /// The full path to write `path` to, if it stays inside the working directory.
    ///
    /// # Errors
    ///
    /// Fails if `path` is outside the working directory.
    pub fn resolve_for_write(&self, path: &str) -> Result<PathBuf, String> {
        let joined = self.root.join(path);
        if joined.symlink_metadata().is_ok() {
            return self.resolve(path);
        }
        let ancestor = joined
            .ancestors()
            .find(|ancestor| ancestor.symlink_metadata().is_ok())
            .ok_or_else(|| format!("cannot access `{path}`"))?;
        let base = ancestor.canonicalize().map_err(|e| format!("cannot access `{path}`: {e}"))?;
        let missing = joined.strip_prefix(ancestor).unwrap_or_else(|_| Path::new(""));
        if !missing.components().all(|part| matches!(part, Component::Normal(_))) {
            return Err(format!("`{path}` is not a plain path inside the working directory"));
        }
        let full = base.join(missing);
        if full.starts_with(&*self.root) { Ok(full) } else { Err(format!("`{path}` is outside the working directory")) }
    }

    #[must_use]
    #[inline]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// How [`Workspace::read`] treats a file.
#[derive(Debug, Clone, Copy)]
pub struct ReadPolicy {
    /// Whether the path must stay inside the working directory.
    confined: bool,
    /// Refuse larger files, if set.
    max_bytes: Option<u64>,
}

impl ReadPolicy {
    /// Any text file on disk, of any size.
    #[must_use]
    #[inline]
    pub const fn anywhere() -> Self {
        Self { confined: false, max_bytes: None }
    }

    /// Any text file on disk up to `max_bytes`: what the model's `read_file` tool may do. A file
    /// is read whole before it is cut for the model, so there has to be a limit.
    #[must_use]
    #[inline]
    pub const fn anywhere_up_to(max_bytes: u64) -> Self {
        Self { confined: false, max_bytes: Some(max_bytes) }
    }

    /// Only files inside the workspace, up to `max_bytes`: what a prompt reference may attach.
    #[must_use]
    #[inline]
    pub const fn inside(max_bytes: u64) -> Self {
        Self { confined: true, max_bytes: Some(max_bytes) }
    }
}

/// Why a file could not be read. Displays as the message a tool reports to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(String);

impl fmt::Display for Refusal {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn current_dir() -> String {
    ".".to_owned()
}

#[must_use]
#[inline]
/// Cuts `text` to `max_bytes` on a character boundary, with a note.
pub fn truncate(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let kept = text.get(..end).unwrap_or_default();
    format!("{kept}\n[truncated: showing the first {end} of {} bytes]", text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::temp_tree::TempTree;

    #[test]
    fn resolves_paths_inside_the_workspace() {
        let tree = TempTree::new("resolve").file("a.txt", "x").file("sub/b.txt", "y");
        let ws = tree.workspace();
        assert!(ws.resolve("a.txt").unwrap().ends_with("a.txt"));
        ws.resolve("sub/../a.txt").unwrap();
        ws.resolve(".").unwrap();
    }

    #[test]
    fn refuses_paths_that_escape() {
        let tree = TempTree::new("escape");
        let ws = tree.workspace();
        ws.resolve("../../../etc/hosts").unwrap_err();
        assert!(ws.resolve("/etc/hosts").unwrap_err().contains("outside the working directory"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_that_escape() {
        let outside = TempTree::new("outside").file("secret.txt", "s");
        let tree = TempTree::new("link").symlink("link", outside.path());
        let err = tree.workspace().resolve("link/secret.txt").unwrap_err();
        assert!(err.contains("outside the working directory"), "{err}");
    }

    #[test]
    fn reads_text_and_rejects_directories_and_binaries() {
        let tree = TempTree::new("read").file("a.txt", "hello").file("bin", "a\0b").file("d/x", "");
        let ws = tree.workspace();
        assert_eq!(ws.read("a.txt", ReadPolicy::anywhere()).unwrap(), "hello");
        assert!(ws.read("d", ReadPolicy::anywhere()).unwrap_err().to_string().contains("not a file"));
        assert!(ws.read("bin", ReadPolicy::anywhere()).unwrap_err().to_string().contains("binary"));
        assert!(ws.read("missing", ReadPolicy::anywhere()).unwrap_err().to_string().contains("cannot access"));
    }

    #[test]
    fn reads_text_outside_the_workspace() {
        let outside = TempTree::new("read-outside").file("note.txt", "elsewhere");
        let tree = TempTree::new("read-inside");
        let absolute = outside.path().join("note.txt");
        assert_eq!(tree.workspace().read(absolute.to_str().unwrap(), ReadPolicy::anywhere()).unwrap(), "elsewhere");
    }

    #[test]
    fn a_confined_read_stays_inside_and_under_the_limit() {
        let outside = TempTree::new("in-outside").file("o.txt", "far");
        let tree = TempTree::new("in-inside").file("ok.txt", "fine").file("big.txt", "0123456789");
        let ws = tree.workspace();
        let inside = ReadPolicy::inside(8);
        assert_eq!(ws.read("ok.txt", inside).unwrap(), "fine");
        assert!(ws.read("big.txt", inside).unwrap_err().to_string().contains("too big"));
        let absolute = outside.path().join("o.txt");
        let err = ws.read(absolute.to_str().unwrap(), inside).unwrap_err();
        assert!(err.to_string().contains("outside the working directory"), "{err}");
    }

    #[test]
    fn display_path_is_relative_to_the_root() {
        let tree = TempTree::new("display").file("sub/b.txt", "y");
        let ws = tree.workspace();
        let full = ws.resolve("sub/b.txt").unwrap();
        assert_eq!(ws.display_path(&full).to_string(), "sub/b.txt");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("short".into(), 100), "short");
        let cut = truncate("\u{e9}".repeat(10), 5);
        assert!(cut.starts_with("\u{e9}\u{e9}") && cut.contains("[truncated"));
    }

    #[test]
    fn a_new_path_resolves_inside_the_workspace_only() {
        let tree = TempTree::new("new-path").file("sub/a.txt", "x");
        let ws = tree.workspace();
        let full = ws.resolve_for_write("sub/deep/new.txt").unwrap();
        assert!(full.ends_with("sub/deep/new.txt"));
        assert!(ws.resolve_for_write("sub/a.txt").is_ok(), "existing files too");
        ws.resolve_for_write("../new.txt").unwrap_err();
        ws.resolve_for_write("missing/../../new.txt").unwrap_err();
        ws.resolve_for_write("/tmp/thoth-never-there/new.txt").unwrap_err();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_path_cannot_leave_through_a_symlink() {
        let outside = TempTree::new("new-outside");
        let tree = TempTree::new("new-link").symlink("link", outside.path());
        let err = tree.workspace().resolve_for_write("link/new.txt").unwrap_err();
        assert!(err.contains("outside the working directory"), "{err}");
    }
}
