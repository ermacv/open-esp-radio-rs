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

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::super::RadioHardware;

    #[test]
    fn reviewed_table_geometry_is_sixteen_sparse_entries_in_order() {
        // Pointer inspection does not perform volatile MMIO.
        let cold = RadioHardware::for_validation().into_bluetooth();
        let (task, _interrupts) = cold.separate_interrupt_owner();
        let table = &task.bluetooth.btdm_scheduler_table;

        let addresses = table
            .entry_iter()
            .map(|entry| entry.as_ptr() as usize)
            .collect::<Vec<_>>();
        assert_eq!(addresses.len(), 16);
        assert_eq!(addresses[0], 0x2010_b000);
        assert_eq!(addresses[15], 0x2010_b0f0);
        assert!(addresses.windows(2).all(|pair| pair[1] - pair[0] == 0x10));
    }
}
