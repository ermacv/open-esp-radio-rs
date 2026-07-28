//! Generated-PAC ownership for the complete cold MAC antenna transaction.

use super::RadioRegisters;

impl RadioRegisters {
    /// Apply all 34 fresh-read RMW edges of `hal_attenna_init`.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_tx.o]`
    /// `hal_attenna_init`, size `0x5e`. The vendor symbol's original spelling
    /// is intentional. Both array traversals run from `0x20105510` down to
    /// `0x201051ac` with a `0x7c` stride.
    pub fn initialize_mac_antenna(&mut self) {
        let init = &self.peripherals.wifi_mac_antenna_init;

        // First complete reverse traversal: one fresh-read edge per word.
        for physical_bank in (0..8).rev() {
            init.bank_control(physical_bank)
                .modify(|_, w| w.first_clear_unknown().clear_bit());
        }

        // Second complete reverse traversal: the blob deliberately samples
        // each word again before every one of these three updates.
        for physical_bank in (0..8).rev() {
            let control = init.bank_control(physical_bank);
            control.modify(|_, w| w.second_clear_unknown().clear_bit());
            control.modify(|_, w| w.bank_enable_unknown().set_bit());
            control.modify(|_, w| w.third_clear_unknown().clear_bit());
        }

        init.common_control()
            .modify(|_, w| w.common_clear_unknown().clear_bit());
        init.common_control()
            .modify(|_, w| w.common_enable_unknown().set_bit());
    }
}
