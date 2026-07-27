//! Safe, ownership-bound access to the recovered modem clock/reset registers.
//!
//! Register layout and field positions come from `svd/esp32s31-radio.svd`.
//! Operation values are independently evidenced by the pinned ESP32-S31
//! `esp-hal` clock implementation at commit `6899213e`. The complete cold-boot
//! ordering intentionally remains in the HAL crate.

use super::RadioRegisters;

impl RadioRegisters {
    /// Enable or disable the recovered PHY calibration clock.
    ///
    /// SOURCE[BLOB_LIBPHY_PHY_BB_INIT]; the complete parent sets this bit
    /// before executing the baseband calibration sequence.
    pub fn set_phy_calibration_clock(&mut self, enabled: bool) {
        self.peripherals
            .phy_clock_oracle
            .fe_bb_clock_control_opaque()
            .modify(|_, w| w.phy_calibration_clock_unknown().bit(enabled));
    }

    /// Open the recovered front-end and baseband clock gates.
    ///
    /// SOURCE[ROM_REV0_PHY_OPEN_FE_BB_CLK]; the four operations retain the
    /// complete ROM body's order. Opaque full-register values stay inside the
    /// PAC, while callers need only the unique mutable radio capability.
    pub fn open_frontend_baseband_clocks(&mut self) {
        // SAFETY: 0x1e7 is the instruction-exact full register value written
        // by the cited ROM leaf; the SVD deliberately marks the register
        // write-only because no field semantics have been inferred.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .fe_clock_gate_opaque()
                .write_with_zero(|w| w.bits(0x1e7));
        }

        // SAFETY: value three fits the recovered two-bit field and reproduces
        // the ROM OR mask without assigning meanings to its constituent bits.
        self.peripherals
            .phy_clock_oracle
            .fe_bb_clock_control_opaque()
            .modify(|_, w| unsafe { w.rom_fe_bb_enable_unknown().bits(3) });

        // SAFETY: all ones is the instruction-exact full register value
        // written by the cited ROM leaf to this write-only opaque gate.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .bb_clock_gate_opaque()
                .write_with_zero(|w| w.bits(u32::MAX));
        }

        // SAFETY: 0x0f fits the recovered four-bit field. The adjacent named
        // bit is independently present in the S31 PMU description.
        self.peripherals
            .pmu
            .hp_active_hp_ck_power()
            .modify(|_, w| unsafe {
                w.rom_open_fe_bb_unknown_low()
                    .bits(0x0f)
                    .hp_active_xpd_bb_i2c()
                    .set_bit()
            });
    }

    /// Close the recovered front-end and baseband clock gates.
    ///
    /// SOURCE[BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK]; this is the exact inverse
    /// three-operation blob leaf. It intentionally leaves PMU power policy to
    /// the surrounding lifecycle owner, matching the source.
    pub fn close_frontend_baseband_clocks(&mut self) {
        // SAFETY: zero is the complete blob value for this write-only opaque
        // gate register.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .fe_clock_gate_opaque()
                .write_with_zero(|w| w.bits(0));
        }

        // SAFETY: zero fits the recovered two-bit field.
        self.peripherals
            .phy_clock_oracle
            .fe_bb_clock_control_opaque()
            .modify(|_, w| unsafe { w.rom_fe_bb_enable_unknown().bits(0) });

        // SAFETY: zero is the complete blob value for this write-only opaque
        // gate register.
        unsafe {
            self.peripherals
                .phy_clock_oracle
                .bb_clock_gate_opaque()
                .write_with_zero(|w| w.bits(0));
        }
    }
}
