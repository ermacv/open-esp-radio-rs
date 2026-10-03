//! Audit the compiled PHY library: it must link no vendor radio archive or
//! ROM ABI, and the packages its build compiles must stay within the
//! reviewed lists.
//!
//! The packages are read from the `compiler-artifact` messages of the PHY
//! build itself, not from `cargo metadata`: metadata resolves features across
//! the whole workspace, so a feature another package enables (a derive, say)
//! would appear in the graph although the PHY build never compiles it.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    path::PathBuf,
};

use cargo_metadata::{Message, TargetKind, camino::Utf8PathBuf};

use super::artifacts;
use crate::{Context, Result, cargo};
use oer_process as process;

const PHY: &str = "crates/hardware/esp32s31/phy/Cargo.toml";
const PHY_PACKAGE: &str = "oer-esp32s31-phy";
/// Packages the PHY build may compile for the chip target.
const PHY_PACKAGES: &[&str] = &[
    // The zero-valid marker `oer-memory`'s zeroed statics use (the one
    // esp-hal's `#[ram(zeroed)]` requires); a no-std trait crate.
    "bytemuck",
    "critical-section",
    "oer-memory",
    "oer-esp32s31-hal",
    "oer-esp32s31-pac",
    "oer-esp32s31-pac-raw",
    "oer-esp32s31-phy",
    // Portable PHY trace events and the typed trace ring they are recorded
    // in. The graph is audited with default features, so `oer-trace` brings
    // no timer: `embassy-time` comes only with the opt-in PHY `trace` feature.
    "oer-phy-trace",
    "oer-trace",
    // Shared register layouts and reviewed transactions of the Wi-Fi MAC that
    // the chip PAC places; closed register PACs, no vendor code.
    "oer-ieee80211-pac",
    "oer-ieee80211-pac-raw",
    // Shared IEEE 802.15.4 MAC values and transactions the chip PAC
    // re-exports; a closed register PAC crate, no vendor code.
    "oer-ieee802154-pac",
    // Portable IEEE 802.15.4 frame values the MAC engine is typed over.
    "oer-ieee802154",
    // The chip-neutral IEEE 802.15.4 MAC engine the HAL's radio owners drive,
    // and the portable trace events it records.
    "oer-espressif-ieee802154-engine",
    "oer-ieee802154-trace",
    // The Espressif coexistence priority table, timer policy and time-slice
    // schedule recovered from esp-coex-lib as reviewed source, over the
    // portable coexistence vocabulary; no vendor archive or radio ABI.
    "oer-espressif-coex",
    "oer-radio-coex",
    // Portable radio port vocabulary and monotonic time contracts the HAL's
    // radio owners are typed over; plain value types, no runtime or driver.
    "oer-radio-port",
    "oer-time",
    // The chip-neutral modem clock planner behind the HAL's clock owners.
    "oer-radio-analog",
    "oer-radio-clock",
    "vcell",
];

/// Proc macros the PHY build may run on the host: they add no code of their
/// own to the artifact but generate some of it.
const PHY_HOST_PACKAGES: &[&str] = &[];

/// The packages one build compiled: for the chip target, and as proc macros
/// on the host.
#[derive(Debug, Default, PartialEq)]
struct BuiltPackages {
    chip: BTreeSet<String>,
    host: BTreeSet<String>,
}

/// Collect the packages a build's `compiler-artifact` messages name. A
/// package is chip-built when an output lies under the target triple's
/// directory; build scripts and the host libraries behind them are not
/// published code and are skipped.
fn built_packages(
    messages: &[u8],
    names: &BTreeMap<Utf8PathBuf, String>,
    target: &str,
) -> Result<BuiltPackages> {
    let mut built = BuiltPackages::default();
    for message in Message::parse_stream(Cursor::new(messages)) {
        let Message::CompilerArtifact(artifact) = message? else {
            continue;
        };
        let name = names
            .get(&artifact.manifest_path)
            .ok_or_else(|| format!("built package {} has no metadata", artifact.manifest_path))?
            .clone();
        if artifact.target.kind.contains(&TargetKind::ProcMacro) {
            built.host.insert(name);
        } else if artifact
            .filenames
            .iter()
            .any(|file| file.components().any(|part| part.as_str() == target))
        {
            built.chip.insert(name);
        }
    }
    Ok(built)
}

/// Every chip-built package and every proc macro must be reviewed, and the
/// PHY itself must be among the chip-built packages: a build layout the
/// target-directory rule no longer recognizes fails rather than passing an
/// empty set.
fn check_built_packages(built: &BuiltPackages) -> Result<()> {
    if !built.chip.contains(PHY_PACKAGE) {
        return Err(format!(
            "the PHY build reported no chip-target artifact of {PHY_PACKAGE}; \
             its packages cannot be audited"
        )
        .into());
    }
    for package in &built.chip {
        if !PHY_PACKAGES.contains(&package.as_str()) {
            return Err(format!("unexpected package in source-only PHY build: {package}").into());
        }
    }
    for package in &built.host {
        if !PHY_HOST_PACKAGES.contains(&package.as_str()) {
            return Err(
                format!("unexpected host proc macro in source-only PHY build: {package}").into(),
            );
        }
    }
    Ok(())
}

fn phy_artifact(messages: &[u8]) -> Result<PathBuf> {
    let mut artifacts = BTreeSet::new();
    for message in Message::parse_stream(Cursor::new(messages)) {
        if let Message::CompilerArtifact(artifact) = message?
            && artifact.target.name == "oer_esp32s31_phy"
            && !artifact.profile.test
            && artifact
                .target
                .kind
                .contains(&cargo_metadata::TargetKind::Lib)
        {
            artifacts.extend(
                artifact
                    .filenames
                    .into_iter()
                    .filter(|p| p.extension() == Some("rlib")),
            );
        }
    }
    if artifacts.len() != 1 {
        return Err("build must emit exactly one PHY rlib".into());
    }
    Ok(artifacts
        .into_iter()
        .next()
        .expect("one artifact")
        .into_std_path_buf())
}

fn phy(ctx: &Context) -> Result<PathBuf> {
    let target = super::target(&ctx.root)?;
    let output = process::capture(ctx.cargo().args([
        "build",
        "--locked",
        "-p",
        "oer-esp32s31-phy",
        "--lib",
        "--release",
        "--target",
        &target,
        "--message-format=json-render-diagnostics",
    ]))?;
    let artifact = phy_artifact(&output.stdout)?;
    artifacts::audit_phy(ctx, &artifact)?;
    let manifest = ctx.root.join(PHY);
    let graph = cargo::metadata(ctx, &manifest, &[], Some(&target), true)?;
    let names = graph
        .metadata
        .packages
        .iter()
        .map(|package| (package.manifest_path.clone(), package.name.to_string()))
        .collect();
    check_built_packages(&built_packages(&output.stdout, &names, &target)?)?;
    Ok(artifact)
}

/// Build the PHY library for the chip target and audit its artifact and graph.
/// Only ESP32-S31 has a production PHY library; any other chip is an error
/// rather than a silent audit of the ESP32-S31 library.
pub fn run(ctx: &Context, chip: &str) -> Result<()> {
    if chip != "esp32s31" {
        return Err(
            format!("chip {chip} has no production PHY library; supported: esp32s31").into(),
        );
    }
    let artifact = phy(ctx)?;
    println!("PHY rlib audit passed: {}", artifact.display());
    Ok(())
}

#[cfg(test)]
#[path = "phy/tests.rs"]
mod tests;
