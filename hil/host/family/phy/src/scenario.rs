//! The `[phy]` scenario table.

use std::path::Path;

use oer_esp32s31_phy_vendor_calibration::boots::Lifecycle;
use oer_hil_image_class::ImageClass;
use oer_hil_lab::config::LabConfig;
use oer_hil_scenario::{AirUse, Plan, ScenarioFamily, bounded};
use oer_hil_workload::context::Context;
use oer_hil_workload::{family::Workload, fixture::Fixtures};
use serde::{Deserialize, Serialize};

use crate::Result;

/// The chip whose PHY the cross-check compares.
pub const CHIP: &str = "esp32s31";

/// One PHY workload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PhyScenario {
    /// The vendor firmware's and the production image's PHY calibration
    /// and register state at one lifecycle point, alternating boot by boot
    /// on the board under test, compared under the reviewed relation.
    VendorCalibration {
        lifecycle: Lifecycle,
        /// Boots per side.
        boots: u8,
        /// The production image class; the lifecycle point's own by default
        /// (`correctness`, or `diagnostic-ieee802154-radio` at an IEEE
        /// 802.15.4 point).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<ImageClass>,
        /// The vendor firmware project of `verification/esp32s31/hil-vendor`;
        /// the lifecycle point's own by default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor_project: Option<String>,
        /// Further device windows the IEEE 802.15.4 reference firmware reads
        /// with `PEEK`: a vendor-side investigation, never compared.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        vendor_windows: Vec<Window>,
        /// After the reads, transmit one frame on the vendor side and read
        /// its analog image and windows again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        vendor_transmit: bool,
        /// Boot only the vendor firmware: an investigation without a
        /// comparison.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        vendor_only: bool,
    },
}

/// `words` consecutive device words from `address`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub address: u32,
    pub words: u32,
}

impl PhyScenario {
    /// The production image class the scenario compares.
    pub fn image(&self) -> ImageClass {
        let Self::VendorCalibration {
            lifecycle, image, ..
        } = self;
        image.unwrap_or(if lifecycle.ieee802154() {
            ImageClass::DiagnosticIeee802154Radio
        } else {
            ImageClass::Correctness
        })
    }
}

impl ScenarioFamily for PhyScenario {
    fn validate(&self) -> Result<()> {
        let Self::VendorCalibration {
            lifecycle,
            boots,
            vendor_project,
            vendor_windows,
            vendor_transmit,
            ..
        } = self;
        bounded(*boots, 1, 20, "boots")?;
        if vendor_project.as_deref().is_some_and(str::is_empty) {
            return Err("vendor_project names no project".into());
        }
        if (!vendor_windows.is_empty() || *vendor_transmit) && !lifecycle.ieee802154() {
            return Err("vendor windows and transmission need an IEEE 802.15.4 point".into());
        }
        for window in vendor_windows {
            if window.address % 4 != 0 || window.words == 0 {
                return Err("a vendor window needs a word-aligned address and a word".into());
            }
        }
        Ok(())
    }

    fn plan(&self) -> Plan {
        Plan::target_only(self.image())
    }

    /// Cold calibration measures TX and RX DC and IQ that another
    /// transmission on the band would bias.
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Band2G4]
    }
}

impl Workload for PhyScenario {
    fn precondition(&self, lab: &LabConfig) -> Result<()> {
        if lab.chip() != CHIP {
            return Err(format!(
                "the calibration cross-check compares the {CHIP} PHY, not {}",
                lab.chip()
            )
            .into());
        }
        Ok(())
    }

    fn run(&self, output: &Path, context: &Context<'_>, _fixtures: &Fixtures) -> Result<()> {
        let Self::VendorCalibration {
            lifecycle,
            boots,
            vendor_project,
            vendor_windows,
            vendor_transmit,
            vendor_only,
            ..
        } = self;
        crate::workload::calibration::run(
            &crate::workload::calibration::Calibration {
                lifecycle: *lifecycle,
                boots: *boots,
                image: self.image(),
                vendor_project: vendor_project
                    .as_deref()
                    .unwrap_or(lifecycle.vendor_project()),
                windows: vendor_windows,
                transmit: *vendor_transmit,
                vendor_only: *vendor_only,
            },
            output,
            context,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> PhyScenario {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn the_lifecycle_point_implies_the_image_and_vendor_project() {
        let cold = table("kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 10\n");
        cold.validate().unwrap();
        assert_eq!(cold.plan().image, ImageClass::Correctness);
        let radio = table(
            "kind = \"vendor-calibration\"\nlifecycle = \"ieee802154-restart\"\nboots = 3\n\
             vendor_windows = [{ address = 0x2010fc00, words = 2 }]\nvendor_transmit = true\n",
        );
        radio.validate().unwrap();
        assert_eq!(radio.plan().image, ImageClass::DiagnosticIeee802154Radio);
        assert_eq!(
            Lifecycle::Ieee802154Restart.vendor_project(),
            "ieee802154-reference"
        );
    }

    #[test]
    fn invalid_tables_are_rejected() {
        let windows_at_cold = table(
            "kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 1\n\
             vendor_windows = [{ address = 0x2010fc00, words = 2 }]\n",
        );
        assert!(windows_at_cold.validate().is_err());
        let unaligned = table(
            "kind = \"vendor-calibration\"\nlifecycle = \"ieee802154\"\nboots = 1\n\
             vendor_windows = [{ address = 0x2010fc02, words = 2 }]\n",
        );
        assert!(unaligned.validate().is_err());
        assert!(
            table("kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 0\n")
                .validate()
                .is_err()
        );
        assert!(
            toml::from_str::<PhyScenario>(
                "kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 1\ncolor = 1\n"
            )
            .is_err()
        );
    }
}
