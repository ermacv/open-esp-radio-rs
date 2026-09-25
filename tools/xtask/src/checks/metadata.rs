use crate::{Context, Result, cargo, paths};
use std::{collections::BTreeSet, fs};

mod lints;
mod pins;

pub fn run(context: &Context) -> Result<usize> {
    let manifests = paths::source_manifests(context)?;
    if manifests.is_empty() {
        return Err("no source Cargo manifests found".into());
    }
    let mut workspaces = BTreeSet::new();
    for manifest in manifests {
        let workspace = cargo::workspace_manifest(context, &manifest)?;
        if !workspace.starts_with(&context.root) {
            return Err(format!(
                "Cargo workspace escaped repository: {}",
                workspace.display()
            )
            .into());
        }
        workspaces.insert(workspace);
    }
    let mut islands = Vec::with_capacity(workspaces.len());
    for manifest in &workspaces {
        let relative = manifest.strip_prefix(&context.root)?.to_path_buf();
        println!("checking locked Cargo metadata: {}", relative.display());
        let graph = cargo::metadata(context, manifest, &[], None, true)?;
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
