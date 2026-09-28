//! Install the stand's operational commands as a prebuilt tool from `main`.
//!
//! `cargo hil queue`, `lease`, `devices` and the other operational commands
//! otherwise build this xtask from the caller's working tree first: minutes
//! after every edit, unavailable while the tree does not compile, and six
//! checkouts on six commits writing the one shared arbiter state with six
//! different binaries. `cargo xtask stand-install` builds the xtask of
//! `origin/main` once, in its own clone, and installs `oer-stand` on the
//! user's PATH: a wrapper that runs that binary against the caller's checkout
//! (`--root` is the caller's Git top level, so owners and local paths stay the
//! caller's).
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{Context, Result, process};

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is required to install the stand tool".into())
}

/// Where the tool's clone, build and binary live.
pub fn tool_directory() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map_or_else(|| home().map(|home| home.join(".local/share")), Ok)?;
    Ok(base.join("open-esp-radio/stand-tool"))
}

/// The wrapper script: the installed binary against the caller's checkout.
pub fn wrapper(binary: &Path, fallback_root: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Installed by `cargo xtask stand-install`; operational HIL stand commands\n\
         # without building the caller's tree. Runs against the caller's checkout.\n\
         root=$(git rev-parse --show-toplevel 2>/dev/null) || root='{}'\n\
         exec '{}' --root \"$root\" hil \"$@\"\n",
        fallback_root.display(),
        binary.display()
    )
}

pub fn run(ctx: &Context) -> Result<()> {
    let tool = tool_directory()?;
    let source = tool.join("src");
    let remote = String::from_utf8(
        process::capture(ctx.command("git").args(["remote", "get-url", "origin"]))?.stdout,
    )?;
    let remote = remote.trim();
    if !source.join(".git").is_dir() {
        fs::create_dir_all(&tool)?;
        process::run(
            ctx.command("git")
                .args(["clone", "--quiet", remote])
                .arg(&source),
        )?;
    }
    process::run(
        ctx.command("git")
            .arg("-C")
            .arg(&source)
            .args(["fetch", "--quiet", "origin", "main"]),
    )?;
    process::run(ctx.command("git").arg("-C").arg(&source).args([
        "checkout",
        "--quiet",
        "--detach",
        "FETCH_HEAD",
    ]))?;
    let commit = String::from_utf8(
        process::capture(ctx.command("git").arg("-C").arg(&source).args([
            "rev-parse",
            "--short=12",
            "HEAD",
        ]))?
        .stdout,
    )?;
    process::run(ctx.cargo().current_dir(&source).args([
        "build",
        "--locked",
        "--release",
        "-p",
        "oer-xtask",
    ]))?;
    let bin = tool.join("bin");
    fs::create_dir_all(&bin)?;
    let installed = bin.join("oer-xtask");
    let staged = bin.join(".oer-xtask.new");
    fs::copy(source.join("target/release/oer-xtask"), &staged)?;
    fs::rename(&staged, &installed)?;
    let script_directory = home()?.join(".local/bin");
    fs::create_dir_all(&script_directory)?;
    let script = script_directory.join("oer-stand");
    fs::write(&script, wrapper(&installed, &source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(tool.join("installed-commit"), commit.trim())?;
    println!(
        "installed oer-stand from main {} ({})",
        commit.trim(),
        script.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapper_runs_the_installed_binary_against_the_callers_checkout() {
        let text = wrapper(Path::new("/tool/bin/oer-xtask"), Path::new("/tool/src"));
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(
            text.contains("root=$(git rev-parse --show-toplevel 2>/dev/null) || root='/tool/src'")
        );
        assert!(text.contains("exec '/tool/bin/oer-xtask' --root \"$root\" hil \"$@\""));
    }
}
