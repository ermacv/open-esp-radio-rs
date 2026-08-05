//! Safe generated-PAC ownership for the station TSF transaction.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence, svd, svd::full_register_write};

/// Snapshot either or both station TSF words using the complete ROM leaf's
/// conditional-output semantics.
#[inline(always)]
pub(crate) fn snapshot_station_tsf(
    registers: &svd::WifiMacStaTsfLoad,
    low: Option<&mut u32>,
    high: Option<&mut u32>,
) {
    registers
        .control()
        .modify(|_, writer| writer.snapshot_station_tsf().set_bit());
    if let Some(low) = low {
        *low = registers.snapshot_low().read().value().bits();
    }
    if let Some(high) = high {
        *high = registers.snapshot_high().read().value().bits();
    }
    registers
        .control()
        .modify(|_, writer| writer.snapshot_station_tsf().clear_bit());
}

impl RadioRegisters {
    /// Return one coherent station TSF snapshot.
    ///
    /// SOURCE: complete rev0 ROM `hal_get_sta_tsf` at `0x2f82c15c` sets
    /// CONTROL bit zero, reads the low word at `0x2010_d820`, reads the high
    /// word at `0x2010_d824`, then clears CONTROL bit zero. Both output
    /// pointers are present in this production specialization.
    pub fn station_tsf(&mut self) -> u64 {
        let mut low = 0;
        let mut high = 0;
        snapshot_station_tsf(
            &self.peripherals.wifi_mac_sta_tsf_load,
            Some(&mut low),
            Some(&mut high),
        );
        u64::from(low) | (u64::from(high) << 32)
    }

    /// Publish a station TSF value and enable the station TSF scheduler.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]`: `hal_set_sta_tsf`
    /// writes the low word, writes the high word and then asserts bit four at
    /// `0x2010_d814` through a fresh-read RMW. Complete
    /// `hal_enable_sta_tsf` performs two further fresh-read RMWs at
    /// `0x2010_d858`: first it sets bits 27 and 31, then it replaces bits
    /// 22:19 with one.
    pub fn start_station_tsf(&mut self, value: u64) {
        let load = &self.peripherals.wifi_mac_sta_tsf_load;
        full_register_write::station_tsf_value_low(load, value as u32);
        full_register_write::station_tsf_value_high(load, (value >> 32) as u32);
        load.control().modify(|_, w| w.load_station_tsf().set_bit());

        let control = self.peripherals.wifi_mac_rtc_timer_update.sta_tsf_control();
        control.modify(|_, w| {
            w.sta_tsf_enable_low()
                .set_bit()
                .sta_tsf_enable_high()
                .set_bit()
        });
        control.modify(|_, w| w.sta_tsf_mode().enabled());
        device_fence();
    }

    /// Enable or disable the station TSF wake signal as one exact two-word
    /// transaction.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::
    /// hal_set_sta_tsf_wakeup`, size `0x32`. Both branches update bit 29 at
    /// `0x2010_d858` first and then set bit 21 at `0x2010_d830`. The second
    /// bit remains set in the vendor disable branch; this non-symmetric image
    /// is preserved rather than replaced with an intuitive guess.
    pub fn set_station_tsf_wakeup(&mut self, enabled: bool) {
        let rtc = &self.peripherals.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control().modify(|_, w| {
            if enabled {
                w.sta_tsf_wakeup_enable().set_bit()
            } else {
                w.sta_tsf_wakeup_enable().clear_bit()
            }
        });
        rtc.control()
            .modify(|_, w| w.sta_tsf_wakeup_enable().set_bit());
    }

    /// Return the complete shared STA TSF control image for HIL comparison.
    pub fn sta_tsf_control_image(&self) -> u32 {
        self.peripherals
            .wifi_mac_rtc_timer_update
            .sta_tsf_control()
            .read()
            .bits()
    }
}
