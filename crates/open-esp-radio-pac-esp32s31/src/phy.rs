//! Ownership-bound leaves shared by the cold PHY prelude.

use super::RadioRegisters;

impl RadioRegisters {
    /// Program the fixed S31 40 MHz crystal-derived modem tick target.
    ///
    /// Complete pinned `libphy.a[phy_init.o]::phy_get_xtal_freq` replaces this
    /// six-bit field with `frequency_mhz - 1`, hence 39 for ESP32-S31.
    pub fn configure_fixed_xtal_40mhz_tick(&mut self) {
        // SAFETY: 39 fits the recovered six-bit target and follows the exact
        // transform proved by the complete blob function.
        unsafe {
            self.peripherals
                .modem_lpcon
                .tick_conf()
                .modify(|_, w| w.modem_pwr_tick_target().bits(39));
        }
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
