//! Ownership-bound leaves shared by the cold PHY prelude.

use super::RadioRegisters;

const fn encode_rx_dco_control_field(field: u8) -> u32 {
    ((field & 3) as u32) << 22
}

const fn decode_rx_dco_control_field(saved_field: u32) -> u8 {
    ((saved_field >> 22) & 3) as u8
}

impl RadioRegisters {
    /// Capture and clear the two RX-DCO control bits through two fresh reads.
    pub fn capture_and_clear_rx_dco_control(&mut self) -> u32 {
        let control = self.peripherals.phy_rx_dco_oracle.control();
        let saved =
            encode_rx_dco_control_field(control.read().calibration_control_unknown().bits());
        control.modify(|_, w| unsafe { w.calibration_control_unknown().bits(0) });
        saved
    }

    /// Restore only the captured RX-DCO control field through one fresh RMW.
    pub fn restore_rx_dco_control(&mut self, saved_field: u32) {
        self.peripherals
            .phy_rx_dco_oracle
            .control()
            .modify(|_, w| unsafe {
                w.calibration_control_unknown()
                    .bits(decode_rx_dco_control_field(saved_field))
            });
    }

    /// Sample the full-width counter used by the SDM-stability deadline.
    ///
    /// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76` proves the
    /// address and wrapping-difference consumer, but not the clock source.
    pub fn sample_sdm_deadline_counter(&mut self) -> u32 {
        self.peripherals
            .phy_cold_deadline_oracle
            .deadline_counter_unknown()
            .read()
            .bits()
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_rx_dco_control_field, encode_rx_dco_control_field};

    #[test]
    fn rx_dco_field_round_trip_is_bounded_to_bits_23_22() {
        assert_eq!(encode_rx_dco_control_field(3), 0x00c0_0000);
        assert_eq!(decode_rx_dco_control_field(0xff7f_ffff), 1);
        assert_eq!(
            encode_rx_dco_control_field(decode_rx_dco_control_field(0xff7f_ffff)),
            0x0040_0000
        );
    }
}
