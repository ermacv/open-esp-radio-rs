//! Ownership-bound access to the recovered ESP32-S31 PHY PBus.
//!
//! Register identities, multifunction fields and packed result windows come
//! from complete rev0 ROM PBus and clock-force bodies cited in the SVD.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;

const fn pbus_force_test_arguments(selector: u8, path: u8, test_value: u16) -> u32 {
    ((test_value as u32) << 6) | ((selector as u32) << 2) | ((path as u32) << 15)
}

impl RadioPhyRegisters {
    /// Publish one of the four reviewed force-TX/RX transition phases.
    pub fn set_pbus_force_txrx_mode(&mut self, enabled: bool, initial_phase: bool) {
        self.peripherals
            .phy_pbus
            .status_clock_force()
            .modify(|_, w| match (enabled, initial_phase) {
                (true, true) => w.force_txrx_mode_unknown().enable_initial_phase(),
                (true, false) => w.force_txrx_mode_unknown().enable_final_phase(),
                (false, true) => w.force_txrx_mode_unknown().disable_initial_phase(),
                (false, false) => w.force_txrx_mode_unknown().disable_final_phase(),
            });
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
        self.peripherals
            .phy_pbus
            .status_clock_force()
            .modify(|_, w| {
                if enabled {
                    w.tx_clock_enable_pair().enabled()
                } else {
                    w.tx_clock_enable_pair().disabled()
                }
            });
    }
}
