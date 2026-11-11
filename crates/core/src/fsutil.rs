use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::process;
use tokio::fs as tokio_fs;

#[inline]
/// Reads a Markdown file and splits its `---` frontmatter from the body.
///
/// # Errors
///
/// Fails if the file cannot be read.
pub async fn read_markdown_with_frontmatter(path: &Path) -> io::Result<(Option<String>, String)> {
    let text = tokio_fs::read_to_string(path).await?;

    if let Some(rest) = text.strip_prefix("---\n")
        && let Some((frontmatter, after)) = rest.split_once("\n---")
    {
        let content = after.strip_prefix('\n').unwrap_or(after).to_owned();

        return Ok((Some(frontmatter.to_owned()), content));
    }

    Ok((None, text))
}

/// Replaces the file at `path` with `bytes` all at once.
///
/// The bytes go to a temporary file beside it, which then takes its place. A crash leaves the
/// old file or the new one, never half of either, and the file keeps its permissions.
#[inline]
/// Replaces the file with `bytes` in one step.
///
/// # Errors
///
/// Fails if the file cannot be written or replaced.
pub fn write_replacing(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_file_name(format!(".{name}.thoth-{}-{unique}.tmp", process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&temp, meta.permissions())?;
        }
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _unused = fs::remove_file(&temp);
    }
    result
}

#[inline]
/// Writes the file atomically, with owner-only permissions.
///
/// # Errors
///
/// Fails if the file cannot be written or replaced.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    let mut file = private_options().open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

#[cfg(unix)]
fn private_options() -> fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).clone()
}

#[cfg(not(unix))]
fn private_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::temp_tree::TempTree;

    #[test]
    fn replacing_a_file_leaves_no_temporary_file_and_keeps_permissions() {
        let tree = TempTree::new("fs-replace").file("a.txt", "old");
        let path = tree.path().join("a.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        };
        write_replacing(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let names: Vec<_> = fs::read_dir(tree.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o750);
        }
    }

    #[test]
    fn replacing_creates_a_missing_file() {
        let tree = TempTree::new("fs-create");
        let path = tree.path().join("new.txt");
        write_replacing(&path, b"hi").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hi");
    }
}
