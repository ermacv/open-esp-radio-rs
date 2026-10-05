//! Resolve facade consumers independently of workspace feature unification.

use crate::{Result, cargo};
use oer_process as process;
use oer_process::Checkout;

use super::super::common::*;

const MANIFEST: &str = "crates/oer/Cargo.toml";
/// The portable packages of each group; every chip adds its own from its
/// profile's `[packages]`.
const WIFI: &[&str] = &[
    "oer-ieee80211-sta",
    "oer-ieee80211-sta-service",
    "oer-ieee80211-rsn-service",
    "oer-ieee80211-ap",
    "oer-ieee80211-softmac",
    "embassy-net",
    "xarxa",
];
const BLUETOOTH: &[&str] = &[
    "oer-bluetooth-hci",
    "oer-bluetooth-hci-transport",
    "oer-bluetooth-ll",
];
const IEEE802154: &[&str] = &["oer-ieee802154"];

/// One facade consumer profile, resolved for one target.
struct Profile {
    /// None selects defaults; an empty string selects no features.
    features: Option<String>,
    required: Vec<String>,
    /// Groups the profile must not reach.
    forbidden: Vec<String>,
    /// A Bluetooth consumer, which must reach no Wi-Fi package.
    bluetooth_only: bool,
    target: String,
}

/// The package groups profiles forbid: the portable ones with every chip's.
struct Groups(std::collections::BTreeMap<&'static str, Vec<String>>);

impl Groups {
    fn of(chips: &oer_repo::chips::Chips) -> Self {
        let owned = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        let mut groups = std::collections::BTreeMap::from([
            ("wifi", owned(WIFI)),
            ("bluetooth", owned(BLUETOOTH)),
            ("ieee802154", owned(IEEE802154)),
            ("backends", Vec::new()),
        ]);
        for profile in chips.profiles() {
            let packages = &profile.packages;
            for (group, names) in [
                ("wifi", &packages.wifi),
                ("bluetooth", &packages.bluetooth),
                ("backends", &packages.backends),
            ] {
                groups
                    .get_mut(group)
                    .expect("every group is declared")
                    .extend(names.iter().cloned());
            }
        }
        Self(groups)
    }

    fn members(&self, group: &str) -> Result<&[String]> {
        self.0
            .get(group)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("facade profile forbids unknown group `{group}`").into())
    }
}

/// The portable profiles, resolved for each chip target, and each chip's own
/// profiles for its target.
fn profiles(chips: &oer_repo::chips::Chips) -> Vec<Profile> {
    let owned = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    let mut profiles = Vec::new();
    for target in chip_targets(chips) {
        for (features, required, forbidden, bluetooth_only) in [
            (
                Some(""),
                &["oer-memory", "oer-network-interface", "oer-radio"][..],
                &["wifi", "bluetooth", "ieee802154"][..],
                false,
            ),
            (
                None,
                &["oer-ieee80211-sta", "oer-ieee80211-ap"],
                &["backends", "bluetooth", "ieee802154"],
                false,
            ),
            (
                Some("wifi"),
                &["oer-ieee80211-sta", "oer-ieee80211-ap"],
                &["backends", "bluetooth", "ieee802154"],
                false,
            ),
            (
                Some("bluetooth"),
                &["oer-bluetooth-hci", "oer-bluetooth-ll"],
                &["wifi", "backends", "ieee802154"],
                true,
            ),
            (
                Some("ieee802154"),
                &["oer-ieee802154"],
                &["wifi", "bluetooth", "backends"],
                false,
            ),
        ] {
            profiles.push(Profile {
                features: features.map(str::to_owned),
                required: owned(required),
                forbidden: owned(forbidden),
                bluetooth_only,
                target: target.clone(),
            });
        }
    }
    for chip in chips.profiles() {
        for profile in &chip.gate.facade {
            profiles.push(Profile {
                features: Some(profile.features.clone()),
                required: profile.required.clone(),
                forbidden: profile.forbidden.clone(),
                bluetooth_only: profile.bluetooth_only,
                target: chip.rust_target.clone(),
            });
        }
    }
    profiles
}

pub(super) fn check(
    ctx: &Checkout,
    chips: &oer_repo::chips::Chips,
    policy: &super::unsafe_policy::Policy,
) -> Result<()> {
    let model = model(ctx)?;
    let manifest = ctx.root.join(MANIFEST);
    let groups = Groups::of(chips);
    let profiles = profiles(chips);
    for profile in &profiles {
        let flags = match profile.features.as_deref() {
            None => vec![],
            Some("") => vec!["--no-default-features".into()],
            Some(features) => vec![
                "--no-default-features".into(),
                "--features".into(),
                features.into(),
            ],
        };
        let graph = cargo::isolated_graph(ctx, &manifest, &flags, Some(&profile.target))?;
        if profile.bluetooth_only {
            super::reject_wifi_in_bluetooth(&graph, &manifest, &policy.closed_pacs)?;
        }
        let packages = closure(&graph, &graph.root(&manifest)?)?;
        let contains = |name| packages.iter().any(|package| package.name.as_str() == name);
        for name in &profile.required {
            if !contains(name.as_str()) {
                return Err(
                    format!("facade profile {:?} is missing {name}", profile.features).into(),
                );
            }
        }
        for group in &profile.forbidden {
            for name in groups.members(group)? {
                if !contains(name.as_str()) {
                    continue;
                }
                return Err(format!(
                    "facade profile {:?} unexpectedly includes {name}",
                    profile.features
                )
                .into());
            }
        }
        // Pure protocol profiles must not pull hardware through an indirect edge.
        if matches!(
            profile.features.as_deref(),
            None | Some("" | "wifi" | "bluetooth" | "ieee802154")
        ) {
            for package in packages.iter().filter(|package| package.source.is_none()) {
                let directory = package
                    .manifest_path
                    .parent()
                    .and_then(|directory| directory.as_std_path().strip_prefix(&ctx.root).ok())
                    .and_then(|directory| directory.to_str())
                    .ok_or_else(|| format!("{} is outside the checkout", package.manifest_path))?;
                let local = model
                    .package_at(directory)
                    .ok_or_else(|| format!("{directory} is no package of the repository"))?;
                if matches!(
                    model.classification(local)?.platform,
                    Platform::Chip(_) | Platform::Family(_)
                ) {
                    return Err(format!(
                        "portable facade profile {:?} includes {}",
                        profile.features, package.name
                    )
                    .into());
                }
            }
        }
        process::run(
            oer_toolchain::cargo_in(&ctx.root)
                .args([
                    "test",
                    "--quiet",
                    "--locked",
                    "--package",
                    "open-esp-radio",
                    "--test",
                    "api",
                ])
                .args(&flags),
        )?;
    }
    eprintln!(
        "facade isolation and API checks passed ({} consumer profiles)",
        profiles.len()
    );
    Ok(())
}
