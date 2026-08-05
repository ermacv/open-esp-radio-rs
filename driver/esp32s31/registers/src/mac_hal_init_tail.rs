//! Generated-PAC ownership for the direct pre-COEX tail of `hal_init`.

use super::ColdRadioRegisters;

impl ColdRadioRegisters {
    /// Apply the complete direct tail through `hal_timer_update_by_rtc`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0xcc..0x12a`, the complete hardware-beacon reload leaf, and
    /// complete `libpp.a[hal_tsf.o]::hal_timer_update_by_rtc`.
    ///
    /// Returns false without touching hardware when `slow_clock_calibration`
    /// does not fit the exact eighteen-bit field consumed by the blob.
    pub fn initialize_mac_hal_tail(
        &mut self,
        event_mask: u32,
        slow_clock_calibration: u32,
    ) -> bool {
        if slow_clock_calibration > 0x0003_ffff {
            return false;
        }

        // SAFETY: complete hal_init stores the full constant image before any
        // remaining tail operation.
        unsafe {
            self.registers
                .peripherals
                .wifi_mac_interrupt
                .enable()
                .write_with_zero(|w| w.event_mask().bits(event_mask));
        }

        // This is deliberately a repeated edge: mac_txrx_init already set the
        // same bit, and complete hal_init samples and sets it again here.
        self.registers
            .peripherals
            .wifi_mac_txrx_prefix
            .feature_edges()
            .modify(|_, w| w.third_enable_unknown().set_bit());

        let csi = self.registers.peripherals.wifi_mac_rx_csi_control.control();
        // SAFETY: both values fit their complete eight-bit fields. Keep the
        // fresh-read byte replacements separate and in blob order.
        csi.modify(|_, w| unsafe { w.hal_init_low_byte_unknown().bits(1) });
        csi.modify(|_, w| unsafe { w.hal_init_second_byte_unknown().bits(1) });

        self.registers
            .peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.hardware_beacon_reload_unknown().set_bit());

        let rtc = &self.registers.peripherals.wifi_mac_rtc_timer_update;
        rtc.control()
            .modify(|_, w| w.rtc_update_enable_unknown().set_bit());
        // SAFETY: the explicit range check above proves the value fits the
        // generated eighteen-bit field without truncation.
        rtc.slow_clock_calibration()
            .modify(|_, w| unsafe { w.value().bits(slow_clock_calibration) });
        true
    }
}
