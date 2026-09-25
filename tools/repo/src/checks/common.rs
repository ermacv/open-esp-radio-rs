use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, Package, PackageId};

use crate::{Context, Result, cargo, graph::Graph, paths};
use std::process::Command;

pub struct ProductionPackage {
    pub package: Package,
    pub manifest: PathBuf,
    pub workspace_member: bool,
}

#[derive(Clone)]
pub struct SourcePackage {
    pub package: Package,
    pub manifest: PathBuf,
}

/// One feature-isolated `cargo` invocation on a package.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CargoConfiguration {
    pub manifest: PathBuf,
    pub package: String,
    pub target: String,
    pub features: Vec<String>,
}

impl CargoConfiguration {
    pub fn apply(&self, command: &mut Command) {
        command
            .args(["--locked", "--offline", "--manifest-path"])
            .arg(&self.manifest)
            .args(["--package", &self.package, "--target", &self.target])
            .args(&self.features);
    }
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

/// Packages whose names Blobray code and provider contracts consume; they are
/// renamed together with Blobray rather than by the repository naming rule.
const BLOBRAY_OWNED_NAMES: &[&str] = &[
    "open-esp-radio-register-model",
    "open-radio-vendor-review",
    "open-radio-vendor-chip-contracts-esp32s31-rev0",
    "open-radio-vendor-chip-knowledge-esp32s31-rev0",
    "open-radio-vendor-chip-models-esp32s31-rev0",
    "open-radio-vendor-harness-esp32s31",
    "open-radio-vendor-knowledge-esp32s31",
    "open-radio-vendor-models-esp32s31",
];

/// Every package is `oer-<tokens>`; the public facade alone is `open-esp-radio`.
pub fn validate_package_name(name: &str, layer: &str) -> Result<()> {
    let valid = if layer == "facade" {
        name == "open-esp-radio"
    } else {
        name.strip_prefix("oer-").is_some_and(|tokens| {
            !tokens.is_empty()
                && tokens.split('-').all(|token| {
                    !token.is_empty()
                        && token
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                })
        }) || BLOBRAY_OWNED_NAMES.contains(&name)
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "package {name} does not follow the `oer-<tokens>` naming rule (docs/architecture.md)"
        )
        .into())
    }
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

/// Discover classified source packages across the root and independent Cargo
/// workspaces. Workspace membership also retains ignored source members.
pub fn source_packages(ctx: &Context) -> Result<Vec<SourcePackage>> {
    let source_manifests = paths::source_manifests(ctx)?
        .into_iter()
        .map(|manifest| manifest.canonicalize())
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let mut workspace_manifests = BTreeSet::new();
    for manifest in &source_manifests {
        workspace_manifests.insert(cargo::workspace_manifest(ctx, manifest)?);
    }
    let root_workspace = cargo::metadata_no_deps(ctx, &ctx.root.join("Cargo.toml"))?;
    for package in &root_workspace.packages {
        if root_workspace.workspace_members.contains(&package.id) {
            workspace_manifests.insert(ctx.root.join("Cargo.toml").canonicalize()?);
        }
    }

    let mut packages = std::collections::BTreeMap::new();
    for workspace_manifest in workspace_manifests {
        let metadata = cargo::metadata_no_deps(ctx, &workspace_manifest)?;
        for package in metadata
            .packages
            .iter()
            .filter(|package| metadata.workspace_members.contains(&package.id))
        {
            let manifest = package.manifest_path.as_std_path().canonicalize()?;
            if !manifest.starts_with(&ctx.root) {
                return Err(format!(
                    "Cargo workspace member escaped repository: {}",
                    manifest.display()
                )
                .into());
            }
            classification(package)?;
            if packages
                .insert(
                    manifest.clone(),
                    SourcePackage {
                        package: package.clone(),
                        manifest,
                    },
                )
                .is_some()
            {
                return Err("source package belongs to multiple Cargo workspaces".into());
            }
        }
    }
    if packages.is_empty() {
        return Err("no classified source packages found".into());
    }
    Ok(packages.into_values().collect())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform<'a> {
    Portable,
    Host,
    Chip(&'a str),
}

pub struct Classification<'a> {
    pub scope: &'a str,
    pub layer: &'a str,
    pub platform: Platform<'a>,
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
    let scope = field("scope")?;
    let layer = field("layer")?;
    let chip = metadata.and_then(|value| value.get("chip"));
    let platform = match (field("platform")?, chip) {
        ("portable", None) => Platform::Portable,
        ("host", None) => Platform::Host,
        ("chip", Some(chip)) => {
            let chip = chip
                .as_str()
                .filter(|chip| {
                    chip.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                        && chip
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                })
                .ok_or_else(|| {
                    format!(
                        "package {} has invalid open-radio.chip identifier",
                        package.name
                    )
                })?;
            Platform::Chip(chip)
        }
        _ => {
            return Err(format!(
                "package {} has inconsistent platform/chip classification",
                package.name
            )
            .into());
        }
    };
    let class = Classification {
        scope,
        layer,
        platform,
    };
    let expected_scope = match class.layer {
        "contract" | "protocol" | "hardware" | "role" | "adapter" | "runtime" | "service"
        | "composition" | "facade" => "production",
        "experiment" => "experimental",
        "tool" | "hil" | "qualification" | "verification" | "application" | "platform" => {
            "development"
        }
        _ => return Err(format!("package {} has unknown architecture layer", package.name).into()),
    };
    if class.scope != expected_scope {
        return Err(format!(
            "package {} has inconsistent architecture classification",
            package.name
        )
        .into());
    }
    Ok(class)
}

/// Executor crates. Lower layers expose futures that any executor may poll.
const EXECUTORS: &[&str] = &["embassy-executor"];

fn binds_executor(layer: &str) -> bool {
    matches!(layer, "adapter" | "composition" | "facade")
}

/// Apply declared production edges, including optional and build dependencies.
/// Dev dependencies may compose experiments with the real production owners.
pub fn validate_production_edges(packages: &[ProductionPackage]) -> Result<()> {
    for source in packages {
        let source_class = classification(&source.package)?;
        for dependency in production_dependencies(&source.package) {
            if EXECUTORS.contains(&dependency.name.as_str()) && !binds_executor(source_class.layer)
            {
                return Err(format!(
                    "{} package {} depends on executor {}; only adapters and compositions bind an executor",
                    source_class.layer, source.package.name, dependency.name
                )
                .into());
            }
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
            if !layer_allows(source_class.layer, target_class.layer) {
                return Err(format!(
                    "forbidden architecture edge {} -> {}: {} depends on {}",
                    source_class.layer, target_class.layer, source.package.name, dependency.name
                )
                .into());
            }
            let platform_allowed = match (source_class.platform, target_class.platform) {
                (_, Platform::Portable) => true,
                (Platform::Chip(source), Platform::Chip(target)) => source == target,
                (Platform::Host, Platform::Host) => true,
                _ => source_class.layer == "facade",
            };
            if !platform_allowed {
                return Err(format!(
                    "incompatible platform edge {:?} -> {:?}: {} depends on {}",
                    source_class.platform,
                    target_class.platform,
                    source.package.name,
                    dependency.name
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Responsibilities are not a single stack: an adapter may bind a service or
/// runtime interface to an executor, while a runtime may use an adapter for a
/// lower executor contract. Hardware owns chip resources and wire codecs; a
/// role composes portable role protocols with that hardware. Services declare
/// executor-free ports and never depend on the adapters that bind them. None
/// may acquire the final composition or public facade above them.
fn layer_allows(source: &str, target: &str) -> bool {
    match source {
        "contract" | "protocol" => matches!(target, "contract" | "protocol"),
        "hardware" => matches!(target, "contract" | "protocol" | "hardware"),
        "role" => matches!(target, "contract" | "protocol" | "hardware" | "role"),
        "service" => matches!(target, "contract" | "protocol" | "service"),
        "adapter" | "runtime" => matches!(
            target,
            "contract" | "protocol" | "hardware" | "role" | "adapter" | "runtime" | "service"
        ),
        "composition" | "facade" => target != "facade",
        _ => false,
    }
}

/// Default builds are always checked. The facade must also work with no
/// features; lower compositions may require one of their declared alternatives.
pub fn compilation_profiles(package: &Package) -> Result<Vec<Vec<String>>> {
    let mut profiles = vec![vec![]];
    if declared_profiles(package)?.is_empty() || classification(package)?.layer == "facade" {
        profiles.insert(0, vec!["--no-default-features".into()]);
    }
    profiles.extend(maximal_profiles(package)?);
    Ok(profiles)
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
    let profiles = profiles
        .iter()
        .map(|profile| {
            profile
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "feature profile must be a nonempty string".into())
        })
        .collect::<Result<Vec<_>>>()?;
    let mut unique_profiles = BTreeSet::new();
    for profile in &profiles {
        if !unique_profiles.insert(profile) {
            return Err(format!(
                "package {} repeats supported feature profile {profile}",
                package.name
            )
            .into());
        }
        let mut unique_features = BTreeSet::new();
        for feature in profile.split(',') {
            if feature.is_empty()
                || !unique_features.insert(feature)
                || !package.features.contains_key(feature)
            {
                return Err(format!(
                    "package {} has invalid supported feature profile {profile}",
                    package.name
                )
                .into());
            }
        }
    }
    Ok(profiles)
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

pub fn architecture_configurations(
    packages: &[ProductionPackage],
    target: &str,
) -> Result<Vec<CargoConfiguration>> {
    let mut configurations = Vec::new();
    for item in packages {
        for features in compilation_profiles(&item.package)? {
            configurations.push(CargoConfiguration {
                manifest: item.manifest.clone(),
                package: item.package.name.to_string(),
                target: target.into(),
                features,
            });
        }
    }
    Ok(configurations)
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
