use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, Package, PackageId};

use crate::{Context, Result, cargo, graph::Graph, paths};

pub struct DriverPackage {
    pub package: Package,
    pub manifest: PathBuf,
    pub workspace_member: bool,
}

pub fn package_for_manifest<'a>(metadata: &'a Metadata, manifest: &Path) -> Result<&'a Package> {
    let canonical = manifest.canonicalize()?;
    let mut candidates = metadata.packages.iter().filter(|package| {
        package
            .manifest_path
            .as_std_path()
            .canonicalize()
            .is_ok_and(|path| path == canonical)
    });
    let package = candidates
        .next()
        .ok_or_else(|| format!("manifest has no Cargo package: {}", manifest.display()))?;
    if candidates.next().is_some() {
        return Err(format!(
            "manifest identifies multiple Cargo packages: {}",
            manifest.display()
        )
        .into());
    }
    Ok(package)
}

pub fn driver_packages(ctx: &Context) -> Result<Vec<DriverPackage>> {
    let workspace = cargo::metadata_no_deps(ctx, &ctx.root.join("Cargo.toml"))?;
    let driver = ctx.root.join("driver").canonicalize()?;
    let mut manifests = BTreeSet::new();
    for manifest in paths::source_manifests(ctx)? {
        if manifest.starts_with(&driver) {
            manifests.insert(manifest.canonicalize()?);
        }
    }
    // Cargo membership is authoritative even for an ignored working-tree file.
    // An ignored production crate must not disappear from compiled audits.
    for package in &workspace.packages {
        if workspace.workspace_members.contains(&package.id) {
            let manifest = package.manifest_path.as_std_path().canonicalize()?;
            if manifest.starts_with(&driver) {
                manifests.insert(manifest);
            }
        }
    }
    let mut packages = Vec::new();
    for manifest in manifests {
        let member = workspace.packages.iter().any(|package| {
            workspace.workspace_members.contains(&package.id)
                && package
                    .manifest_path
                    .as_std_path()
                    .canonicalize()
                    .is_ok_and(|path| path == manifest)
        });
        let package = if member {
            package_for_manifest(&workspace, &manifest)?.clone()
        } else {
            package_for_manifest(&cargo::metadata_no_deps(ctx, &manifest)?, &manifest)?.clone()
        };
        packages.push(DriverPackage {
            package,
            manifest,
            workspace_member: member,
        });
    }
    if packages.is_empty() {
        return Err("no production driver packages found".into());
    }
    Ok(packages)
}

pub fn declared_profiles(package: &Package) -> Result<Vec<String>> {
    let Some(profiles) = package
        .metadata
        .get("open-radio")
        .and_then(|v| v.get("supported-feature-profiles"))
    else {
        return Ok(Vec::new());
    };
    let profiles = profiles
        .as_array()
        .ok_or("supported-feature-profiles must be an array")?;
    profiles
        .iter()
        .map(|profile| {
            profile
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "feature profile must be a nonempty string".into())
        })
        .collect()
}

pub fn maximal_profiles(package: &Package) -> Result<Vec<Vec<String>>> {
    let profiles = declared_profiles(package)?;
    Ok(if profiles.is_empty() {
        vec![vec!["--all-features".into()]]
    } else {
        profiles
            .into_iter()
            .map(|p| vec!["--no-default-features".into(), "--features".into(), p])
            .collect()
    })
}

pub fn production_dependencies(
    package: &Package,
) -> impl Iterator<Item = &cargo_metadata::Dependency> {
    package
        .dependencies
        .iter()
        .filter(|dependency| dependency.kind != DependencyKind::Development)
}

pub fn id_for_name(graph: &Graph, name: &str) -> Result<PackageId> {
    let mut packages = graph
        .metadata
        .packages
        .iter()
        .filter(|p| p.name.as_str() == name);
    let id = packages
        .next()
        .ok_or_else(|| format!("missing package {name}"))?
        .id
        .clone();
    if packages.next().is_some() {
        return Err(format!("ambiguous package name {name}").into());
    }
    Ok(id)
}

pub fn package_feature(graph: &Graph, name: &str, feature: &str) -> Result<bool> {
    let id = id_for_name(graph, name)?;
    let resolve = graph
        .metadata
        .resolve
        .as_ref()
        .ok_or("missing Cargo resolve graph")?;
    let node = resolve
        .nodes
        .iter()
        .find(|node| node.id == id)
        .ok_or("missing Cargo resolve node")?;
    Ok(node.features.iter().any(|f| f.as_str() == feature))
}

pub fn forbid_features(graph: &Graph, forbidden: &[&str]) -> Result<()> {
    for node in &graph
        .metadata
        .resolve
        .as_ref()
        .ok_or("missing Cargo resolve graph")?
        .nodes
    {
        for feature in &node.features {
            if forbidden.contains(&feature.as_str()) {
                return Err(format!("forbidden feature {feature} enabled in {}", node.id).into());
            }
        }
    }
    Ok(())
}

pub fn closure<'a>(graph: &'a Graph, root: &PackageId) -> Result<Vec<&'a Package>> {
    let resolve = graph
        .metadata
        .resolve
        .as_ref()
        .ok_or("missing Cargo resolve graph")?;
    if !resolve.nodes.iter().any(|node| &node.id == root) {
        return Err("closure root has no Cargo resolve node".into());
    }
    Ok(graph
        .reachable(root)
        .keys()
        .map(|id| graph.package(id))
        .collect())
}

#[cfg(test)]
mod tests;
