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

use crate::{Result, cargo};
use oer_process as process;
use oer_process::Checkout;

/// Shared packages the PHY build may compile for the chip target; the chip's
/// own (its profile's `[gate.phy] packages`) join them.
const PHY_PACKAGES: &[&str] = &[
    // The zero-valid marker `oer-memory`'s zeroed statics use (the one
    // esp-hal's `#[ram(zeroed)]` requires); a no-std trait crate.
    "bytemuck",
    "critical-section",
    "oer-memory",
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
fn check_built_packages(
    built: &BuiltPackages,
    phy: &oer_repo::chips::profile::PhyLibrary,
) -> Result<()> {
    if !built.chip.contains(&phy.package) {
        return Err(format!(
            "the PHY build reported no chip-target artifact of {}; \
             its packages cannot be audited",
            phy.package
        )
        .into());
    }
    for package in &built.chip {
        if !PHY_PACKAGES.contains(&package.as_str()) && !phy.packages.contains(package) {
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

fn phy_artifact(messages: &[u8], package: &str) -> Result<PathBuf> {
    let library = package.replace('-', "_");
    let mut artifacts = BTreeSet::new();
    for message in Message::parse_stream(Cursor::new(messages)) {
        if let Message::CompilerArtifact(artifact) = message?
            && artifact.target.name == library
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

fn phy(
    ctx: &Checkout,
    profile: &oer_repo::chips::profile::Profile,
    phy: &oer_repo::chips::profile::PhyLibrary,
) -> Result<PathBuf> {
    let target = &profile.rust_target;
    let output = process::capture(oer_toolchain::cargo_in(&ctx.root).args([
        "build",
        "--locked",
        "-p",
        &phy.package,
        "--lib",
        "--release",
        "--target",
        target,
        "--message-format=json-render-diagnostics",
    ]))?;
    let artifact = phy_artifact(&output.stdout, &phy.package)?;
    // The archive's symbol policy reads ELF: its own check process. The
    // PHY's code may call into the chip's other packages its build compiles.
    let mut audit = oer_toolchain::cargo_in(&ctx.root);
    audit
        .args([
            "run",
            "--quiet",
            "--locked",
            "-p",
            "oer-check-phy-archive",
            "--",
        ])
        .arg(&artifact);
    for source in phy
        .packages
        .iter()
        .filter(|package| **package != phy.package)
    {
        audit.args(["--source", source]);
    }
    process::run(&mut audit)?;
    let graph = cargo::metadata(ctx, &ctx.root.join("Cargo.toml"), &[], Some(target), true)?;
    let names = graph
        .metadata
        .packages
        .iter()
        .map(|package| (package.manifest_path.clone(), package.name.to_string()))
        .collect();
    check_built_packages(&built_packages(&output.stdout, &names, target)?, phy)?;
    Ok(artifact)
}

/// Build `chip`'s production PHY library (`[gate.phy]` of its profile) for
/// the chip target and audit its artifact and graph; a chip without one is
/// an error naming the chips that have one.
pub fn run(ctx: &Checkout, chip: &str) -> Result<()> {
    let chips = oer_repo::chips::Chips::at(&ctx.root)?;
    let with_phy = || {
        chips
            .profiles()
            .iter()
            .filter(|profile| profile.gate.phy.is_some())
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let profile = chips
        .profile(chip)
        .ok_or_else(|| format!("no chip profile `{chip}`"))?;
    let phy = profile.gate.phy.as_ref().ok_or_else(|| {
        format!(
            "chip {chip} has no production PHY library; supported: {}",
            with_phy()
        )
    })?;
    let artifact = self::phy(ctx, profile, phy)?;
    println!("PHY rlib audit passed: {}", artifact.display());
    Ok(())
}

/// [`run`] for every chip with a production PHY library.
pub fn run_all(ctx: &Checkout) -> Result<()> {
    for profile in oer_repo::chips::Chips::at(&ctx.root)?.profiles() {
        if profile.gate.phy.is_some() {
            run(ctx, &profile.id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "phy/tests.rs"]
mod tests;
