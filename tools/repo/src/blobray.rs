//! Location of the separate Blobray host-tool workspace and its build outputs.
use crate::Context;
use std::{path::PathBuf, process::Command};

/// Workspace manifest, relative to the repository root.
pub const MANIFEST: &str = "tools/blobray/Cargo.toml";

/// Blobray keeps its own target directory; builds never share the radio
/// workspace's Cargo lock or artifacts.
pub fn target_directory(ctx: &Context) -> PathBuf {
    ctx.root.join("tools/blobray/target")
}

/// A Cargo command for `subcommand` in the Blobray workspace with a fixed
/// target directory, independent of the caller's `CARGO_TARGET_DIR`.
pub fn cargo(ctx: &Context, subcommand: &str) -> Command {
    let mut command = ctx.cargo();
    command
        .env("CARGO_TARGET_DIR", target_directory(ctx))
        .arg(subcommand)
        .arg("--manifest-path")
        .arg(ctx.root.join(MANIFEST));
    command
}

/// The optimized `blobray` executable built with `--profile blobray`.
pub fn binary(ctx: &Context, name: &str) -> PathBuf {
    target_directory(ctx).join("blobray").join(name)
}
