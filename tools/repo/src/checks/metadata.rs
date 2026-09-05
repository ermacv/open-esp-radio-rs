use crate::{Context, Result, cargo, paths};
use std::collections::BTreeSet;

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
    for manifest in &workspaces {
        println!(
            "checking locked Cargo metadata: {}",
            manifest.strip_prefix(&context.root)?.display()
        );
        cargo::metadata(context, manifest, &[], None, true)?;
    }
    println!(
        "locked Cargo metadata passed for {} workspace(s)",
        workspaces.len()
    );
    Ok(workspaces.len())
}
