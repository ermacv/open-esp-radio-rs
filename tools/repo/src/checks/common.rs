use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, Package, PackageId};

use crate::{Context, Result, cargo, graph::Graph, paths};

pub struct ProductionPackage {
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

pub fn production_packages(ctx: &Context) -> Result<Vec<ProductionPackage>> {
    let workspace = cargo::metadata_no_deps(ctx, &ctx.root.join("Cargo.toml"))?;
    let mut manifests = BTreeSet::new();
    for manifest in paths::source_manifests(ctx)? {
        manifests.insert(manifest.canonicalize()?);
    }
    // Cargo membership is authoritative even for an ignored working-tree file.
    // An ignored production crate must not disappear from compiled audits.
    for package in &workspace.packages {
        if workspace.workspace_members.contains(&package.id) {
            let manifest = package.manifest_path.as_std_path().canonicalize()?;
            manifests.insert(manifest);
        }
    }
    let mut packages = Vec::new();
    for manifest in manifests {
        let document: toml::Value = toml::from_str(&std::fs::read_to_string(&manifest)?)?;
        if document.get("package").is_none() {
            continue;
        }
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
        let class = classification(&package)?;
        if class.scope != "production" {
            continue;
        }
        packages.push(ProductionPackage {
            package,
            manifest,
            workspace_member: member,
        });
    }
    if packages.is_empty() {
        return Err("no production packages found".into());
    }
    Ok(packages)
}

pub struct Classification<'a> {
    pub scope: &'a str,
    pub layer: &'a str,
    pub platform: &'a str,
}

/// Classification is required for every source package, regardless of its path.
pub fn classification(package: &Package) -> Result<Classification<'_>> {
    let metadata = package.metadata.get("open-radio");
    let field = |name| {
        metadata
            .and_then(|value| value.get(name))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("package {} lacks open-radio.{name}", package.name))
    };
    let class = Classification {
        scope: field("scope")?,
        layer: field("layer")?,
        platform: field("platform")?,
    };
    let expected_scope = match class.layer {
        "contract" | "protocol" | "hardware" | "adapter" | "runtime" | "service"
        | "composition" | "facade" => "production",
        "experiment" => "experimental",
        "tool" | "hil" | "qualification" | "verification" | "application" | "platform" => {
            "development"
        }
        _ => return Err(format!("package {} has unknown architecture layer", package.name).into()),
    };
    if class.scope != expected_scope || !matches!(class.platform, "portable" | "esp32s31" | "host")
    {
        return Err(format!(
            "package {} has inconsistent architecture classification",
            package.name
        )
        .into());
    }
    Ok(class)
}

/// Apply declared production edges, including optional and build dependencies.
/// Dev dependencies may compose experiments with the real production owners.
pub fn validate_production_edges(packages: &[ProductionPackage]) -> Result<()> {
    for source in packages {
        let source_class = classification(&source.package)?;
        for dependency in production_dependencies(&source.package) {
            let Some(path) = &dependency.path else {
                continue;
            };
            let manifest = path.join("Cargo.toml").as_std_path().canonicalize()?;
            let target = packages
                .iter()
                .find(|item| item.manifest == manifest)
                .ok_or_else(|| {
                    format!(
                        "production package {} depends on non-production package {}",
                        source.package.name, dependency.name
                    )
                })?;
            let target_class = classification(&target.package)?;
            if target_class.layer == "facade" {
                return Err(format!(
                    "internal package {} depends on public facade {}",
                    source.package.name, dependency.name
                )
                .into());
            }
            if matches!(source_class.layer, "contract" | "protocol")
                && !matches!(target_class.layer, "contract" | "protocol")
            {
                return Err(format!(
                    "portable policy {} depends on upper layer {}",
                    source.package.name, dependency.name
                )
                .into());
            }
            if source_class.platform == "portable"
                && target_class.platform == "esp32s31"
                && source_class.layer != "facade"
            {
                return Err(format!(
                    "portable package {} depends on chip-specific {}",
                    source.package.name, dependency.name
                )
                .into());
            }
        }
    }
    Ok(())
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
