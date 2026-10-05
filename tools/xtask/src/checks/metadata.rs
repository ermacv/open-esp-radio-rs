use crate::{Result, cargo};
use oer_process::Checkout;
use std::{collections::BTreeSet, fs};

mod lints;
mod pins;

/// Every Cargo workspace of the repository, by its root manifest, as the
/// repository model (`oer-repo`) discovers them from the manifests as text.
pub fn workspaces(context: &Checkout) -> Result<BTreeSet<std::path::PathBuf>> {
    let workspaces: BTreeSet<_> = super::common::model(context)?
        .workspaces()
        .iter()
        .map(|manifest| context.root.join(manifest))
        .collect();
    if workspaces.is_empty() {
        return Err("no Cargo workspace found".into());
    }
    Ok(workspaces)
}

/// Brings every workspace lock in line with its manifests (`cargo xtask
/// lock`), so a dependency or pin change updates all locks in one step.
pub fn update_locks(context: &Checkout) -> Result<()> {
    for manifest in workspaces(context)? {
        let lock = manifest.with_file_name("Cargo.lock");
        let before = fs::read_to_string(&lock).ok();
        oer_process::capture(
            oer_toolchain::cargo_in(&context.root)
                .args(["metadata", "--format-version", "1"])
                // The one Cargo call of the repository that may go online.
                .args(["--config", "net.offline=false"])
                .arg("--manifest-path")
                .arg(&manifest),
        )?;
        let changed = before != fs::read_to_string(&lock).ok();
        println!(
            "{} {}",
            if changed { "updated  " } else { "unchanged" },
            lock.strip_prefix(&context.root)?.display()
        );
    }
    Ok(())
}

/// Fails, naming every one, when a workspace's lock no longer matches its
/// manifests (`cargo xtask lock --check`). Offline: a dependency missing
/// from the local cache is reported as such, and `cargo tidy fetch`
/// downloads it.
pub fn check_locks(context: &Checkout, manifests: &[std::path::PathBuf]) -> Result<()> {
    let mut stale = Vec::new();
    for manifest in manifests {
        let result = oer_process::capture(
            oer_toolchain::cargo_in(&context.root)
                .args(["metadata", "--format-version", "1", "--locked"])
                .arg("--manifest-path")
                .arg(manifest),
        );
        if let Err(error) = result {
            let reason = error
                .to_string()
                .lines()
                .rev()
                .find(|line| line.starts_with("error"))
                .unwrap_or("cargo metadata --locked failed")
                .to_owned();
            stale.push(format!(
                "{}: {reason}",
                manifest.strip_prefix(&context.root)?.display()
            ));
        }
    }
    if stale.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} workspace lock(s) do not match their manifests; `cargo xtask lock` updates them:\n{}",
        stale.len(),
        stale.join("\n")
    )
    .into())
}

pub fn run(context: &Checkout) -> Result<usize> {
    let workspaces = workspaces(context)?;
    let mut islands = Vec::with_capacity(workspaces.len());
    let mut stale = Vec::new();
    for manifest in &workspaces {
        let relative = manifest.strip_prefix(&context.root)?.to_path_buf();
        println!("checking locked Cargo metadata: {}", relative.display());
        // Report every stale lock at once: one push round per lock is the
        // cost this avoids.
        let graph = match cargo::metadata(context, manifest, &[], None, true) {
            Ok(graph) => graph,
            Err(error) => {
                stale.push(format!("{}: {error}", relative.display()));
                continue;
            }
        };
        let mut members = Vec::new();
        for package in graph.metadata.workspace_packages() {
            let path = package.manifest_path.as_std_path();
            members.push((
                package.name.to_string(),
                path.strip_prefix(&context.root)?.to_path_buf(),
                fs::read_to_string(path)?,
            ));
        }
        islands.push(lints::Island {
            manifest: relative,
            contents: fs::read_to_string(manifest)?,
            members,
        });
    }
    if !stale.is_empty() {
        return Err(format!(
            "locked Cargo metadata failed in {} workspace(s); `cargo xtask lock` updates every workspace lock:\n{}",
            stale.len(),
            stale.join("\n")
        )
        .into());
    }
    println!(
        "locked Cargo metadata passed for {} workspace(s)",
        workspaces.len()
    );
    let mut locks = Vec::with_capacity(workspaces.len());
    for manifest in &workspaces {
        let lock = manifest.with_file_name("Cargo.lock");
        let contents =
            fs::read_to_string(&lock).map_err(|error| format!("{}: {error}", lock.display()))?;
        locks.push((lock.strip_prefix(&context.root)?.to_path_buf(), contents));
    }
    pins::check(
        &fs::read_to_string(context.root.join("Cargo.toml"))?,
        &locks,
    )?;
    println!(
        "Git pins and root patches agree across {} lock catalog(s)",
        locks.len()
    );
    let generated: Vec<String> = oer_repo::chips::Chips::at(&context.root)?
        .profiles()
        .iter()
        .flat_map(|profile| profile.packages.generated.iter().cloned())
        .collect();
    lints::check(std::path::Path::new("Cargo.toml"), &islands, &generated)?;
    println!(
        "the root lint policy applies in {} workspace(s)",
        islands.len()
    );
    Ok(workspaces.len())
}

#[cfg(test)]
mod workflow_tests {
    /// The repository keeps Cargo offline, so a CI runner without a Cargo
    /// cache must turn the network back on in every workflow.
    #[test]
    fn every_workflow_lets_cargo_download() {
        let workflows =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
        for entry in std::fs::read_dir(&workflows).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.contains("CARGO_NET_OFFLINE: 'false'"),
                "{} runs Cargo offline",
                path.display()
            );
        }
    }
}
