//! The chips with a vendor verification project.
//!
//! A chip is supported when `verification/<chip>/artifacts.toml` exists. Its
//! verification files live at fixed paths below that directory, so no task
//! keeps its own list of chips.
use crate::Result;
use std::path::Path;

/// Directory of every chip's verification project, relative to the root.
const VERIFICATION: &str = "verification";

/// The pin every supported chip has.
const ARTIFACTS: &str = "artifacts.toml";

/// Every chip with a verification project, sorted.
pub fn supported(root: &Path) -> Result<Vec<String>> {
    let mut chips = vec![];
    for entry in std::fs::read_dir(root.join(VERIFICATION))? {
        let path = entry?.path();
        if path.join(ARTIFACTS).is_file()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            chips.push(name.to_owned());
        }
    }
    chips.sort();
    Ok(chips)
}

/// One supported chip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chip(String);

impl Chip {
    /// `name`, when it has a verification project; otherwise an error that
    /// lists the supported chips.
    pub fn new(root: &Path, name: &str) -> Result<Self> {
        let supported = supported(root)?;
        if supported.iter().any(|chip| chip == name) {
            Ok(Self(name.to_owned()))
        } else {
            Err(format!(
                "no verification project for chip {name}; supported chips: {}",
                supported.join(", ")
            )
            .into())
        }
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// `verification/<chip>/<path>`, relative to the root.
    pub fn verification(&self, path: &str) -> String {
        format!("{VERIFICATION}/{}/{path}", self.0)
    }

    /// The single pin of the chip's vendor artifacts.
    pub fn artifacts(&self) -> String {
        self.verification(ARTIFACTS)
    }

    /// Directory of the chip's scenario evidence shards.
    pub fn evidence_shards(&self) -> String {
        self.verification("evidence/scenarios")
    }

    /// The chip's registry of reviewed vendor-function fingerprints.
    pub fn provenance_registry(&self) -> String {
        self.verification("facts/provenance.toml")
    }

    /// Package and binary of the chip's typed vendor scenarios.
    pub fn scenarios(&self) -> (String, String) {
        (
            format!("oer-{}-vendor-scenarios", self.0),
            format!("{}-vendor-scenarios", self.0),
        )
    }

    /// Whether `path` lies in a directory named after another chip, so a
    /// scan for this chip skips it. A chip directory is a path component
    /// that names a supported chip.
    pub fn excludes(&self, path: &Path, supported: &[String]) -> bool {
        path.components().any(|component| {
            let component = component.as_os_str().to_str().unwrap_or_default();
            component != self.0 && supported.iter().any(|chip| chip == component)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn tracked_chips_are_supported_and_others_are_rejected_with_the_list() {
        let supported = supported(&root()).unwrap();
        assert!(supported.contains(&"esp32s31".to_owned()));
        assert!(supported.contains(&"esp32c5".to_owned()));
        let error = Chip::new(&root(), "esp32").unwrap_err().to_string();
        assert!(error.contains("esp32c5, esp32s31"), "{error}");
    }

    #[test]
    fn paths_follow_the_chip_directory() {
        let chip = Chip::new(&root(), "esp32c5").unwrap();
        assert_eq!(chip.artifacts(), "verification/esp32c5/artifacts.toml");
        assert_eq!(
            chip.evidence_shards(),
            "verification/esp32c5/evidence/scenarios"
        );
        assert_eq!(
            chip.scenarios(),
            (
                "oer-esp32c5-vendor-scenarios".to_owned(),
                "esp32c5-vendor-scenarios".to_owned()
            )
        );
    }

    #[test]
    fn a_scan_skips_other_chips_directories_only() {
        let chip = Chip::new(&root(), "esp32s31").unwrap();
        let supported = vec!["esp32c5".to_owned(), "esp32s31".to_owned()];
        assert!(chip.excludes(
            Path::new("crates/hardware/esp32c5/pac/src/lib.rs"),
            &supported
        ));
        assert!(!chip.excludes(
            Path::new("crates/hardware/esp32s31/hal/src/lib.rs"),
            &supported
        ));
        assert!(!chip.excludes(
            Path::new("crates/protocols/ieee802154/src/lib.rs"),
            &supported
        ));
    }
}
