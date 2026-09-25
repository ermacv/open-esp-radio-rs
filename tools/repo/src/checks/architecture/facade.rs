//! Resolve facade consumers independently of workspace feature unification.

use crate::{Context, Result, cargo, process};

use super::super::{TARGET, common::*};

const MANIFEST: &str = "crates/oer/Cargo.toml";
const WIFI: &[&str] = &[
    "oer-ieee80211-sta",
    "oer-ieee80211-ap",
    "oer-ieee80211-softmac",
    "oer-esp32s31-ieee80211-mac",
    "oer-esp32s31-ieee80211-sta",
    "oer-esp32s31-ieee80211-ap",
    "oer-esp32s31-ieee80211-runtime",
    "oer-esp32s31-ieee80211-system",
    "embassy-net",
    "xarxa",
];
const BACKENDS: &[&str] = &[
    "oer-esp32s31-hal",
    "oer-esp32s31-bluetooth",
    "oer-esp32s31-ieee80211-mac",
    "oer-esp32s31-ieee80211-sta",
    "oer-esp32s31-ieee80211-ap",
];
const BLUETOOTH: &[&str] = &[
    "oer-bluetooth-hci",
    "oer-bluetooth-ll",
    "oer-esp32s31-bluetooth",
    "oer-esp32s31-bluetooth-system",
    "oer-esp32s31-bluetooth-runtime",
];
const IEEE802154: &[&str] = &["oer-ieee802154"];

struct Profile {
    /// None selects defaults; an empty string selects no features.
    features: Option<&'static str>,
    required: &'static [&'static str],
    forbidden: &'static [&'static [&'static str]],
}

pub(super) fn check(ctx: &Context) -> Result<()> {
    let manifest = ctx.root.join(MANIFEST);
    for profile in [
        Profile {
            features: Some(""),
            required: &["oer-memory", "oer-network-interface", "oer-radio"],
            forbidden: &[WIFI, BLUETOOTH, IEEE802154],
        },
        Profile {
            features: None,
            required: &["oer-ieee80211-sta", "oer-ieee80211-ap"],
            forbidden: &[BACKENDS, BLUETOOTH, IEEE802154],
        },
        Profile {
            features: Some("wifi"),
            required: &["oer-ieee80211-sta", "oer-ieee80211-ap"],
            forbidden: &[BACKENDS, BLUETOOTH, IEEE802154],
        },
        Profile {
            features: Some("bluetooth"),
            required: &["oer-bluetooth-hci", "oer-bluetooth-ll"],
            forbidden: &[WIFI, BACKENDS, IEEE802154],
        },
        Profile {
            features: Some("ieee802154"),
            required: &["oer-ieee802154"],
            forbidden: &[WIFI, BLUETOOTH, BACKENDS],
        },
        Profile {
            features: Some("esp32s31"),
            required: &["oer-esp32s31-hal"],
            forbidden: &[WIFI, BLUETOOTH],
        },
        Profile {
            features: Some("esp32s31-wifi"),
            required: &["oer-esp32s31-ieee80211-sta", "oer-esp32s31-ieee80211-ap"],
            forbidden: &[BLUETOOTH],
        },
        Profile {
            features: Some("esp32s31-bluetooth"),
            required: &[
                "oer-esp32s31-bluetooth",
                "oer-bluetooth-hci",
                "oer-bluetooth-ll",
            ],
            forbidden: &[WIFI],
        },
        Profile {
            features: Some("ieee802154,esp32s31"),
            required: &["oer-ieee802154", "oer-esp32s31-hal"],
            forbidden: &[WIFI, BLUETOOTH],
        },
        Profile {
            features: Some("embassy-esp32s31-bluetooth"),
            required: &[
                "oer-esp32s31-bluetooth-system",
                "oer-esp32s31-bluetooth-runtime",
            ],
            forbidden: &[WIFI],
        },
    ] {
        let flags = match profile.features {
            None => vec![],
            Some("") => vec!["--no-default-features".into()],
            Some(features) => vec![
                "--no-default-features".into(),
                "--features".into(),
                features.into(),
            ],
        };
        let graph = cargo::isolated_graph(ctx, &manifest, &flags, Some(TARGET))?;
        if matches!(
            profile.features,
            Some("bluetooth" | "esp32s31-bluetooth" | "embassy-esp32s31-bluetooth")
        ) {
            super::reject_wifi_in_bluetooth(&graph, &manifest)?;
        }
        let packages = closure(&graph, &graph.root(&manifest)?)?;
        let contains = |name| packages.iter().any(|package| package.name.as_str() == name);
        for name in profile.required {
            if !contains(*name) {
                return Err(
                    format!("facade profile {:?} is missing {name}", profile.features).into(),
                );
            }
        }
        for name in profile.forbidden.iter().flat_map(|group| group.iter()) {
            if contains(*name) {
                return Err(format!(
                    "facade profile {:?} unexpectedly includes {name}",
                    profile.features
                )
                .into());
            }
        }
        // Pure protocol profiles must not pull hardware through an indirect edge.
        if matches!(
            profile.features,
            None | Some("" | "wifi" | "bluetooth" | "ieee802154")
        ) {
            for package in packages.iter().filter(|package| package.source.is_none()) {
                if matches!(classification(package)?.platform, Platform::Chip(_)) {
                    return Err(format!(
                        "portable facade profile {:?} includes {}",
                        profile.features, package.name
                    )
                    .into());
                }
            }
        }
        process::run(
            ctx.cargo()
                .args([
                    "test",
                    "--quiet",
                    "--offline",
                    "--locked",
                    "--package",
                    "open-esp-radio",
                    "--test",
                    "api",
                ])
                .args(&flags),
        )?;
    }
    eprintln!("facade isolation and API checks passed (10 consumer profiles)");
    Ok(())
}
