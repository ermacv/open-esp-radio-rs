//! Extract and compile Blobray without access to repository path dependencies.

use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Context, Result, paths, process};

const WORKSPACE: &str = r#"

[workspace]
members = [
    ".",
    "crates/analysis-model",
    "crates/backend-riscv",
    "crates/contracts",
    "crates/execution-model",
    "crates/register-model",
    "crates/semantics",
]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.97.1"
license = "MIT OR Apache-2.0"
repository = "https://github.com/ermacv/blobray"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[workspace.lints.clippy]
debug_assert_with_mut_call = "deny"

[profile.blobray]
inherits = "dev"
opt-level = 3
"#;

pub fn run(context: &Context) -> Result<()> {
    let toolchain = selected_toolchain(context, std::env::var_os("RUSTUP_TOOLCHAIN"))?;
    let scratch = tempfile::Builder::new()
        .prefix("blobray-standalone-")
        .tempdir()?;
    let root = scratch.path().canonicalize()?;
    extract(
        &context.root.join("tools/blobray"),
        &root,
        paths::source_files(context)?,
    )?;
    let mut manifest = fs::OpenOptions::new()
        .append(true)
        .open(root.join("Cargo.toml"))?;
    manifest.write_all(WORKSPACE.as_bytes())?;
    drop(manifest);

    let output = process::capture(command(context, &root, &toolchain).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]))?;
    let metadata: cargo_metadata::Metadata = serde_json::from_slice(&output.stdout)?;
    require_contained_dependencies(
        &root,
        metadata.packages.iter().flat_map(|package| {
            package
                .dependencies
                .iter()
                .filter_map(|dependency| dependency.path.as_ref().map(|path| path.as_std_path()))
        }),
    )?;
    process::run(command(context, &root, &toolchain).arg("generate-lockfile"))?;
    process::run(command(context, &root, &toolchain).args([
        "check",
        "--workspace",
        "--all-targets",
        "--locked",
    ]))?;
    eprintln!("standalone Blobray workspace is self-contained");
    Ok(())
}

fn selected_toolchain(context: &Context, caller: Option<OsString>) -> Result<OsString> {
    if let Some(caller) = caller {
        return Ok(caller);
    }
    let document: toml::Value = toml::from_str(&fs::read_to_string(
        context.root.join("rust-toolchain.toml"),
    )?)?;
    let channel = document
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .filter(|channel| !channel.is_empty())
        .ok_or("repository Rust toolchain channel is missing")?;
    Ok(channel.into())
}

fn command(context: &Context, root: &Path, toolchain: &OsStr) -> Command {
    let mut command = context.cargo();
    // Discover Cargo config from the extraction, not the parent repository;
    // neither an inherited target directory nor its artifacts prove autonomy.
    command
        .current_dir(root)
        .env("RUSTUP_TOOLCHAIN", toolchain)
        .env("CARGO_TARGET_DIR", root.join("target"));
    command
}

fn extract(source: &Path, destination: &Path, files: Vec<PathBuf>) -> Result<()> {
    let source = source.canonicalize()?;
    for file in files {
        let Ok(relative) = file.strip_prefix(&source) else {
            continue;
        };
        if !file.canonicalize()?.starts_with(&source) {
            return Err(format!(
                "Blobray source escapes its extraction boundary: {}",
                file.display()
            )
            .into());
        }
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().ok_or("source file has no parent")?)?;
        fs::copy(file, target)?;
    }
    if !destination.join("Cargo.toml").is_file() {
        return Err("Blobray source inventory omitted its root manifest".into());
    }
    Ok(())
}

fn require_contained_dependencies<'a>(
    root: &Path,
    dependencies: impl IntoIterator<Item = &'a Path>,
) -> Result<()> {
    let root = root.canonicalize()?;
    for dependency in dependencies {
        if !dependency.canonicalize()?.starts_with(&root) {
            return Err(format!(
                "extracted Blobray contains a path dependency outside its workspace: {}",
                dependency.display()
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
