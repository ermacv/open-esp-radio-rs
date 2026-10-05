//! Where the stand keeps its state, after the XDG base directories
//! (`oer-durable`'s `xdg`): every checkout of the user shares them.
//!
//! - the stand file: `open-esp-radio/stand.toml` in `$XDG_CONFIG_HOME`
//!   (else `~/.config`), or `$OER_STAND_FILE`;
//! - the arbiter's state (queue, holders, history, board journal, jobs):
//!   `open-esp-radio/arbiter` in `$XDG_CACHE_HOME` (else `~/.cache`), or
//!   `$OER_STAND_ARBITER_DIR`, which tests and a nested private arbiter use;
//! - the lock files, the arbiter's final exclusion layer:
//!   `open-esp-radio/leases` in the XDG cache directory. They do not move
//!   with `$OER_STAND_ARBITER_DIR`, so every process on the host excludes
//!   every other one from a board, whichever arbiter ordered it.
//!
//! The stand file never moves with the arbiter's directory: the arbiter, the
//! runner's laboratory configuration and the board controls read the same
//! file.

use std::{ffi::OsString, path::PathBuf};

use oer_durable::xdg::{Base, overridable_in, path_in};

/// Overrides the arbiter's state directory, for tests and for a nested
/// arbiter of a bisection's revision.
pub const ARBITER_ENV: &str = "OER_STAND_ARBITER_DIR";

/// Names the stand file instead of the user's.
pub const STAND_FILE_ENV: &str = "OER_STAND_FILE";

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn located(path: oer_durable::Result<PathBuf>) -> crate::Result<PathBuf> {
    path.map_err(|cause| crate::Error(cause.to_string()))
}

/// The user's stand file.
pub fn stand_file() -> crate::Result<PathBuf> {
    stand_file_in(environment)
}

/// The arbiter's state directory.
pub fn arbiter() -> crate::Result<PathBuf> {
    arbiter_in(environment)
}

/// The directory of the lock files.
pub fn locks() -> crate::Result<PathBuf> {
    locks_in(environment)
}

fn stand_file_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    if let Some(path) = variable(STAND_FILE_ENV).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    located(path_in(Base::Config, "stand.toml", variable))
}

fn arbiter_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    located(overridable_in(
        ARBITER_ENV,
        Base::Cache,
        "arbiter",
        variable,
    ))
}

fn locks_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    located(path_in(Base::Cache, "leases", variable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn environment(name: &str) -> Option<OsString> {
        match name {
            "XDG_CONFIG_HOME" => Some("/config".into()),
            "XDG_CACHE_HOME" => Some("/cache".into()),
            "HOME" => Some("/home/u".into()),
            _ => None,
        }
    }

    #[test]
    fn the_stand_state_lies_below_the_xdg_bases() {
        assert_eq!(
            stand_file_in(environment).unwrap(),
            Path::new("/config/open-esp-radio/stand.toml")
        );
        assert_eq!(
            arbiter_in(environment).unwrap(),
            Path::new("/cache/open-esp-radio/arbiter")
        );
        assert_eq!(
            locks_in(environment).unwrap(),
            Path::new("/cache/open-esp-radio/leases")
        );
        let home_only = |name: &str| (name == "HOME").then(|| OsString::from("/home/u"));
        assert_eq!(
            stand_file_in(home_only).unwrap(),
            Path::new("/home/u/.config/open-esp-radio/stand.toml")
        );
    }

    #[test]
    fn a_private_arbiter_keeps_the_stand_file_and_the_locks() {
        let private = |name: &str| {
            (name == ARBITER_ENV)
                .then(|| OsString::from("/tmp/private"))
                .or_else(|| environment(name))
        };
        assert_eq!(arbiter_in(private).unwrap(), Path::new("/tmp/private"));
        assert_eq!(
            stand_file_in(private).unwrap(),
            stand_file_in(environment).unwrap()
        );
        assert_eq!(locks_in(private).unwrap(), locks_in(environment).unwrap());
    }
}
