//! Audit the compiled PHY library: it must link no vendor radio archive or
//! ROM ABI, and its dependency graph must stay within the reviewed packages.

use std::{collections::BTreeSet, io::Cursor, path::PathBuf};

use cargo_metadata::Message;

use super::{TARGET, artifacts, common};
use crate::{Context, Result, cargo, process};

const PHY: &str = "crates/hardware/esp32s31/phy/Cargo.toml";
const PHY_PACKAGES: &[&str] = &[
    "critical-section",
    "oer-memory",
    "oer-esp32s31-coex",
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
    // Portable IEEE 802.15.4 frame values the MAC engine is typed over.
    "oer-ieee802154",
    // The chip-neutral IEEE 802.15.4 MAC engine the HAL's radio owners drive.
    "oer-ieee802154-engine",
    // The chip-neutral modem clock planner behind the HAL's clock owners.
    "oer-radio-analog",
    "oer-radio-clock",
    // Safe structural pin projection for the observed child future; this is
    // a Rust macro library, with no allocator, native build or radio ABI.
    "pin-project-lite",
    "vcell",
];

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
    let output = process::capture(ctx.cargo().args([
        "build",
        "--locked",
        "--offline",
        "-p",
        "oer-esp32s31-phy",
        "--lib",
        "--release",
        "--target",
        TARGET,
        "--message-format=json-render-diagnostics",
    ]))?;
    let artifact = phy_artifact(&output.stdout)?;
    artifacts::audit_phy(ctx, &artifact)?;
    let manifest = ctx.root.join(PHY);
    let graph = cargo::metadata(ctx, &manifest, &[], Some(TARGET), true)?;
    for package in common::closure(&graph, &graph.root(&manifest)?)? {
        if !PHY_PACKAGES.contains(&package.name.as_str()) {
            return Err(format!(
                "unexpected package in source-only PHY graph: {}",
                package.name
            )
            .into());
        }
    }
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
