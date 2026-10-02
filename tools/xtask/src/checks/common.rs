use std::{
    collections::{BTreeMap, BTreeSet},
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
            // Resolve another workspace's package only when its manifest
            // classifies it as production: one Cargo call per package.
            let section = &document["package"];
            let table = section.get("metadata").and_then(|m| m.get("open-radio"));
            let class = oer_tidy::classification::classify(
                section.get("name").and_then(toml::Value::as_str).unwrap_or(""),
                &|key| table.and_then(|t| t.get(key)).and_then(toml::Value::as_str).map(str::to_owned),
            )?;
            if class.scope != Scope::Production {
                continue;
            }
            package_for_manifest(&cargo::metadata_no_deps(ctx, &manifest)?, &manifest)?.clone()
        };
        let class = classification(&package)?;
        if class.scope != Scope::Production {
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
    // Every workspace as `oer-tidy` discovers it, and the root one, whose
    // membership also retains ignored source members.
    let repo = oer_tidy::repo::Repo::from_git(&ctx.root)?;
    let mut workspace_manifests = BTreeSet::from([ctx.root.join("Cargo.toml")]);
    for manifest in oer_tidy::workspaces::discover(&oer_tidy::manifest::Manifests::load(&repo)?) {
        workspace_manifests.insert(ctx.root.join(manifest));
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

pub use oer_tidy::classification::{Classification, Evidence, Platform, Scope};

/// The family of every chip, keyed by chip id (`chip.toml`).
pub type Families = BTreeMap<String, String>;

/// The one package through which selected packages reach a chip's PAC.
const SELECTED_PAC: &str = "oer-pac";

/// The classification `oer-tidy` reads from the package's
/// `[package.metadata.open-radio]` table.
pub fn classification(package: &Package) -> Result<Classification> {
    let metadata = package.metadata.get("open-radio");
    Ok(oer_tidy::classification::classify(&package.name, &|key| {
        metadata
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    })?)
}

/// Executor crates. Lower layers expose futures that any executor may poll.
const EXECUTORS: &[&str] = &["embassy-executor"];

fn binds_executor(layer: &str) -> bool {
    matches!(layer, "adapter" | "composition" | "facade")
}

/// Crates that read or wait on the image's one time driver: the driver
/// interface and its `oer-time` binding. Lower layers, runtimes included,
/// take time through the `oer-time` clock and timer ports; their tests use
/// virtual time, so the rule covers dev dependencies too.
const TIME_DRIVERS: &[&str] = &["embassy-time", "oer-time-embassy"];

fn binds_time_driver(layer: &str) -> bool {
    matches!(layer, "adapter" | "composition" | "facade")
}

/// Apply declared production edges, including optional and build dependencies.
/// Dev dependencies may compose experiments with the real production owners.
/// `families` maps each chip to its family for chip-to-family edges.
pub fn validate_production_edges(
    packages: &[ProductionPackage],
    families: &Families,
) -> Result<()> {
    for source in packages {
        let source_class = classification(&source.package)?;
        if !binds_time_driver(&source_class.layer)
            && let Some(dependency) = source
                .package
                .dependencies
                .iter()
                .find(|dependency| TIME_DRIVERS.contains(&dependency.name.as_str()))
        {
            return Err(format!(
                "{} package {} depends on time driver {}; only adapters, compositions and the facade bind it, and lower layers take time through oer-time",
                source_class.layer, source.package.name, dependency.name
            )
            .into());
        }
        for dependency in production_dependencies(&source.package) {
            if EXECUTORS.contains(&dependency.name.as_str()) && !binds_executor(&source_class.layer)
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
            if !layer_allows(&source_class.layer, &target_class.layer) {
                return Err(format!(
                    "forbidden architecture edge {} -> {}: {} depends on {}",
                    source_class.layer, target_class.layer, source.package.name, dependency.name
                )
                .into());
            }
            let platform_allowed = if dependency.kind == DependencyKind::Build {
                // A build script runs on the host.
                matches!(target_class.platform, Platform::Host | Platform::Portable)
            } else {
                match (&source_class.platform, &target_class.platform) {
                    (_, Platform::Portable) => true,
                    (Platform::Chip(source), Platform::Chip(target)) => source == target,
                    (Platform::Host, Platform::Host) => true,
                    // Code written once for every chip reaches a chip's
                    // PAC only through `oer-pac`.
                    (Platform::Selected, Platform::Selected) => true,
                    (Platform::Selected, Platform::Chip(_)) => {
                        source.package.name.as_str() == SELECTED_PAC
                    }
                    // A chip package may use shared code, selecting its own
                    // chip.
                    (Platform::Chip(_), Platform::Selected) => true,
                    // Family code is shared within its family only.
                    (Platform::Family(source), Platform::Family(target)) => source == target,
                    (Platform::Chip(chip), Platform::Family(family)) => {
                        families.get(chip).is_some_and(|own| own == family)
                    }
                    _ => source_class.layer == "facade",
                }
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
    feature_sets(
        package,
        "supported-feature-profiles",
        "supported feature profile",
    )
}

/// Feature sets a package's tests also run with besides its default
/// features, as `open-radio.test-feature-sets` declares them: `check
/// changed` tests a changed package with each, and CI tests every package
/// with each (`cargo xtask check feature-sets`).
pub fn test_feature_sets(package: &Package) -> Result<Vec<String>> {
    feature_sets(package, "test-feature-sets", "test feature set")
}

/// Comma-separated feature sets under `open-radio.<key>`: unique, nonempty,
/// naming only features the package declares.
fn feature_sets(package: &Package, key: &str, noun: &str) -> Result<Vec<String>> {
    let Some(profiles) = package.metadata.get("open-radio").and_then(|v| v.get(key)) else {
        return Ok(Vec::new());
    };
    let profiles = profiles
        .as_array()
        .ok_or_else(|| format!("open-radio.{key} must be an array"))?;
    let profiles = profiles
        .iter()
        .map(|profile| {
            profile
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("each {noun} must be a nonempty string").into())
        })
        .collect::<Result<Vec<_>>>()?;
    let mut unique_profiles = BTreeSet::new();
    for profile in &profiles {
        if !unique_profiles.insert(profile) {
            return Err(format!("package {} repeats {noun} {profile}", package.name).into());
        }
        let mut unique_features = BTreeSet::new();
        for feature in profile.split(',') {
            if feature.is_empty()
                || !unique_features.insert(feature)
                || !package.features.contains_key(feature)
            {
                return Err(
                    format!("package {} has invalid {noun} {profile}", package.name).into(),
                );
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

/// Every package's compilation profiles, each for its own chip's Rust target:
/// a chip package builds for the target its chip profile names, and a
/// portable or host package for `target`.
pub fn architecture_configurations(
    root: &Path,
    packages: &[ProductionPackage],
    target: &str,
) -> Result<Vec<CargoConfiguration>> {
    let mut configurations = Vec::new();
    for item in packages {
        let target = match classification(&item.package)?.platform {
            Platform::Chip(chip) => oer_chip_profile::Profile::load(root, &chip)?.rust_target,
            // Built for the target of every chip of its family.
            Platform::Family(family) => {
                let targets = oer_chip_profile::Profile::all(root)?
                    .into_iter()
                    .filter(|chip| chip.family == *family)
                    .map(|chip| chip.rust_target)
                    .collect::<BTreeSet<_>>();
                if targets.is_empty() {
                    return Err(format!(
                        "family package {} names family `{family}`, which no chip declares",
                        item.package.name
                    )
                    .into());
                }
                for target in targets {
                    for features in compilation_profiles(&item.package)? {
                        configurations.push(CargoConfiguration {
                            manifest: item.manifest.clone(),
                            package: item.package.name.to_string(),
                            target: target.clone(),
                            features,
                        });
                    }
                }
                continue;
            }
            Platform::Portable => target.to_owned(),
            // Host packages run on the build machine: the workspace's own
            // host build covers them.
            Platform::Host => continue,
            // Written once for every chip: built once for each, with that
            // chip's feature and target.
            Platform::Selected => {
                for chip in oer_chip_profile::Profile::all(root)? {
                    configurations.push(CargoConfiguration {
                        manifest: item.manifest.clone(),
                        package: item.package.name.to_string(),
                        target: chip.rust_target,
                        features: vec![
                            String::from("--no-default-features"),
                            String::from("--features"),
                            chip.id,
                        ],
                    });
                }
                continue;
            }
        };
        for features in compilation_profiles(&item.package)? {
            configurations.push(CargoConfiguration {
                manifest: item.manifest.clone(),
                package: item.package.name.to_string(),
                target: target.clone(),
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
