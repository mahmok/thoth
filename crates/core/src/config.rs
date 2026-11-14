use std::env;
use std::path::PathBuf;

#[inline]
/// The configuration directory of the app, if it can be found.
pub fn dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData/Roaming")))?;

    #[cfg(not(windows))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(base.join(crate::APP_NAME))
}
