//! The chips with a vendor verification project and its fixed layout.
//!
//! The supported chips are those with a profile
//! ([`oer_chip_profile::Profile`]); a chip has a vendor verification project
//! when `verification/<chip>/artifacts.toml` also exists. Its verification
//! files live at fixed paths below that directory, so no task keeps its own
//! list of chips.
use crate::Result;
use std::path::Path;

/// Directory of every chip's verification project, relative to the root.
const VERIFICATION: &str = "verification";

/// Every supported chip, sorted: the chips with a profile.
pub fn supported(root: &Path) -> Result<Vec<String>> {
    oer_chip_profile::supported(root)
}

/// The package of the vendor scenarios' command line, and its binary.
const SCENARIO_COMMAND: &str = "oer-vendor-scenario-cli";
const SCENARIO_BINARY: &str = "vendor-scenarios";

/// A chip's typed vendor scenarios: the library that decides their
/// verdicts, and the command package whose binary runs them.
#[derive(Debug, Eq, PartialEq)]
pub struct Scenarios {
    pub library: String,
    pub command: String,
    pub binary: String,
}

/// The vendor verification project of one supported chip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project(String);

impl Project {
    /// `name`, when it is supported and has a verification project;
    /// otherwise an error that lists the supported chips or names the
    /// missing project.
    pub fn new(root: &Path, name: &str) -> Result<Self> {
        crate::manifest_path(root, name)?;
        Ok(Self(oer_chip_profile::Profile::load(root, name)?.id))
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
        crate::manifest(&self.0)
    }

    /// Directory of the chip's scenario evidence shards.
    pub fn evidence_shards(&self) -> String {
        self.verification("evidence/scenarios")
    }

    /// The platform's reviewed summaries of the ROM functions its images
    /// call, relative to the root.
    pub fn rom_summaries(&self) -> String {
        format!("platform/{}/linker/rom/functions.toml", self.0)
    }

    /// The chip's registry of reviewed vendor-function fingerprints.
    pub fn provenance_registry(&self) -> String {
        self.verification("facts/provenance.toml")
    }

    /// Packages and binary of the chip's typed vendor scenarios: its own
    /// library, and the one command line of every chip's scenarios, which
    /// takes the chip as its first argument.
    pub fn scenarios(&self) -> Scenarios {
        Scenarios {
            library: format!("oer-{}-vendor-scenarios", self.0),
            command: SCENARIO_COMMAND.to_owned(),
            binary: SCENARIO_BINARY.to_owned(),
        }
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
        let error = Project::new(&root(), "esp32").unwrap_err().to_string();
        assert!(error.contains("unsupported chip `esp32`"), "{error}");
        assert!(error.contains("esp32c5, esp32s31"), "{error}");
    }

    #[test]
    fn paths_follow_the_chip_directory() {
        let chip = Project::new(&root(), "esp32c5").unwrap();
        assert_eq!(chip.artifacts(), "verification/esp32c5/artifacts.toml");
        assert_eq!(
            chip.evidence_shards(),
            "verification/esp32c5/evidence/scenarios"
        );
        assert_eq!(
            chip.scenarios(),
            Scenarios {
                library: "oer-esp32c5-vendor-scenarios".to_owned(),
                command: "oer-vendor-scenario-cli".to_owned(),
                binary: "vendor-scenarios".to_owned(),
            }
        );
    }
}
