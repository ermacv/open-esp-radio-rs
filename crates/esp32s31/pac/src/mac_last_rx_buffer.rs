//! Generated-PAC ownership for the complete last-RX-buffer table init.

use super::RadioRegisters;

impl RadioRegisters {
    /// Publish all six table entries, then their three enable edges.
    ///
    /// SOURCE: complete pinned
    /// `_oracles/libpp.a[hal_mac.o]::mac_last_rxbuf_init`, size `0xd2`.
    pub fn initialize_mac_last_rx_buffer(&mut self) {
        const CONTROL: [u32; 6] = [
            0x0002_3006,
            0x0002_3006,
            0x0002_3006,
            0x0002_301c,
            0x0002_301c,
            0x0002_3011,
        ];
        const PARAMETER_A: [u32; 6] = [
            0x0000_0608,
            0x0000_0808,
            0x0000_8e88,
            0x4400_4300,
            0x4300_4400,
            0x0000_0001,
        ];
        const PARAMETER_B: [u32; 6] = [
            0x0000_ffff,
            0x0000_ffff,
            0x0000_ffff,
            0xffff_ffff,
            0xffff_ffff,
            0x0000_00ff,
        ];

        let table = &self.peripherals.wifi_mac_last_rx_buffer;
        for entry in 0..6 {
            // SAFETY: these are the complete full-width images from the leaf;
            // all three generated registers are write-only table apertures.
            unsafe {
                table
                    .entry_control(entry)
                    .write_with_zero(|w| w.image_unknown().bits(CONTROL[entry]));
                table
                    .entry_parameter_a(entry)
                    .write_with_zero(|w| w.image_unknown().bits(PARAMETER_A[entry]));
                table
                    .entry_parameter_b(entry)
                    .write_with_zero(|w| w.image_unknown().bits(PARAMETER_B[entry]));
            }
        }

        // Preserve the two distinct fresh-read edges from the complete leaf.
        table
            .control()
            .modify(|_, w| unsafe { w.high_enable_group_unknown().bits(0x3f) });
        table
            .control()
            .modify(|_, w| unsafe { w.low_enable_group_unknown().bits(0x3f) });
        self.peripherals
            .wifi_mac_rx_csi_control
            .control()
            .modify(|_, w| w.last_rx_buffer_enable_unknown().set_bit());
    }
}
