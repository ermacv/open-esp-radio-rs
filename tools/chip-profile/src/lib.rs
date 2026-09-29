//! The chips this repository supports, from their tracked profiles.
//!
//! Every supported chip has `platform/<id>/chip.toml`, holding only what
//! differs between chips and cannot be derived from the id: the Rust target,
//! the boot flow, the chip name `espflash` uses and the silicon revisions.
//! Everything else follows the id by convention (`verification/<id>`,
//! `registers/<id>`, `hil/targets/<id>`, `qualification/targets/<id>`,
//! `target/hil/<id>`), and a capability exists where its directory does.
//! Host tools resolve a chip through [`Profile::load`] instead of matching
//! chip names, so an unknown chip is refused with the supported ones.

use std::path::{Path, PathBuf};

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
    /// The Rust target triple of its firmware.
    pub rust_target: String,
    pub boot: Boot,
    /// The `--chip` value of `espflash`.
    pub espflash_chip: String,
    /// Silicon revisions the repository's models and pins describe.
    pub revisions: Vec<String>,
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
        let esp32c5 = Profile::load(&root, "esp32c5").unwrap();
        assert_eq!(esp32c5.boot, Boot::EspIdfBootloader);
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
}
