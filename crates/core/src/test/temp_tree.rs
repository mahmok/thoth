use core::sync::atomic::{AtomicUsize, Ordering};
use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process;

use crate::workspace::Workspace;

/// A throwaway directory tree under the system temp dir, removed on drop.
#[derive(Debug)]
pub struct TempTree {
    dir: PathBuf,
}

impl TempTree {
    #[must_use]
    #[inline]
    /// Writes a file, creating its folders.
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be written.
    pub fn file(self, path: &str, content: &str) -> Self {
        let full = self.dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
        self
    }

    #[inline]
    /// Creates an empty directory that is removed when dropped.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created.
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = env::temp_dir().join(format!(
            "{}-{name}-{}-{}",
            crate::APP_NAME,
            process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _unused = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    #[must_use]
    #[inline]
    pub fn path(&self) -> &Path {
        &self.dir
    }

    #[cfg(unix)]
    #[must_use]
    #[inline]
    /// Adds a symbolic link.
    ///
    /// # Panics
    ///
    /// Panics if the link cannot be made.
    pub fn symlink(self, name: &str, target: &Path) -> Self {
        unix_fs::symlink(target, self.dir.join(name)).unwrap();
        self
    }

    #[must_use]
    #[inline]
    /// A Workspace rooted at the directory.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be opened as a Workspace.
    pub fn workspace(&self) -> Workspace {
        Workspace::new(&self.dir).unwrap()
    }
}

impl Drop for TempTree {
    #[inline]
    fn drop(&mut self) {
        let _unused = fs::remove_dir_all(&self.dir);
    }
}
