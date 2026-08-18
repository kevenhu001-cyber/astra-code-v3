//! Single source of truth for the Astra home directory: `$ASTRA_HOME` or
//! `<home>/.astra`. Shared by `xai-grok-config` and `xai-fast-worktree`.
//!
//! Which function to call:
//! - [`astra_home`]: the usual choice, a cached, created path to build on.
//! - [`user_astra_home`]: `None` instead of a cwd fallback when no home resolves.
//! - [`default_astra_home`]: the `<home>/.astra` default, ignoring `$ASTRA_HOME`, so callers can detect an override.
//! - [`resolve_astra_home`]: a fresh, uncached resolve.
//!
//! TODO: collapse these getters by threading the path through config as an
//! explicit value.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `<home>/.astra`, canonicalized via `dunce` (not `std::fs::canonicalize`,
/// which yields Windows `\\?\` verbatim paths).
fn astra_home_in(home: &Path) -> PathBuf {
    dunce::canonicalize(home)
        .unwrap_or_else(|_| home.to_path_buf())
        .join(".astra")
}

/// `$ASTRA_HOME` verbatim when non-empty, else `<home>/.astra`. The env value is
/// used as-is (not canonicalized) so it stays stable and comparable: callers do
/// literal prefix checks against it, and downstream symlink guards must still see
/// its original components.
fn resolve_astra_home_from(
    astra_home_env: Option<&OsStr>,
    os_home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(env) = astra_home_env.filter(|env| !env.is_empty()) {
        return Some(PathBuf::from(env));
    }
    os_home.map(astra_home_in)
}

/// Resolve the Astra home from the environment (fresh, no cache); `None` if neither resolves.
pub fn resolve_astra_home() -> Option<PathBuf> {
    resolve_astra_home_from(
        std::env::var_os("ASTRA_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// The default `<home>/.astra`, used when `$ASTRA_HOME` is unset.
pub fn default_astra_home() -> PathBuf {
    astra_home_in(&dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
}

/// The Astra home, created if missing and cached for the process; falls back to
/// [`default_astra_home`] when neither `$ASTRA_HOME` nor a home resolves.
pub fn astra_home() -> PathBuf {
    static ASTRA_HOME: OnceLock<PathBuf> = OnceLock::new();
    ASTRA_HOME
        .get_or_init(|| {
            let home = resolve_astra_home().unwrap_or_else(default_astra_home);
            if let Err(err) = std::fs::create_dir_all(&home) {
                tracing::warn!(path = %home.display(), %err, "failed to create astra home");
            }
            home
        })
        .clone()
}

/// Like [`astra_home`], but `None` when no home resolves (no cwd fallback).
pub fn user_astra_home() -> Option<PathBuf> {
    resolve_astra_home().is_some().then(astra_home)
}

// ---------------------------------------------------------------------------
// Backward-compatible aliases for the legacy `grok_home*` API. These wrappers
// keep the internal Rust call sites compiling without touching every
// `use xai_grok_home::grok_home;` line; the values they return resolve to
// `<home>/.astra` (or `$ASTRA_HOME`) under the hood. New code should prefer
// the `astra_*` functions above.
// ---------------------------------------------------------------------------

/// Deprecated: use [`astra_home`] instead. Returns the same path.
#[deprecated(note = "use `astra_home` instead; the underlying directory is now `.astra`")]
pub fn grok_home() -> PathBuf {
    astra_home()
}

/// Deprecated: use [`default_astra_home`] instead.
#[deprecated(note = "use `default_astra_home` instead")]
pub fn default_grok_home() -> PathBuf {
    default_astra_home()
}

/// Deprecated: use [`user_astra_home`] instead.
#[deprecated(note = "use `user_astra_home` instead")]
pub fn user_grok_home() -> Option<PathBuf> {
    user_astra_home()
}

/// Deprecated: use [`resolve_astra_home`] instead.
#[deprecated(note = "use `resolve_astra_home` instead")]
pub fn resolve_grok_home() -> Option<PathBuf> {
    resolve_astra_home()
}

#[allow(dead_code)]
#[deprecated(note = "internal helper; use [`astra_home_in`] instead")]
fn grok_home_in(home: &Path) -> PathBuf {
    astra_home_in(home)
}

#[allow(dead_code)]
#[deprecated(note = "internal helper; use [`resolve_astra_home_from`] instead")]
fn resolve_grok_home_from(
    grok_home_env: Option<&OsStr>,
    os_home: Option<&Path>,
) -> Option<PathBuf> {
    resolve_astra_home_from(grok_home_env, os_home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::ffi::OsString;

    #[test]
    fn env_wins_over_os_home() {
        let resolved =
            resolve_astra_home_from(Some(OsStr::new("/custom/home")), Some(Path::new("/home/u")));
        assert_eq!(resolved, Some(PathBuf::from("/custom/home")));
    }

    #[test]
    fn env_used_verbatim_even_when_it_exists() {
        // A real, existing dir whose canonical form differs (macOS symlinks
        // `/var` -> `/private/var`): the env value must come back unchanged.
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_astra_home_from(Some(tmp.path().as_os_str()), None);
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn empty_env_falls_through_to_os_home() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_astra_home_from(Some(&OsString::new()), Some(tmp.path()));
        assert_eq!(
            resolved,
            Some(dunce::canonicalize(tmp.path()).unwrap().join(".astra"))
        );
    }

    #[test]
    fn default_astra_home_has_no_verbatim_prefix() {
        // The reason we canonicalize via dunce: std::fs::canonicalize yields
        // `\\?\` verbatim paths on Windows that break git and byte-exact
        // comparisons. No-op assertion on Unix.
        let home = default_astra_home();
        assert!(!home.to_string_lossy().starts_with(r"\\?\"));
        assert!(home.ends_with(".astra"));
    }

    #[test]
    fn none_when_nothing_resolves() {
        assert_eq!(
            resolve_astra_home_from(/* astra_home_env */ None, /* os_home */ None),
            None
        );
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_aliases_resolve_to_astra_path() {
        // The deprecated wrappers must track the new implementation: callers
        // that still reach for `grok_home()` see the same `.astra` directory.
        assert!(grok_home().ends_with(".astra"));
        assert!(default_grok_home().ends_with(".astra"));
        assert_eq!(user_grok_home().unwrap().extension().is_some(), false);
    }
}