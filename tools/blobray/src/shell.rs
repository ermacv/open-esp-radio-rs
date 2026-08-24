//! Safe rendering for the POSIX-style copy/paste commands emitted by Blobray.

use std::ffi::OsStr;

/// Keep ordinary repository paths readable while protecting spaces and shell
/// metacharacters in commands presented to users.
pub(crate) fn arg(value: impl AsRef<OsStr>) -> String {
    let value = value.as_ref().to_string_lossy();
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:=,@%".contains(&byte))
    {
        return value.into_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::arg;

    #[test]
    fn arguments_are_readable_and_safe_for_copy_paste() {
        assert_eq!(
            arg("project/vendor-project.toml"),
            "project/vendor-project.toml"
        );
        assert_eq!(
            arg("project tree/vendor-project.toml"),
            "'project tree/vendor-project.toml'"
        );
        assert_eq!(
            arg("owner's/registers.svd"),
            "'owner'\"'\"'s/registers.svd'"
        );
        assert_eq!(arg(""), "''");
    }
}
