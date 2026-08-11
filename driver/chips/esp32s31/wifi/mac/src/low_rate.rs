//! Cross-layer ownership boundary for the PHY low-rate path.

use open_esp_radio_esp32s31_pac::ColdRadioRegisters;

/// Narrow PHY capability needed by the MAC cold-start policy.
///
/// The MAC chooses whether low-rate operation is wanted, while the generated
/// PAC owns the PHY register identities and the ordered hardware edges.
pub trait MacLowRateHardware {
    fn disable_phy_low_rate(&mut self);
}

impl MacLowRateHardware for ColdRadioRegisters {
    fn disable_phy_low_rate(&mut self) {
        self.configure_phy_low_rate(false);
    }
}
