//! Safe runtime AGC register transactions.

#![forbid(unsafe_code)]

use super::RadioRegisters;

/// One complete byte accepted by the recovered forced-RX-gain leaf.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ForcedRxGain(u8);

impl ForcedRxGain {
    /// Construct the exact byte published by the hardware transaction.
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Return the value accepted by the generated PAC field writer.
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl RadioRegisters {
    /// Apply complete rev0 ROM `phy_ant_dft_cfg`.
    pub fn configure_antenna_diversity(&mut self, enabled: bool) {
        self.peripherals
            .phy_agc_oracle
            .antenna_control_0()
            .modify(|_, w| {
                if enabled {
                    w.antenna_diversity_enable_unknown().enabled()
                } else {
                    w.antenna_diversity_enable_unknown().disabled()
                }
            });
    }

    /// Apply complete rev0 ROM `phy_force_rx_gain`.
    pub fn configure_forced_rx_gain(&mut self, enabled: bool, gain: ForcedRxGain) {
        let control = self.peripherals.phy_agc_oracle.agc_shared_control();
        control.modify(|_, w| w.control_high_unknown().set(gain.get()));
        control.modify(|_, w| w.pulse_unknown().bit(enabled));
    }
}

#[cfg(test)]
mod tests {
    use super::ForcedRxGain;

    #[test]
    fn forced_gain_owns_one_complete_byte() {
        assert_eq!(ForcedRxGain::new(0).get(), 0);
        assert_eq!(ForcedRxGain::new(u8::MAX).get(), u8::MAX);
    }
}
