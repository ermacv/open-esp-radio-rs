//! Extract and compile the Blobray workspace and its complete path-package
//! closure, including test dependencies, without access to the repository.

use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::Result;
use oer_process as process;
use oer_process::Checkout;

const WORKSPACE: &str = r#"

[workspace]
members = []
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

pub fn run(context: &Checkout) -> Result<()> {
    let toolchain = selected_toolchain(context, std::env::var_os("RUSTUP_TOOLCHAIN"))?;
    let scratch = tempfile::Builder::new()
        .prefix("blobray-standalone-")
        .tempdir()?;
    let root = scratch.path().canonicalize()?;
    extract_workspace(context, &root)?;
    let output = process::capture(command(context, &root, &toolchain).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--offline",
    ]))?;
    let metadata: cargo_metadata::Metadata = serde_json::from_slice(&output.stdout)?;
    require_contained_dependencies(
        &root,
        metadata.packages.iter().flat_map(|package| {
            package
                .dependencies
                .iter()
                .filter_map(|d| d.path.as_ref().map(|p| p.as_std_path()))
        }),
    )?;
    process::run(command(context, &root, &toolchain).args(["test", "--workspace", "--offline"]))?;
    eprintln!("standalone Blobray core and tests are self-contained");
    Ok(())
}

fn extract_workspace(context: &Checkout, root: &Path) -> Result<()> {
    let repo = oer_repo::Repo::from_git(&context.root)?;
    let model = oer_repo::Model::load(&repo)?;
    let workspace_manifest = oer_toolchain::workspace::BLOBRAY.manifest();
    let roots: Vec<_> = model.members(&workspace_manifest).collect();
    if roots.is_empty() {
        return Err(format!("{workspace_manifest} has no workspace members").into());
    }
    let members: Vec<_> = model
        .closure(
            &roots,
            oer_repo::closure::Edges::All,
            None,
            &oer_repo::closure::Features::All,
        )?
        .into_iter()
        .map(|package| package.directory.clone())
        .collect();
    // Members keep their repository paths, so the RV32 crates Blobray takes
    // by path are extracted beside it and no path dependency is rewritten.
    let files: Vec<_> = repo.files().map(|file| context.root.join(file)).collect();
    for member in &members {
        extract(&context.root.join(member), &root.join(member), &files)?;
    }
    let mut workspace: toml::Value = toml::from_str(WORKSPACE)?;
    workspace["workspace"]["members"] =
        toml::Value::Array(members.iter().cloned().map(toml::Value::String).collect());
    fs::write(root.join("Cargo.toml"), toml::to_string(&workspace)?)?;
    Ok(())
}

fn selected_toolchain(context: &Checkout, caller: Option<OsString>) -> Result<OsString> {
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

fn command(context: &Checkout, root: &Path, toolchain: &OsStr) -> Command {
    let mut command = oer_toolchain::cargo_in(&context.root);
    // Discover Cargo config from the extraction, not the parent repository;
    // neither an inherited target directory nor its artifacts prove autonomy.
    command
        .current_dir(root)
        .env("RUSTUP_TOOLCHAIN", toolchain)
        .env("CARGO_TARGET_DIR", root.join("target"));
    command
}

fn extract(source: &Path, destination: &Path, files: &[PathBuf]) -> Result<()> {
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
