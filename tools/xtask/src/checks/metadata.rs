use crate::{Context, Result, cargo};
use std::{collections::BTreeSet, fs};

mod lints;
mod pins;

/// Every Cargo workspace of the repository, by its root manifest, as
/// `oer-tidy` discovers them from the manifests as text.
pub fn workspaces(context: &Context) -> Result<BTreeSet<std::path::PathBuf>> {
    let repo = oer_tidy::repo::Repo::from_git(&context.root)?;
    let workspaces: BTreeSet<_> =
        oer_tidy::workspaces::discover(&oer_tidy::manifest::Manifests::load(&repo)?)
            .into_iter()
            .map(|manifest| context.root.join(manifest))
            .collect();
    if workspaces.is_empty() {
        return Err("no Cargo workspace found".into());
    }
    Ok(workspaces)
}

/// Brings every workspace lock in line with its manifests (`cargo xtask
/// lock`), so a dependency or pin change updates all locks in one step.
pub fn update_locks(context: &Context) -> Result<()> {
    for manifest in workspaces(context)? {
        let lock = manifest.with_file_name("Cargo.lock");
        let before = fs::read_to_string(&lock).ok();
        crate::process::capture(
            context
                .cargo()
                .args(["metadata", "--format-version", "1"])
                .args(ONLINE)
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

/// Lets one Cargo command use the network although the repository's
/// configuration keeps Cargo offline.
pub const ONLINE: [&str; 2] = ["--config", "net.offline=false"];

/// Downloads whatever each workspace's lock file names and the local cache
/// lacks. A workspace whose dependencies are all present costs one offline
/// `cargo fetch` of a fraction of a second; only a missing one goes online.
pub fn fetch(context: &Context) -> Result<()> {
    for manifest in workspaces(context)? {
        let offline = context
            .cargo()
            .args(["fetch", "--locked", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if offline.success() {
            continue;
        }
        println!(
            "fetching the dependencies of {}",
            manifest.strip_prefix(&context.root)?.display()
        );
        crate::process::run(
            context
                .cargo()
                .args(["fetch", "--locked"])
                .args(ONLINE)
                .arg("--manifest-path")
                .arg(&manifest),
        )?;
    }
    Ok(())
}

pub fn run(context: &Context) -> Result<usize> {
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
    lints::check(std::path::Path::new("Cargo.toml"), &islands)?;
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
