//! The separate Blobray host-tool workspace and its build outputs, as every
//! caller that builds or runs Blobray from the root workspace reaches them:
//! the vendor scenarios and probes, the register inventory, the image
//! pipeline's final radio target audit and the conformance check.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Workspace manifest, relative to the repository root.
pub const MANIFEST: &str = "tools/blobray/Cargo.toml";

/// Blobray keeps its own target directory; builds never share the radio
/// workspace's Cargo lock or artifacts.
pub fn target_directory(root: &Path) -> PathBuf {
    root.join("tools/blobray/target")
}

/// A Cargo command for `subcommand` in the Blobray workspace of `root` with
/// its fixed target directory, independent of the caller's
/// `CARGO_TARGET_DIR`.
pub fn cargo(root: &Path, subcommand: &str) -> Command {
    let mut command = crate::cargo_in(root);
    command
        .env("CARGO_TARGET_DIR", target_directory(root))
        .arg(subcommand)
        .arg("--manifest-path")
        .arg(root.join(MANIFEST));
    command
}

/// The optimized executable `name` built with `--profile blobray`.
pub fn binary(root: &Path, name: &str) -> PathBuf {
    target_directory(root).join("blobray").join(name)
}

/// Build the `blobray` host with `--profile blobray` and return its path.
pub fn host(root: &Path) -> crate::Result<PathBuf> {
    oer_process::run(cargo(root, "build").args([
        "--locked",
        "--profile",
        "blobray",
        "-p",
        "blobray-cli",
        "--bin",
        "blobray",
    ]))?;
    Ok(binary(root, "blobray"))
}
