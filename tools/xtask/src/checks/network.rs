//! Network policies consume package identities, resolved features and declared edges.

use crate::{Context, Result, cargo, graph::Graph};
use cargo_metadata::{DependencyKind, Package};
use std::{collections::BTreeSet, path::Path};

const OWNED: &str = "oer-embassy-net-owned";
const TARGET: &str = "riscv32imafc-unknown-none-elf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    Neutral,
    Owned,
    Research,
    Datapath,
    RadioCore,
    OwnedProduct,
}
impl Boundary {
    pub fn name(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Owned => "owned",
            Self::Research => "research",
            Self::Datapath => "datapath",
            Self::RadioCore => "radio-core",
            Self::OwnedProduct => "owned-product",
        }
    }
    fn product(self) -> bool {
        self == Self::OwnedProduct
    }
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
/// The packet driver contract between a radio and the owned stack.
const DRIVER: &str = "xarxa-driver";
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
    p.name.starts_with("oer-esp32s31")
        || p.manifest_path
            .as_std_path()
            .starts_with(root.join("crates/hardware"))
}
fn optimized(p: &Package) -> bool {
    p.name == OWNED
        || xarxa_api(p.name.as_str())
        || (network_api(p.name.as_str()) && !official_registry(p))
}
fn stack(name: &str) -> bool {
    name.starts_with("embassy-") || name.starts_with("oer-embassy") || xarxa_api(name)
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
            // The owned adapter may declare the portable `oer-memory`
            // handoff contract, whose detached DMA slots it adopts for
            // zero-copy RX, but never a chip or hardware owner.
            Boundary::Owned => {
                name.starts_with("oer-esp32s31")
                    || (name == DRIVER && released)
                    || dependency.path.as_ref().is_some_and(|path| {
                        path.as_std_path()
                            .starts_with(repository.join("crates/hardware"))
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
        Boundary::Owned => {
            reject(
                &|p| p.name == DRIVER && official_registry(p),
                "owned adapter acquired the released driver contract",
            )?;
            // Reachability also covers a chip owner behind `oer-memory`.
            reject(
                &|p| physical(p, repository),
                "owned adapter acquired physical radio ownership",
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
        Boundary::RadioCore => reject(
            &optimized,
            "radio core acquired the optimized network graph",
        )?,
        Boundary::OwnedProduct => {
            required(&|p| p.name == OWNED, "owned network adapter")?;
            required(
                &|p| p.name == DRIVER && pinned_git(p),
                "owned Xarxa driver contract",
            )?;
            required(&|p| p.name == "xarxa" && pinned_git(p), "owned Xarxa stack")?;
            required(
                &|p| p.name == "embassy-net" && pinned_git(p),
                "owned Embassy network stack",
            )?;
            // One Xarxa revision pins both the stack and its driver contract, so
            // a radio and the stack can never disagree about packet ownership.
            let sources = |api: fn(&str) -> bool| {
                dependencies
                    .iter()
                    .filter(|p| api(p.name.as_str()))
                    .filter_map(|p| p.source.as_ref().map(|source| source.repr.as_str()))
                    .collect::<BTreeSet<_>>()
                    .len()
            };
            if sources(xarxa_api) != 1 {
                return Err(
                    "owned-product: Xarxa stack and driver must resolve to the same pinned source"
                        .into(),
                );
            }
            if sources(network_api) != 1 {
                return Err(
                    "owned-product: Embassy network packages must resolve to one pinned source"
                        .into(),
                );
            }
        }
    }
    let features = &graph.node(&root).features;
    let has = |feature: &str| features.iter().any(|f| f.as_str() == feature);
    if boundary.product() && !has("owned-network") {
        return Err(format!("{}: owned-network is not selected", boundary.name()).into());
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
pub fn profiles() -> [Profile; 10] {
    use Boundary::*;
    let product = "crates/composition/esp32s31/embassy/ieee80211/Cargo.toml";
    [
        Profile {
            boundary: OwnedProduct,
            manifest: "examples/esp32s31/access-point/Cargo.toml",
            features: &["--no-default-features", "--features", "owned-network"],
        },
        Profile {
            boundary: OwnedProduct,
            manifest: "hil/targets/esp32s31/runtime/Cargo.toml",
            features: &[
                "--no-default-features",
                "--features",
                "open-radio-hil,owned-network",
            ],
        },
        Profile {
            boundary: Neutral,
            manifest: "crates/network/interface/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Owned,
            manifest: "crates/adapters/embassy-net/owned/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Research,
            manifest: "experiments/network-engine/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: Research,
            manifest: "experiments/network-engine/Cargo.toml",
            features: &["--all-features"],
        },
        Profile {
            boundary: Datapath,
            manifest: "crates/protocols/ieee80211/datapath/Cargo.toml",
            features: &[],
        },
        Profile {
            boundary: RadioCore,
            manifest: "crates/runtime/esp32s31/ieee80211/Cargo.toml",
            features: &["--no-default-features"],
        },
        Profile {
            boundary: OwnedProduct,
            manifest: product,
            features: &[],
        },
        Profile {
            boundary: OwnedProduct,
            manifest: "examples/esp32s31/station/Cargo.toml",
            features: &["--no-default-features", "--features", "owned-network"],
        },
    ]
}

pub fn run(context: &Context) -> Result<()> {
    for profile in profiles() {
        let manifest = context.root.join(profile.manifest);
        let flags = profile
            .features
            .iter()
            .map(|v| (*v).to_owned())
            .collect::<Vec<_>>();
        let target = profile.boundary.product().then_some(TARGET);
        let graph = if profile.manifest.starts_with("hil/") {
            cargo::metadata(context, &manifest, &flags, target, true)?
        } else if profile.manifest.starts_with("examples/") {
            // Binary examples cannot become a scratch consumer's dependency;
            // they resolve in the examples workspace. Its feature unification
            // can only add to the example's graph, never hide a forbidden
            // dependency, and the audit walks from the example's own package.
            let examples = context.root.join("examples/esp32s31/Cargo.toml");
            if cargo::workspace_manifest(context, &manifest)? != examples.canonicalize()? {
                return Err(format!(
                    "example must belong to the examples workspace: {}",
                    manifest.display()
                )
                .into());
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
    println!("network adapter dependency boundaries are clean");
    Ok(())
}
