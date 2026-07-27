//! Ownership-bound leaves shared by the cold PHY prelude.

use super::RadioRegisters;

impl RadioRegisters {
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
