//! Chip properties as compile-time configuration.
//!
//! Code above the PAC is written once for every chip. Where chips differ in
//! what they have — a radio band, a Bluetooth mode, the number of cores —
//! the difference is a property of `platform/<chip>/chip.toml`, and a package
//! built for one chip sees it as a `cfg` or a constant, never as a run-time
//! choice. Differences of address or register layout are not properties:
//! they belong in the register model.
//!
//! A package that is built for one selected chip calls [`emit`] from its
//! build script. The chip is the package's one enabled feature named after a
//! chip id (`esp32s31`, `esp32c5`); a package built with none sees no chip
//! `cfg`, and one built with two fails to build.
//!
//! | Property | `cfg` or constant |
//! | --- | --- |
//! | `wifi-bands = ["2g4", "5g"]` | `oer_wifi_band_2g4`, `oer_wifi_band_5g` |
//! | `bluetooth = ["le", "br-edr"]` | `oer_bluetooth_le`, `oer_bluetooth_br_edr` |
//! | `ieee802154 = true` | `oer_ieee802154` |
//! | `cores = 2` | `pub const CORES: usize = 2;` in `chip_properties.rs` |

#![forbid(unsafe_code)]

use std::{env, fmt::Write as _, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/profiles.rs"));

/// Every `cfg` a chip property can set; `rustc-check-cfg` rejects any other
/// `oer_` spelling in a package that calls [`emit`].
pub const CFGS: &[&str] = &[
    "oer_wifi_band_2g4",
    "oer_wifi_band_5g",
    "oer_bluetooth_le",
    "oer_bluetooth_br_edr",
    "oer_ieee802154",
];

/// A Wi-Fi band the chip's radio serves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum WifiBand {
    #[serde(rename = "2g4")]
    Band2g4,
    #[serde(rename = "5g")]
    Band5g,
}

/// A Bluetooth mode the chip's controller serves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BluetoothMode {
    Le,
    BrEdr,
}

/// The `[properties]` table of a chip profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Properties {
    pub wifi_bands: Vec<WifiBand>,
    pub bluetooth: Vec<BluetoothMode>,
    pub ieee802154: bool,
    pub cores: u8,
}

impl Properties {
    /// The `cfg`s these properties set.
    pub fn cfgs(&self) -> Vec<&'static str> {
        let mut cfgs = Vec::new();
        for band in &self.wifi_bands {
            cfgs.push(match band {
                WifiBand::Band2g4 => "oer_wifi_band_2g4",
                WifiBand::Band5g => "oer_wifi_band_5g",
            });
        }
        for mode in &self.bluetooth {
            cfgs.push(match mode {
                BluetoothMode::Le => "oer_bluetooth_le",
                BluetoothMode::BrEdr => "oer_bluetooth_br_edr",
            });
        }
        if self.ieee802154 {
            cfgs.push("oer_ieee802154");
        }
        cfgs
    }

    /// Rust source of the numeric properties, for `include!`.
    pub fn constants(&self) -> String {
        format!(
            "/// Harts of the chip's high-performance CPU.\npub const CORES: usize = {};\n",
            self.cores
        )
    }
}

#[derive(Deserialize)]
struct Profile {
    properties: Properties,
}

/// The ids of every chip with a profile, sorted.
pub fn chips() -> impl Iterator<Item = &'static str> {
    PROFILES.iter().map(|(id, _)| *id)
}

/// The properties of `chip`, or an error naming the supported chips.
pub fn properties(chip: &str) -> Result<Properties, String> {
    let (_, text) = PROFILES.iter().find(|(id, _)| *id == chip).ok_or_else(|| {
        format!(
            "unsupported chip `{chip}`; supported: {}",
            chips().collect::<Vec<_>>().join(", ")
        )
    })?;
    toml::from_str::<Profile>(text)
        .map(|profile| profile.properties)
        .map_err(|error| format!("platform/{chip}/chip.toml: {error}"))
}

/// The one chip among `enabled` feature names, `None` without one.
pub fn selected<'a>(
    enabled: impl IntoIterator<Item = &'a str>,
) -> Result<Option<&'static str>, String> {
    let enabled = enabled.into_iter().collect::<Vec<_>>();
    let mut chosen = chips().filter(|chip| enabled.contains(chip));
    match (chosen.next(), chosen.next()) {
        (chip, None) => Ok(chip),
        (Some(first), Some(second)) => Err(format!(
            "a package is built for one chip, but both `{first}` and `{second}` are enabled"
        )),
        (None, Some(_)) => unreachable!("a second chip without a first"),
    }
}

/// From a build script: set the `cfg`s of the package's selected chip,
/// declare every property `cfg` to `rustc-check-cfg`, and write
/// `$OUT_DIR/chip_properties.rs` with the numeric properties (empty without a
/// chip).
///
/// # Panics
///
/// When two chip features are enabled or the chip's profile does not parse:
/// a build for no single chip must fail.
pub fn emit() {
    let features = chips()
        .filter(|chip| {
            env::var_os(format!(
                "CARGO_FEATURE_{}",
                chip.to_uppercase().replace('-', "_")
            ))
            .is_some()
        })
        .collect::<Vec<_>>();
    let chip = selected(features).unwrap_or_else(|error| panic!("{error}"));
    let mut check = String::new();
    for cfg in CFGS {
        if !check.is_empty() {
            check.push_str(", ");
        }
        write!(check, "{cfg}").expect("formatting to a string");
    }
    println!("cargo::rustc-check-cfg=cfg({check})");
    let constants = match chip {
        Some(chip) => {
            let properties = properties(chip).unwrap_or_else(|error| panic!("{error}"));
            for cfg in properties.cfgs() {
                println!("cargo::rustc-cfg={cfg}");
            }
            properties.constants()
        }
        None => String::new(),
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("set by Cargo")).join("chip_properties.rs"),
        constants,
    )
    .expect("writable OUT_DIR");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tracked_chip_has_parseable_properties() {
        let chips = chips().collect::<Vec<_>>();
        assert!(
            chips.contains(&"esp32s31") && chips.contains(&"esp32c5"),
            "{chips:?}"
        );
        for chip in chips {
            let properties = properties(chip).unwrap();
            assert!(properties.cores >= 1, "{chip}");
            for cfg in properties.cfgs() {
                assert!(CFGS.contains(&cfg), "{chip}: {cfg} is not declared");
            }
        }
    }

    #[test]
    fn the_dual_band_chip_alone_sets_the_5_ghz_cfg() {
        assert!(
            properties("esp32c5")
                .unwrap()
                .cfgs()
                .contains(&"oer_wifi_band_5g")
        );
        assert!(
            !properties("esp32s31")
                .unwrap()
                .cfgs()
                .contains(&"oer_wifi_band_5g")
        );
        assert!(
            properties("esp32s31")
                .unwrap()
                .constants()
                .contains("CORES: usize = 2")
        );
    }

    #[test]
    fn a_build_selects_at_most_one_chip() {
        assert_eq!(selected(["std"]).unwrap(), None);
        assert_eq!(selected(["esp32c5", "std"]).unwrap(), Some("esp32c5"));
        assert!(selected(["esp32c5", "esp32s31"]).is_err());
        assert!(properties("esp32zz").is_err());
    }
}
