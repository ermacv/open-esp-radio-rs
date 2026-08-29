//! Safe runtime AGC register transactions.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;
use super::generated::AgcRuntimeEnableState;
pub use super::generated::ForcedRxGain;

const fn agc_runtime_enable_state(enabled: bool) -> AgcRuntimeEnableState {
    if enabled {
        AgcRuntimeEnableState::Enabled
    } else {
        AgcRuntimeEnableState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Apply complete rev0 ROM `phy_ant_dft_cfg`.
    pub fn configure_antenna_diversity(&mut self, enabled: bool) {
        super::generated::configure_antenna_diversity_state(
            &self.peripherals.phy_agc_oracle,
            agc_runtime_enable_state(enabled),
        );
    }

    /// Apply complete rev0 ROM `phy_force_rx_gain`.
    pub fn configure_forced_rx_gain(&mut self, enabled: bool, gain: ForcedRxGain) {
        let agc = &self.peripherals.phy_agc_oracle;
        super::generated::configure_forced_rx_gain_value(agc, gain);
        super::generated::configure_forced_rx_gain_state(agc, agc_runtime_enable_state(enabled));
    }
}
