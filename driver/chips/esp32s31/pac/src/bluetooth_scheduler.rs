//! Restricted ownership for the first controller scheduler-table transaction.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

impl BluetoothTaskRegisters {
    /// Clear the low twenty state bits of all sixteen scheduler entries.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` member `btdm_sched.c`
    /// symbol `r_sym_bt_XPuqTHliEO5V9xpR7aJR`. Its first hardware transaction
    /// walks `0x2010_b000..=0x2010_b0f0` with stride `0x10`; every entry is
    /// freshly read, bits 19:0 are cleared, and bits 31:20 are preserved.
    ///
    /// This method deliberately does not expose the later software event and
    /// list initialization performed by the vendor function, and therefore
    /// does not claim that the complete controller or scheduler is running.
    pub fn clear_scheduler_table_low_bits(&mut self) {
        for entry in self.bluetooth.btdm_scheduler_table.entry_iter() {
            entry.modify(|_, writer| writer.state_low_20().cleared());
        }
        device_fence();
    }
}
