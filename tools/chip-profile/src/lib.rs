//! The chips this repository supports, from their tracked profiles.
//!
//! Every supported chip has `platform/<id>/chip.toml`, holding only what
//! differs between chips and cannot be derived from the id: the chip family,
//! the Rust target, the boot flow, the chip name `espflash` uses, the silicon revisions and
//! the chip's properties (radio bands, Bluetooth modes, cores).
//! Everything else follows the id by convention (`verification/<id>`,
//! `registers/<id>`, `hil/targets/<id>`, `qualification/targets/<id>`,
//! `target/hil/<id>`), and a capability exists where its directory does.
//! Host tools resolve a chip through [`Profile::load`] instead of matching
//! chip names, so an unknown chip is refused with the supported ones.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Format of `chip.toml`.
pub const SCHEMA: u32 = 1;
/// Directory of the platforms, relative to the repository root.
const PLATFORM: &str = "platform";
const PROFILE: &str = "chip.toml";

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How the chip starts an application.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Boot {
    /// The ROM loads the platform's bootstrap, which stages the runtime.
    Staged,
    /// The ESP-IDF second-stage bootloader loads an application image.
    EspIdfBootloader,
}

/// One supported chip.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Profile {
    schema: u32,
    /// The chip id: lowercase, as in paths, package names and evidence.
    pub id: String,
    /// The family whose shared code the chip uses: a `family` package
    /// declaring this id builds for, and may be used by, every chip of it.
    pub family: String,
    /// The Rust target triple of its firmware.
    pub rust_target: String,
    pub boot: Boot,
    /// The `--chip` value of `espflash`.
    pub espflash_chip: String,
    /// Silicon revisions the repository's models and pins describe.
    pub revisions: Vec<String>,
    /// Where the chip's images lie in flash: the image pipeline encodes
    /// for it, and every flash writes there. `None` for a chip that has no
    /// images yet.
    #[serde(default)]
    pub flash: Option<FlashMap>,
    /// What the chip has.
    pub properties: Properties,
}

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

/// The chip's flash map (`[flash]` of its `chip.toml`): where the
/// second-stage bootloader, the partition table, the application and the
/// OTA selection lie, and the partition tables the platform defines.
///
/// A partition table named here is the source of the partitions it holds;
/// the image pipeline checks that `application` and `otadata` are the
/// offsets of its first application partition and its OTA data partition.
/// A chip whose ESP-IDF build makes its partition table names none.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FlashMap {
    pub bootloader: u32,
    pub partition_table: u32,
    /// The partition an image's application is written to.
    pub application: u32,
    /// The OTA data partition, which selects the application slot; `None`
    /// when the bootloader boots a factory application.
    #[serde(default)]
    pub otadata: Option<u32>,
    /// The partition table images boot with, as a CSV relative to the
    /// repository root.
    #[serde(default)]
    pub partitions: Option<PathBuf>,
    /// How a written image starts.
    pub start: Start,
}

/// How the stand starts an image it wrote into a board's flash.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Start {
    /// The writer's hard reset after the last segment starts the image.
    Reset,
    /// The writer leaves the ROM in its download mode; a power-on reset of
    /// the board's hub port starts the image, or an RTS reset on a board
    /// that does not reset by power. An RTS reset out of download mode starts
    /// an esp32c5's image with its USB Serial/JTAG console silent, and the
    /// writer's own reset can leave it in download mode.
    PowerOn,
}

impl Profile {
    /// The profile of `id`, or an error naming the supported chips.
    pub fn load(root: &Path, id: &str) -> Result<Self> {
        let path = profile_path(root, id);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "unsupported chip `{id}`; supported: {}",
                    supported(root)?.join(", ")
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        };
        let profile: Self =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        if profile.schema != SCHEMA {
            return Err(format!(
                "{} has schema {}; this tool reads schema {SCHEMA}",
                path.display(),
                profile.schema
            )
            .into());
        }
        if profile.id != id {
            return Err(format!("{} names chip `{}`", path.display(), profile.id).into());
        }
        if !valid_identifier(&profile.family) {
            return Err(format!(
                "{} names invalid family `{}`",
                path.display(),
                profile.family
            )
            .into());
        }
        Ok(profile)
    }

    /// Every supported chip, sorted by id.
    pub fn all(root: &Path) -> Result<Vec<Self>> {
        supported(root)?
            .iter()
            .map(|id| Self::load(root, id))
            .collect()
    }

    /// `<root>/<directory>/<id>`, such as `verification/esp32c5`.
    pub fn directory(&self, root: &Path, directory: &str) -> PathBuf {
        root.join(directory).join(&self.id)
    }

    /// The Cargo workspace of the chip's platform, which holds the
    /// bootstrap of a staged boot.
    pub fn platform_workspace(&self, root: &Path) -> PathBuf {
        self.directory(root, PLATFORM)
    }

    /// The bootstrap package a staged boot builds into the image beside the
    /// HIL agent; `None` when the chip's bootloader loads the agent itself.
    pub fn bootstrap_package(&self) -> Option<String> {
        (self.boot == Boot::Staged).then(|| format!("oer-{}-platform-bootstrap", self.id))
    }
}

/// The Rust target of `id`'s firmware, from its `chip.toml`.
pub fn rust_target(root: &Path, id: &str) -> Result<String> {
    Ok(Profile::load(root, id)?.rust_target)
}

/// The family of every supported chip, keyed by chip id.
pub fn families(root: &Path) -> Result<BTreeMap<String, String>> {
    Ok(Profile::all(root)?
        .into_iter()
        .map(|profile| (profile.id, profile.family))
        .collect())
}

/// A chip or family identifier: a lowercase ASCII letter, then lowercase
/// letters, digits or hyphens.
pub fn valid_identifier(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Ids of the chips with a profile, sorted.
pub fn supported(root: &Path) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(root.join(PLATFORM))? {
        let entry = entry?;
        if entry.path().join(PROFILE).is_file() {
            ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

fn profile_path(root: &Path, id: &str) -> PathBuf {
    root.join(PLATFORM).join(id).join(PROFILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_tracked_profiles_load() {
        let root = repository();
        let chips = Profile::all(&root).unwrap();
        assert!(chips.iter().any(|chip| chip.id == "esp32s31"));
        assert!(chips.iter().all(|chip| chip.family == "espressif"));
        assert_eq!(families(&root).unwrap().len(), chips.len());
        let esp32c5 = Profile::load(&root, "esp32c5").unwrap();
        assert_eq!(esp32c5.boot, Boot::EspIdfBootloader);
        let flash = esp32c5.flash.clone().unwrap();
        assert!(flash.bootloader < flash.partition_table);
        assert!(flash.partition_table < flash.application);
        assert_eq!((flash.otadata, flash.partitions), (None, None));
        let esp32s31 = Profile::load(&root, "esp32s31").unwrap().flash.unwrap();
        assert!(esp32s31.partition_table < esp32s31.otadata.unwrap());
        assert!(root.join(esp32s31.partitions.unwrap()).is_file());
        assert_eq!(
            esp32c5.directory(&root, "verification"),
            root.join("verification/esp32c5")
        );
    }

    #[test]
    fn an_unknown_chip_names_the_supported_ones() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(PLATFORM).join("esp32x9");
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(
            platform.join(PROFILE),
            "schema = 1\nid = \"esp32x9\"\nrust-target = \"riscv32imac-unknown-none-elf\"\n\
             boot = \"esp-idf-bootloader\"\nespflash-chip = \"esp32x9\"\nrevisions = [\"rev0\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join(PLATFORM).join("no-profile")).unwrap();
        let error = Profile::load(root.path(), "esp32s2")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "unsupported chip `esp32s2`; supported: esp32x9");
        assert_eq!(supported(root.path()).unwrap(), ["esp32x9"]);
    }

    #[test]
    fn a_profile_must_name_its_own_chip() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(PLATFORM).join("esp32x9");
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(
            platform.join(PROFILE),
            "schema = 1\nid = \"other\"\nrust-target = \"t\"\nboot = \"staged\"\n\
             espflash-chip = \"x\"\nrevisions = []\n",
        )
        .unwrap();
        assert!(Profile::load(root.path(), "esp32x9").is_err());
    }

    #[test]
    fn a_profile_must_name_a_valid_family() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(PLATFORM).join("esp32x9");
        std::fs::create_dir_all(&platform).unwrap();
        let profile = |family: &str| {
            format!(
                "schema = 1\nid = \"esp32x9\"\nfamily = \"{family}\"\n\
                 rust-target = \"t\"\nboot = \"staged\"\nespflash-chip = \"x\"\n\
                 revisions = []\n[properties]\nwifi-bands = []\nbluetooth = []\n\
                 ieee802154 = false\ncores = 1\n"
            )
        };
        std::fs::write(platform.join(PROFILE), profile("espressif")).unwrap();
        assert_eq!(
            Profile::load(root.path(), "esp32x9").unwrap().family,
            "espressif"
        );
        std::fs::write(platform.join(PROFILE), profile("Espressif")).unwrap();
        let error = Profile::load(root.path(), "esp32x9")
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid family"), "{error}");
    }
}
