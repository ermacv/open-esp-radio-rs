//! Ownership-bound AGC control leaves.

use super::RadioRegisters;

impl RadioRegisters {
    /// Select the complete rev0 ROM AGC enable or disable sequence.
    pub fn set_agc_enabled(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        if !enabled {
            agc.agc_antenna_control()
                .modify(|_, w| w.agc_disable_unknown().set_bit());
            return;
        }

        agc.agc_antenna_control()
            .modify(|_, w| w.agc_disable_unknown().clear_bit());
        agc.agc_shared_control()
            .modify(|_, w| w.pulse_unknown().set_bit());
        agc.agc_shared_control()
            .modify(|_, w| w.pulse_unknown().clear_bit());
    }

    /// Publish both complete pinned RX-compensation fields in order.
    pub fn configure_rx_compensation(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        // SAFETY: 0xed fits both generated eight-bit fields.
        agc.agc_shared_control()
            .modify(|_, w| unsafe { w.rx_compensation_low_unknown().bits(0xed) });
        agc.rx_compensation_high_control()
            .modify(|_, w| unsafe { w.rx_compensation_high_unknown().bits(0xed) });
    }

    /// Pulse the generated DC-memory clear field through two fresh RMWs.
    pub fn clear_agc_dc_memory(&mut self) {
        let control = self.peripherals.phy_agc_oracle.dc_memory_control();
        control.modify(|_, w| w.clear_pulse_unknown().set_bit());
        control.modify(|_, w| w.clear_pulse_unknown().clear_bit());
    }

    /// Apply the two MMIO edges after the PBus work-mode 1 µs delay.
    pub fn configure_pbus_work_mode_pulse(&mut self) {
        let control = self.peripherals.phy_agc_oracle.agc_shared_control();
        // SAFETY: 0x32 fits the generated eight-bit field.
        control.modify(|_, w| unsafe { w.control_high_unknown().bits(0x32) });
        control.modify(|_, w| w.pulse_unknown().set_bit());
    }

    /// Clear the PBus work-mode pulse after the caller-owned 2 µs delay.
    pub fn clear_pbus_work_mode_pulse(&mut self) {
        self.peripherals
            .phy_agc_oracle
            .agc_shared_control()
            .modify(|_, w| w.pulse_unknown().clear_bit());
    }
}
