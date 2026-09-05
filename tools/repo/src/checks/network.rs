//! Network policies consume package identities, resolved features and declared edges.

use crate::{Context, Result, cargo, graph::Graph, process};
use cargo_metadata::{DependencyKind, Package};
use std::{collections::BTreeSet, path::Path};

const NETWORK: &str = "open-esp-radio-network";
const OWNED: &str = "open-esp-radio-embassy-net";
const COMPAT: &str = "open-esp-radio-embassy-net-compat";
const BRIDGE: &str = "open-esp-radio-esp32s31-wifi-embassy-compat";
const TARGET: &str = "riscv32imafc-unknown-none-elf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    Neutral,
    Compat,
    Owned,
    Research,
    Datapath,
    RadioCore,
    CompatBridge,
    OwnedProduct,
    CompatProduct,
}
impl Boundary {
    pub fn name(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Compat => "compat",
            Self::Owned => "owned",
            Self::Research => "research",
            Self::Datapath => "datapath",
            Self::RadioCore => "radio-core",
            Self::CompatBridge => "compat-bridge",
            Self::OwnedProduct => "owned-product",
            Self::CompatProduct => "compat-product",
        }
    }
    fn product(self) -> bool {
        matches!(self, Self::OwnedProduct | Self::CompatProduct)
    }
}
fn registry(p: &Package) -> bool {
    p.source
        .as_ref()
        .is_some_and(|source| source.to_string().starts_with("registry+"))
}
fn official_registry(p: &Package) -> bool {
    p.source
        .as_ref()
        .is_some_and(|source| source.is_crates_io())
}
fn network_api(name: &str) -> bool {
    name == "embassy-net" || name.starts_with("embassy-net-")
}
fn xarxa_api(name: &str) -> bool {
    name == "xarxa" || name.starts_with("xarxa-")
}
fn owned_api(p: &Package) -> bool {
    network_api(p.name.as_str()) || xarxa_api(p.name.as_str())
}
fn pinned_git(p: &Package) -> bool {
    let Some(source) = p
        .source
        .as_ref()
        .and_then(|source| source.repr.strip_prefix("git+"))
    else {
        return false;
    };
    let Some((selection, commit)) = source.rsplit_once('#') else {
        return false;
    };
    let Some((_, revision)) = selection.split_once("?rev=") else {
        return false;
    };
    revision == commit && commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn physical(p: &Package, root: &Path) -> bool {
    p.name.starts_with("open-esp-radio-esp32s31")
        || p.manifest_path
            .as_std_path()
            .starts_with(root.join("driver/chips"))
}
fn optimized(p: &Package) -> bool {
    p.name == OWNED
        || xarxa_api(p.name.as_str())
        || (network_api(p.name.as_str()) && !official_registry(p))
}
fn stack(name: &str) -> bool {
    name.starts_with("embassy-") || name.starts_with("open-esp-radio-embassy") || xarxa_api(name)
}

pub fn audit(graph: &Graph, manifest: &Path, boundary: Boundary, repository: &Path) -> Result<()> {
    let root = graph.root(manifest)?;
    for dependency in &graph.package(&root).dependencies {
        if dependency.kind == DependencyKind::Development {
            continue;
        }
        let name = dependency.name.as_str();
        let released = dependency
            .source
            .as_ref()
            .is_some_and(|source| source.repr.starts_with("registry+"));
        let forbidden = match boundary {
            Boundary::Neutral => true,
            Boundary::Compat => {
                (!released && name != NETWORK)
                    || name == OWNED
                    || xarxa_api(name)
                    || (network_api(name)
                        && !dependency
                            .source
                            .as_ref()
                            .is_some_and(|source| source.is_crates_io()))
            }
            Boundary::Owned => {
                name.starts_with("open-esp-radio-esp32s31")
                    || name == "open-esp-radio-dma"
                    || (name == "embassy-net-driver" && released)
                    || dependency.path.as_ref().is_some_and(|path| {
                        ["driver/chips", "driver/memory"]
                            .iter()
                            .any(|owner| path.as_std_path().starts_with(repository.join(owner)))
                    })
            }
            Boundary::Research | Boundary::Datapath => stack(name),
            _ => false,
        };
        if forbidden {
            return Err(format!(
                "{}: forbidden declared production dependency: {name}",
                boundary.name()
            )
            .into());
        }
    }
    let paths = graph.reachable(&root);
    let dependencies: Vec<_> = paths
        .keys()
        .filter(|id| **id != root)
        .map(|id| graph.package(id))
        .collect();
    let reject = |predicate: &dyn Fn(&Package) -> bool, reason: &str| -> Result<()> {
        if let Some(package) = dependencies.iter().find(|p| predicate(p)) {
            let chain = paths[&package.id]
                .iter()
                .map(|id| graph.package(id).name.as_str())
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(format!(
                "{}: {reason}: {chain} ({})",
                boundary.name(),
                package
                    .source
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| package.manifest_path.to_string())
            )
            .into());
        }
        Ok(())
    };
    let required = |predicate: &dyn Fn(&Package) -> bool, description: &str| -> Result<()> {
        if !dependencies.iter().any(|p| predicate(p)) {
            return Err(format!("{}: missing {description}", boundary.name()).into());
        }
        Ok(())
    };
    let released_driver = || -> Result<()> {
        reject(
            &|p| {
                p.name == "embassy-net-driver"
                    && (!official_registry(p) || p.version.to_string() != "0.2.0")
            },
            "requires released embassy-net-driver 0.2.0",
        )?;
        required(
            &|p| {
                p.name == "embassy-net-driver"
                    && official_registry(p)
                    && p.version.to_string() == "0.2.0"
            },
            "released embassy-net-driver 0.2.0",
        )
    };
    if matches!(boundary, Boundary::Owned | Boundary::OwnedProduct) {
        reject(
            &|p| owned_api(p) && !pinned_git(p),
            "owned network APIs require full Git revision pins",
        )?;
    }
    match boundary {
        Boundary::Neutral => reject(
            &|_| true,
            "neutral network values acquired a production dependency",
        )?,
        Boundary::Compat => {
            reject(
                &|p| !registry(p) && p.name != NETWORK,
                "compatibility adapter acquired a non-neutral dependency",
            )?;
            reject(
                &optimized,
                "compatibility adapter requires official crates.io network APIs without owned contracts",
            )?;
            released_driver()?;
        }
        Boundary::Owned => {
            reject(
                &|p| p.name == "embassy-net-driver" && registry(p),
                "owned adapter acquired the released driver contract",
            )?;
            reject(
                &|p| physical(p, repository),
                "owned adapter acquired physical radio ownership",
            )?;
            reject(
                &|p| {
                    paths[&p.id].len() == 2
                        && (p.name == "open-esp-radio-dma"
                            || p.manifest_path
                                .as_std_path()
                                .starts_with(repository.join("driver/memory")))
                },
                "owned adapter acquired a direct physical-memory dependency",
            )?;
        }
        Boundary::Research | Boundary::Datapath => {
            reject(
                &|p| stack(p.name.as_str()),
                "portable contract acquired an executor or network stack",
            )?;
            if boundary == Boundary::Datapath {
                reject(
                    &|p| physical(p, repository),
                    "radio-native datapath acquired a chip dependency",
                )?;
            }
        }
        Boundary::RadioCore | Boundary::CompatBridge | Boundary::CompatProduct => {
            reject(
                &optimized,
                "compatibility/radio core acquired the optimized network graph",
            )?;
            if boundary != Boundary::RadioCore {
                released_driver()?;
            }
            if boundary == Boundary::CompatProduct {
                reject(
                    &|p| network_api(p.name.as_str()) && !official_registry(p),
                    "compatibility product acquired a non-release Embassy network package",
                )?;
                required(&|p| p.name == COMPAT, "compatibility network adapter")?;
                required(&|p| p.name == BRIDGE, "compatibility radio bridge")?;
                required(
                    &|p| p.name == "embassy-net" && official_registry(p),
                    "official crates.io embassy-net stack",
                )?;
            }
        }
        Boundary::OwnedProduct => {
            reject(
                &|p| matches!(p.name.as_str(), COMPAT | BRIDGE),
                "owned product acquired a compatibility network leaf",
            )?;
            required(&|p| p.name == OWNED, "owned network adapter")?;
            required(
                &|p| p.name == "embassy-net-driver" && pinned_git(p),
                "owned Embassy driver contract",
            )?;
            required(
                &|p| p.name == "embassy-net" && pinned_git(p),
                "owned Embassy network stack",
            )?;
            let embassy_sources = dependencies
                .iter()
                .filter(|p| network_api(p.name.as_str()))
                .filter_map(|p| p.source.as_ref().map(|source| source.repr.as_str()))
                .collect::<BTreeSet<_>>();
            if embassy_sources.len() != 1 {
                return Err("owned-product: Embassy network stack and driver must resolve to the same pinned source".into());
            }
        }
    }
    let features = &graph.node(&root).features;
    let has = |feature: &str| features.iter().any(|f| f.as_str() == feature);
    if boundary.product() {
        let (expected, other) = if boundary == Boundary::OwnedProduct {
            ("owned-network", "compat-network")
        } else {
            ("compat-network", "owned-network")
        };
        if !has(expected) || has(other) {
            return Err(format!(
                "{}: selected feature boundary is not exclusive",
                boundary.name()
            )
            .into());
        }
    }
    if boundary == Boundary::RadioCore && has("owned-network") {
        return Err("radio-core: owned-network unexpectedly enabled".into());
    }
    Ok(())
}

pub struct Profile {
    pub boundary: Boundary,
    pub manifest: &'static str,
    pub features: &'static [&'static str],
}
pub fn profiles() -> [Profile; 12] {
    use Boundary::*;
    let product = "driver/integration/esp32s31/embassy/ieee80211/Cargo.toml";
    [
        Profile {
            boundary: Neutral,
            manifest: "driver/network/interface/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Compat,
            manifest: "driver/network/adapters/embassy/compat/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Owned,
            manifest: "driver/network/adapters/embassy/owned/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Research,
            manifest: "driver/network/research/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Research,
            manifest: "driver/network/research/Cargo.toml",
            features: &["--all-features"],
        },
        Profile {
            boundary: Datapath,
            manifest: "driver/ieee80211/datapath/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: RadioCore,
            manifest: "driver/runtime/embassy/esp32s31/ieee80211/Cargo.toml",
            features: &["--no-default-features"],
        },
        Profile {
            boundary: CompatBridge,
            manifest: "driver/adapters/embassy/esp32s31/ieee80211-compat/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: OwnedProduct,
            manifest: product,
            features: &[],
        },
        Profile {
            boundary: CompatProduct,
            manifest: product,
            features: &["--no-default-features", "--features", "compat-network"],
        },
        Profile {
            boundary: OwnedProduct,
            manifest: "examples/esp32s31-station/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: CompatProduct,
            manifest: "examples/esp32s31-station/Cargo.toml",
            features: &["--no-default-features", "--features", "compat-network"],
        },
    ]
}

pub fn run(context: &Context, dependencies_only: bool) -> Result<()> {
    for profile in profiles() {
        let manifest = context.root.join(profile.manifest);
        let flags = profile
            .features
            .iter()
            .map(|v| (*v).to_owned())
            .collect::<Vec<_>>();
        let target = profile.boundary.product().then_some(TARGET);
        let graph = if profile.manifest.starts_with("examples/") {
            // Binary examples cannot become a scratch consumer's dependency.
            // Their independent workspace already isolates feature resolution.
            if cargo::workspace_manifest(context, &manifest)? != manifest.canonicalize()? {
                return Err(
                    format!("example must own its workspace: {}", manifest.display()).into(),
                );
            }
            cargo::metadata(context, &manifest, &flags, target, true)?
        } else {
            cargo::isolated_graph(context, &manifest, &flags, target)?
        };
        audit(&graph, &manifest, profile.boundary, &context.root)?;
        println!(
            "network boundary passed: {} ({}, {:?})",
            profile.boundary.name(),
            profile.manifest,
            profile.features
        );
    }
    if !dependencies_only {
        for profile in profiles()
            .into_iter()
            .filter(|p| p.boundary != Boundary::Neutral)
            .chain([Profile {
                boundary: Boundary::RadioCore,
                manifest: "driver/runtime/embassy/esp32s31/ieee80211/Cargo.toml",
                features: &[],
            }])
        {
            let mut command = context.cargo();
            command
                .args(["check", "--manifest-path", profile.manifest, "--locked"])
                .args(profile.features);
            if profile.boundary.product() {
                command.args(["--target", TARGET]);
            } else {
                command.arg("--all-targets");
            }
            process::run(&mut command)?;
        }
    }
    println!(
        "network adapter {} boundaries are clean",
        if dependencies_only {
            "dependency"
        } else {
            "compile and dependency"
        }
    );
    Ok(())
}
