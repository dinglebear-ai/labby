//! Pure, dependency-free runtime helpers shared by the gateway-extraction
//! crates.
//!
//! These mirror the small leaf helpers in the `lab` binary's
//! `dispatch::helpers` module, but without the binary-only test override hooks
//! (`TEST_LABBY_HOME` / thread-local `ENV_OVERRIDE`). `labby-gateway` and friends
//! use these production-path versions; the `lab` binary keeps its own copies
//! with the test seams its unit tests rely on.

use std::path::PathBuf;

/// Resolve the lab home directory: `$LABBY_HOME` if set and non-empty, else
/// `$HOME/.labby/` (`USERPROFILE` is the Windows fallback).
///
/// Falls back to a fixed absolute directory below the system temporary
/// directory when neither variable is set. This keeps daemon and Windows CI
/// callers from anchoring durable state to the process working directory.
#[must_use]
pub fn lab_home() -> PathBuf {
    if let Ok(home) = std::env::var("LABBY_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join(".labby"),
        _ => match std::env::var("USERPROFILE") {
            Ok(home) if !home.is_empty() => PathBuf::from(home).join(".labby"),
            _ => std::env::temp_dir().join("labby"),
        },
    }
}

/// The user's home directory (`$HOME`), or `None` when unset/empty.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => None,
    }
}

/// Read an environment variable, returning `None` if absent or empty.
#[must_use]
pub fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
