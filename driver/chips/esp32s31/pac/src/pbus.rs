//! Ownership-bound access to the recovered ESP32-S31 PHY PBus.
//!
//! Register identities, multifunction fields and packed result windows come
//! from complete rev0 ROM PBus and clock-force bodies cited in the SVD.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;

const PBUS_COMMAND_PRESERVE_MASK: u32 = 0xfffe_0001;
const PBUS_COMMAND_ARGUMENT_MASK: u32 = 0x0001_fffc;
const PBUS_TRANSACTION_START: u32 = 1 << 1;

const fn pbus_force_test_arguments(selector: u8, path: u8, test_value: u16) -> u32 {
    ((test_value as u32) << 6) | ((selector as u32) << 2) | ((path as u32) << 15)
}

/// Reproduce the complete rev0 ROM `phy_pbus_force_test` command encoder.
///
/// `esp32s31_rev0_rom.elf`, symbol `phy_pbus_force_test` at `0x2f82_4228`,
/// shifts `test_value` left by six and masks the combined selector/value/path
/// image with `0x0001_fffc`. It does not range-check `test_value` before that
/// operation. Consequently the two high retained bits of a signed RX-DCO
/// halfword can contribute to the physical path field. This helper preserves
/// that instruction-exact behavior rather than silently narrowing the input
/// to the separately described nine-bit result/value window.
pub const fn pbus_force_test_command_image(
    current: u32,
    selector: u8,
    path: u8,
    test_value: u16,
) -> u32 {
    let arguments = pbus_force_test_arguments(selector, path, test_value);
    (current & PBUS_COMMAND_PRESERVE_MASK)
        | (arguments & PBUS_COMMAND_ARGUMENT_MASK)
        | PBUS_TRANSACTION_START
}

impl RadioPhyRegisters {
    /// Replace the four-bit force-TX/RX mode field.
    pub fn set_pbus_force_txrx_mode(&mut self, mode: u8) -> bool {
        if mode > 0x0f {
            return false;
        }
        self.peripherals
            .phy_pbus
            .status_clock_force()
            .modify(|_, w| w.force_txrx_mode_unknown().set(mode));
        true
    }

    /// Enable or disable the PBus work-mode selector.
    pub fn set_pbus_work_mode(&mut self, enabled: bool) {
        self.peripherals
            .phy_pbus
            .mode()
            .modify(|_, w| w.work_mode_enable().bit(enabled));
    }

    /// Enable or disable the PBus debug-mode selector.
    pub fn set_pbus_debug_mode(&mut self, enabled: bool) {
        self.peripherals
            .phy_pbus
            .command()
            .modify(|_, w| w.debug_mode_enable().bit(enabled));
    }

    /// Sample the PBus busy bit exactly once.
    pub fn pbus_is_busy(&mut self) -> bool {
        self.peripherals
            .phy_pbus
            .status_clock_force()
            .read()
            .busy()
            .bit_is_set()
    }

    /// Publish one instruction-exact ROM force-test command in a fresh RMW edge.
    ///
    /// Like ROM, selector and path contribute only the bits retained by the
    /// combined argument mask. The value is also deliberately not narrowed:
    /// RX-DCO passes signed halfword images, and the complete encoder retains
    /// their low eleven bits while composing the physical command word.
    pub fn publish_pbus_force_test(&mut self, selector: u8, path: u8, test_value: u16) {
        super::generated::publish_pbus_force_test(
            &self.peripherals.phy_pbus,
            super::generated::PbusForceTestInput::new(pbus_force_test_arguments(
                selector, path, test_value,
            )),
        );
    }

    /// Clear the force-test transaction bit after one completed observation.
    pub fn clear_pbus_transaction(&mut self) {
        self.peripherals
            .phy_pbus
            .command()
            .modify(|_, w| w.transaction_start().clear_bit());
    }

    /// Read the packed nine-bit result selected by the recovered ROM tables.
    pub fn read_pbus_result(&mut self, selector: u8, path: u8) -> Option<u16> {
        let path_one = path == 1;
        Some(match selector {
            0 if path_one => self
                .peripherals
                .phy_pbus
                .read_result_3()
                .read()
                .result_window_2_unknown()
                .bits(),
            0 => self
                .peripherals
                .phy_pbus
                .read_result_3()
                .read()
                .result_window_1_unknown()
                .bits(),
            1 if path_one => self
                .peripherals
                .phy_pbus
                .read_result_0()
                .read()
                .result_window_1_unknown()
                .bits(),
            1 => self
                .peripherals
                .phy_pbus
                .read_result_0()
                .read()
                .result_window_0_rx_dco()
                .bits(),
            2 if path_one => self
                .peripherals
                .phy_pbus
                .read_result_1()
                .read()
                .result_window_0_unknown()
                .bits(),
            2 => self
                .peripherals
                .phy_pbus
                .read_result_2()
                .read()
                .result_window_2_unknown()
                .bits(),
            3 if path_one => self
                .peripherals
                .phy_pbus
                .read_result_2()
                .read()
                .result_window_1_unknown()
                .bits(),
            3 => self
                .peripherals
                .phy_pbus
                .read_result_2()
                .read()
                .result_window_0_unknown()
                .bits(),
            4 if path_one => self
                .peripherals
                .phy_pbus
                .read_result_3()
                .read()
                .result_window_0_unknown()
                .bits(),
            4 => self
                .peripherals
                .phy_pbus
                .read_result_4()
                .read()
                .result_window_2_unknown()
                .bits(),
            5 => self
                .peripherals
                .phy_pbus
                .read_result_4()
                .read()
                .result_window_1_unknown()
                .bits(),
            _ => self
                .peripherals
                .phy_pbus
                .read_result_4()
                .read()
                .result_window_0_unknown()
                .bits(),
        })
    }

    /// Enable or disable both recovered RX clock bits as one pair.
    pub fn set_pbus_rx_clock_pair(&mut self, enabled: bool) {
        self.peripherals
            .phy_pbus
            .status_clock_force()
            .modify(|_, w| {
                w.rx_clock_low_or_rxiq_status_first_unknown()
                    .bit(enabled)
                    .rx_clock_high_or_rxiq_status_second_unknown()
                    .bit(enabled)
            });
    }

    /// Enable or disable both recovered TX clock bits as one pair.
    pub fn set_pbus_tx_clock_pair(&mut self, enabled: bool) {
        let value = if enabled { 3 } else { 0 };
        self.peripherals
            .phy_pbus
            .status_clock_force()
            .modify(|_, w| w.tx_clock_enable_pair().set(value));
    }
}

#[cfg(test)]
mod tests {
    use super::pbus_force_test_command_image;

    #[test]
    fn force_test_encoder_matches_complete_rom_images() {
        assert_eq!(pbus_force_test_command_image(0, 4, 1, 0), 0x0000_8012);
        assert_eq!(
            pbus_force_test_command_image(u32::MAX, 3, 2, 0x100),
            0xffff_400f
        );
    }

    #[test]
    fn signed_rx_dco_halfword_keeps_the_rom_overlap_behavior() {
        // `-251_i16 as u16 == 0xff05`. ROM keeps the low eleven value bits,
        // so their upper two bits combine with path one into physical path 3.
        assert_eq!(pbus_force_test_command_image(0, 3, 1, 0xff05), 0x0001_c14e);
    }
}
