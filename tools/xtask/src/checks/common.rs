use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{Metadata, Package, PackageId};

use crate::{Result, graph::Graph};
use oer_process::Checkout;
use std::process::Command;

pub use oer_repo::{Classification, Layer, Platform, Scope};

/// One classified package of the repository model.
#[derive(Clone, Debug)]
pub struct Classified {
    pub package: oer_repo::Package,
    pub class: Classification,
    /// Its absolute manifest path.
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
            .args(["--locked", "--manifest-path"])
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

/// The repository model of the checkout (`oer-repo`).
pub fn model(ctx: &Checkout) -> Result<oer_repo::Model> {
    Ok(oer_repo::Model::load(&oer_repo::Repo::from_git(
        &ctx.root,
    )?)?)
}

/// Every package of every workspace with its classification; a package the
/// model cannot classify fails, so none disappears from an audit.
pub fn source_packages(root: &Path, model: &oer_repo::Model) -> Result<Vec<Classified>> {
    let mut packages = Vec::new();
    for package in model.packages() {
        let class = model
            .classification(package)
            .map_err(|error| format!("{}: {error}", package.manifest))?;
        packages.push(Classified {
            package: package.clone(),
            class: class.clone(),
            manifest: root.join(&package.manifest),
        });
    }
    if packages.is_empty() {
        return Err("no classified source packages found".into());
    }
    Ok(packages)
}

/// The production packages of every workspace.
pub fn production_packages(root: &Path, model: &oer_repo::Model) -> Result<Vec<Classified>> {
    let packages: Vec<Classified> = source_packages(root, model)?
        .into_iter()
        .filter(|package| package.class.scope == Scope::Production)
        .collect();
    if packages.is_empty() {
        return Err("no production packages found".into());
    }
    Ok(packages)
}

/// Default builds are always checked. The facade must also work with no
/// features; lower compositions may require one of their declared alternatives.
pub fn compilation_profiles(class: &Classification) -> Vec<Vec<String>> {
    let mut profiles = vec![vec![]];
    if class.supported_feature_profiles.is_empty() || class.layer == Layer::Facade {
        profiles.insert(0, vec!["--no-default-features".into()]);
    }
    profiles.extend(maximal_profiles(class));
    profiles
}

pub fn maximal_profiles(class: &Classification) -> Vec<Vec<String>> {
    if class.supported_feature_profiles.is_empty() {
        vec![vec!["--all-features".into()]]
    } else {
        class
            .supported_feature_profiles
            .iter()
            .map(|p| {
                vec![
                    "--no-default-features".into(),
                    "--features".into(),
                    p.clone(),
                ]
            })
            .collect()
    }
}

/// Every package's compilation profiles, each for its own chip's Rust target:
/// a chip package builds for the target its chip profile names, and a
/// portable or host package for `target`.
pub fn architecture_configurations(
    root: &Path,
    packages: &[Classified],
    target: &str,
) -> Result<Vec<CargoConfiguration>> {
    let mut configurations = Vec::new();
    for item in packages {
        let target = match &item.class.platform {
            Platform::Chip(chip) => oer_chip_profile::Profile::load(root, chip)?.rust_target,
            // Built for the target of every chip of its family.
            Platform::Family(family) => {
                let targets = oer_chip_profile::Profile::all(root)?
                    .into_iter()
                    .filter(|chip| chip.family == **family)
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
                    for features in compilation_profiles(&item.class) {
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
        };
        for features in compilation_profiles(&item.class) {
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
