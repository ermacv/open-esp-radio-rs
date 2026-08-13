//! Closed Bluetooth coexistence register transactions.

#![forbid(unsafe_code)]

use super::RadioRegisters;

impl RadioRegisters {
    /// Publish the complete reviewed Bluetooth PTI image.
    ///
    /// SOURCE: complete `libbtbb.a[bt_bb_v2.o]::coex_pti_v2` performs two
    /// ordered fresh-read read-modify-write operations on one register. The
    /// first replaces the high halfword with `0x0640`; the second takes a new
    /// sample and replaces the low halfword with `0x0010`. The meanings of
    /// the individual bits remain unknown, so neither halfword nor the raw
    /// register is exposed to callers.
    pub fn configure_reviewed_bluetooth_pti(&mut self) {
        let register = self.peripherals.phy_fecoex_recovered.bt_coex_pti_config();

        register.modify(|_, writer| writer.high_image_unknown().reviewed_image());
        register.modify(|_, writer| writer.low_image_unknown().reviewed_image());
    }
}
