use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, Package, PackageId};

use crate::{Context, Result, cargo, graph::Graph, paths};
use sha2::{Digest as _, Sha256};
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
    pub workspace_manifest: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CargoBuildProfile {
    Dev,
    Release,
}

impl CargoBuildProfile {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Release => "release",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CargoTargetSelection {
    DefaultTargets,
    Lib(String),
    Bin(String),
}

impl CargoTargetSelection {
    pub fn label(&self) -> String {
        match self {
            Self::DefaultTargets => "default-targets".into(),
            Self::Lib(name) => format!("lib:{name}"),
            Self::Bin(name) => format!("bin:{name}"),
        }
    }
}

/// One feature-isolated Cargo invocation. Checks add their own purpose while
/// sharing this selection identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CargoConfiguration {
    pub manifest: PathBuf,
    pub workspace_manifest: PathBuf,
    pub package: String,
    pub target: String,
    pub features: Vec<String>,
    pub build_profile: CargoBuildProfile,
    pub cargo_target: CargoTargetSelection,
}

impl CargoConfiguration {
    pub fn apply(&self, command: &mut Command) {
        command
            .args(["--locked", "--offline", "--manifest-path"])
            .arg(&self.manifest)
            .args(["--package", &self.package, "--target", &self.target]);
        if self.build_profile == CargoBuildProfile::Release {
            command.arg("--release");
        }
        match &self.cargo_target {
            CargoTargetSelection::DefaultTargets => {}
            CargoTargetSelection::Lib(_) => {
                command.arg("--lib");
            }
            CargoTargetSelection::Bin(name) => {
                command.args(["--bin", name]);
            }
        }
        command.args(&self.features);
    }

    pub fn id(&self, root: &Path) -> Result<String> {
        let manifest = self.manifest.strip_prefix(root)?;
        let workspace = self.workspace_manifest.strip_prefix(root)?;
        let identity = format!(
            "workspace={}\nmanifest={}\npackage={}\ntarget={}\nfeatures={:?}\nbuild-profile={}\ncargo-target={}",
            workspace.display(),
            manifest.display(),
            self.package,
            self.target,
            self.features,
            self.build_profile.label(),
            self.cargo_target.label(),
        );
        let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
        let stem = self
            .package
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        Ok(format!("{stem}-{}", &digest[..16]))
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
                        workspace_manifest: workspace_manifest.clone(),
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
        "contract" | "protocol" | "hardware" | "adapter" | "runtime" | "service"
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

/// Responsibilities are not a single stack: an adapter may implement a runtime
/// interface, while a runtime may use an adapter for a lower executor contract.
/// Neither may acquire the final composition or public facade above them.
fn layer_allows(source: &str, target: &str) -> bool {
    match source {
        "contract" | "protocol" => matches!(target, "contract" | "protocol"),
        "hardware" => matches!(target, "contract" | "protocol" | "hardware"),
        "adapter" => matches!(
            target,
            "contract" | "protocol" | "hardware" | "adapter" | "runtime"
        ),
        "runtime" => matches!(
            target,
            "contract" | "protocol" | "hardware" | "adapter" | "runtime" | "service"
        ),
        "service" => matches!(target, "contract" | "protocol" | "adapter" | "service"),
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
    ctx: &Context,
    packages: &[ProductionPackage],
    target: &str,
) -> Result<Vec<CargoConfiguration>> {
    let root_workspace = ctx.root.join("Cargo.toml").canonicalize()?;
    let mut configurations = Vec::new();
    for item in packages {
        let workspace_manifest = if item.workspace_member {
            root_workspace.clone()
        } else {
            cargo::workspace_manifest(ctx, &item.manifest)?
        };
        for features in compilation_profiles(&item.package)? {
            configurations.push(CargoConfiguration {
                manifest: item.manifest.clone(),
                workspace_manifest: workspace_manifest.clone(),
                package: item.package.name.to_string(),
                target: target.into(),
                features,
                build_profile: CargoBuildProfile::Dev,
                cargo_target: CargoTargetSelection::DefaultTargets,
            });
        }
    }
    Ok(configurations)
}

fn application_profiles(package: &Package) -> Result<Vec<Vec<String>>> {
    let mut profiles = if uses_default_configuration(package)? {
        vec![Vec::new()]
    } else {
        Vec::new()
    };
    profiles.extend(
        declared_profiles(package)?
            .into_iter()
            .map(|profile| vec!["--no-default-features".into(), "--features".into(), profile]),
    );
    if profiles.is_empty() {
        return Err(format!(
            "package {} disables its default configuration without a supported feature profile",
            package.name
        )
        .into());
    }
    Ok(profiles)
}

pub fn documentation_profiles(package: &Package) -> Result<Vec<Vec<String>>> {
    let class = classification(package)?;
    match (class.scope, class.layer) {
        ("production", _) => compilation_profiles(package),
        (_, "application") => application_profiles(package),
        (_, "experiment") => {
            let mut profiles = if uses_default_configuration(package)? {
                vec![Vec::new()]
            } else {
                Vec::new()
            };
            profiles.extend(maximal_profiles(package)?);
            Ok(profiles)
        }
        _ => {
            let mut profiles = if uses_default_configuration(package)? {
                vec![Vec::new()]
            } else {
                Vec::new()
            };
            profiles.extend(
                declared_profiles(package)?.into_iter().map(|profile| {
                    vec!["--no-default-features".into(), "--features".into(), profile]
                }),
            );
            if profiles.is_empty() {
                return Err(format!(
                    "package {} disables its default configuration without a supported feature profile",
                    package.name
                )
                .into());
            }
            Ok(profiles)
        }
    }
}

pub fn uses_default_configuration(package: &Package) -> Result<bool> {
    match package
        .metadata
        .get("open-radio")
        .and_then(|metadata| metadata.get("default-configuration"))
    {
        None => Ok(true),
        Some(value) => value.as_bool().ok_or_else(|| {
            format!(
                "package {} open-radio.default-configuration must be a boolean",
                package.name
            )
            .into()
        }),
    }
}

pub fn example_configurations(ctx: &Context, target: &str) -> Result<Vec<CargoConfiguration>> {
    let mut configurations = Vec::new();
    for item in source_packages(ctx)? {
        let class = classification(&item.package)?;
        let relative = item.manifest.strip_prefix(&ctx.root)?;
        if class.layer != "application" || !relative.starts_with("examples") {
            continue;
        }
        if !matches!(class.platform, Platform::Chip("esp32s31")) {
            return Err(format!(
                "example package {} has unsupported platform {:?}",
                item.package.name, class.platform
            )
            .into());
        }
        if !item
            .package
            .targets
            .iter()
            .any(|target| target.kind.contains(&cargo_metadata::TargetKind::Bin))
        {
            return Err(
                format!("example package {} has no binary target", item.package.name).into(),
            );
        }
        for features in application_profiles(&item.package)? {
            configurations.push(CargoConfiguration {
                manifest: item.manifest.clone(),
                workspace_manifest: item.workspace_manifest.clone(),
                package: item.package.name.to_string(),
                target: target.into(),
                features,
                build_profile: CargoBuildProfile::Release,
                cargo_target: CargoTargetSelection::DefaultTargets,
            });
        }
    }
    if configurations.is_empty() {
        return Err("no ESP32-S31 example configurations found".into());
    }
    Ok(configurations)
}

pub fn example_host_test_configurations(
    ctx: &Context,
    host: &str,
) -> Result<Vec<CargoConfiguration>> {
    let mut configurations = Vec::new();
    for item in source_packages(ctx)? {
        let class = classification(&item.package)?;
        let relative = item.manifest.strip_prefix(&ctx.root)?;
        if class.layer != "application" || !relative.starts_with("examples") {
            continue;
        }
        let host_tests = item
            .package
            .metadata
            .get("open-radio")
            .and_then(|metadata| metadata.get("host-tests"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !host_tests {
            continue;
        }
        let library = item
            .package
            .targets
            .iter()
            .find(|target| target.kind.contains(&cargo_metadata::TargetKind::Lib))
            .ok_or_else(|| {
                format!(
                    "package {} enables open-radio.host-tests without a library target",
                    item.package.name
                )
            })?;
        configurations.push(CargoConfiguration {
            manifest: item.manifest,
            workspace_manifest: item.workspace_manifest,
            package: item.package.name.to_string(),
            target: host.into(),
            features: Vec::new(),
            build_profile: CargoBuildProfile::Dev,
            cargo_target: CargoTargetSelection::Lib(library.name.clone()),
        });
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
