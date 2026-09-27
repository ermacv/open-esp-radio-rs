//! Safe generated-PAC ownership for the station TSF transaction.

#![forbid(unsafe_code)]

use crate::{WifiRadioRegisters, device_fence, svd};

/// The station-TBTT wake gates are not in the reviewed idle image.
///
/// The complete vendor disable leaf leaves RTC CONTROL bit 21 asserted.
/// Entry therefore requires that exact idle image: synthesizing a clear
/// during rollback would not be evidence-backed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTbttWakeGateBaselineUnsupported;

/// Affine rollback token for the reviewed station-TBTT wake prefix.
#[must_use = "a prepared station TBTT wake prefix must be restored"]
pub struct StaTbttWakeRestore {
    previous_target_bits_35_10: u32,
    programmed_target_bits_35_10: u32,
}

impl StaTbttWakeRestore {
    /// Exact low 26-bit image published from `wake_tsf[35:10]`.
    pub const fn programmed_target_bits_35_10(&self) -> u32 {
        self.programmed_target_bits_35_10
    }
}

/// One station TBTT schedule, as the vendor power manager programs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTbttSchedule {
    /// Station TSF of the first TBTT.
    pub first_tbtt_tsf: u64,
    /// Interval between TBTT events, in microseconds.
    pub interval_micros: u32,
    /// How long before each TBTT the event fires, in microseconds.
    pub ahead_micros: u16,
    /// Light-sleep wake lead published beside the schedule, in microseconds.
    pub wake_ahead_micros: u16,
}

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

impl WifiRadioRegisters {
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
            &self.peripherals.wifi_mac.wifi_mac_sta_tsf_load,
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
        let load = &self.peripherals.wifi_mac.wifi_mac_sta_tsf_load;
        crate::generated::station_tsf_value_low(
            load,
            crate::generated::StationTsfLowWord::new(value as u32),
        );
        crate::generated::station_tsf_value_high(
            load,
            crate::generated::StationTsfHighWord::new((value >> 32) as u32),
        );
        load.control().modify(|_, w| w.load_station_tsf().set_bit());

        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .sta_tsf_control();
        control.modify(|_, w| {
            w.sta_tsf_enable_low()
                .set_bit()
                .sta_tsf_enable_high()
                .set_bit()
        });
        control.modify(|_, w| w.sta_tsf_mode().enabled());
        device_fence();
    }

    /// Program the station TBTT schedule and enable its interrupt.
    ///
    /// SOURCE: complete pinned `libpp.a[pm.o]::pm_update_next_tbtt` disables
    /// and re-enables the station TBTT, programs the schedule with
    /// `hal_set_sta_tbtt`, then clears the SoC wakeup request and sets bit
    /// zero at `0x2010_d810`. Complete `hal_tsf.o::hal_disable_sta_tbtt`
    /// clears TBTT enable bit 26 at `0x2010_d858`; `hal_enable_sta_tbtt`
    /// sets it; `hal_set_sta_tbtt` writes the interval, the target, the
    /// target load strobe, the ahead time and the light-sleep wake lead, in
    /// that order. The station TBTT interrupt enable in the same leaves
    /// belongs to the interrupt epoch, which keeps it enabled; bit 26 gates
    /// the event.
    pub fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        self.stop_station_tbtt();
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_enable().set_bit());

        self.set_station_tbtt_interval(schedule.interval_micros);
        let target_bits_35_10 = ((schedule.first_tbtt_tsf >> 10) as u32)
            & crate::generated::StationTbttTargetBits35To10::MAX;
        crate::generated::publish_station_tbtt_target(
            &self.peripherals.wifi_mac.wifi_mac_sta_tbtt_target,
            crate::generated::StationTbttTargetBits35To10::new(target_bits_35_10)
                .expect("masked station-TBTT target is a reviewed 26-bit value"),
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_sta_tsf_load
            .control()
            .modify(|_, w| w.load_station_tbtt_target().set_bit());
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_ahead_time().set(schedule.ahead_micros));
        rtc.sta_light_sleep_wake_ahead()
            .modify(|_, w| w.time().set(schedule.wake_ahead_micros));

        rtc.soc_wakeup_clear()
            .modify(|_, w| w.clear_request().set_bit());
        self.peripherals
            .wifi_mac
            .wifi_mac_tsf_status
            .wakeup_signal_clear()
            .modify(|_, w| w.clear().set_bit());
        device_fence();
    }

    /// Stop the station TBTT.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_tsf.o]::hal_disable_sta_tbtt`
    /// clears bit 26 at `0x2010_d858`.
    pub fn stop_station_tbtt(&mut self) {
        self.peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_enable().clear_bit());
        device_fence();
    }

    /// Replace the station TBTT lead and the light-sleep wake lead beside it.
    ///
    /// SOURCE: complete pinned `libpp.a[pm.o]::pm_update_params` calls
    /// `hal_set_sta_tbtt_ahead_time`, which replaces bits 15:0 at
    /// `0x2010_d858`, then `hal_set_sta_light_sleep_wake_ahead_time`, which
    /// replaces bits 31:16 at `0x2010_d840`.
    pub fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_ahead_time().set(ahead_micros));
        rtc.sta_light_sleep_wake_ahead()
            .modify(|_, w| w.time().set(wake_ahead_micros));
    }

    /// Replace the station TBTT interval, as the vendor does when the
    /// coexistence period changes.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_tsf.o]::
    /// hal_set_sta_tbtt_interval` replaces bits 25:0 at `0x2010_d85c` with
    /// the interval shifted right by ten.
    pub fn set_station_tbtt_interval(&mut self, interval_micros: u32) {
        self.peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .sta_tbtt_interval()
            .modify(|_, w| w.interval_tsf_bits_35_10().set(interval_micros >> 10));
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
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
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

    /// Publish the reviewed station-TBTT target and enable its dedicated wake
    /// signal, returning the only authority to undo the prefix.
    ///
    /// The target packing is the complete `hal_set_sta_tbtt` transaction:
    /// bits 25:0 receive station TSF bits 35:10 while bits 31:26 are
    /// preserved. Wake enable then follows the exact two-register
    /// `hal_set_sta_tsf_wakeup(true)` order. This does not bind a generic TSF
    /// timer to WDEVPWR or power down RF/PHY.
    pub fn prepare_station_tbtt_wake(
        &mut self,
        wake_tsf: u64,
    ) -> Result<StaTbttWakeRestore, StaTbttWakeGateBaselineUnsupported> {
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        if rtc
            .sta_tsf_control()
            .read()
            .sta_tsf_wakeup_enable()
            .bit_is_set()
            || rtc.control().read().sta_tsf_wakeup_enable().bit_is_clear()
        {
            return Err(StaTbttWakeGateBaselineUnsupported);
        }

        let target = &self.peripherals.wifi_mac.wifi_mac_sta_tbtt_target;
        let previous_target_bits_35_10 = target.target().read().tsf_bits_35_10().bits();
        let programmed_target_bits_35_10 =
            ((wake_tsf >> 10) as u32) & crate::generated::StationTbttTargetBits35To10::MAX;
        debug_assert!(
            (crate::generated::StationTbttTargetBits35To10::MIN
                ..=crate::generated::StationTbttTargetBits35To10::MAX)
                .contains(&programmed_target_bits_35_10)
        );
        crate::generated::publish_station_tbtt_target(
            target,
            crate::generated::StationTbttTargetBits35To10::new(programmed_target_bits_35_10)
                .expect("masked station-TBTT target is a reviewed 26-bit value"),
        );
        self.set_station_tsf_wakeup(true);
        device_fence();
        Ok(StaTbttWakeRestore {
            previous_target_bits_35_10,
            programmed_target_bits_35_10,
        })
    }

    /// Disable the station wake signal using the complete vendor disable
    /// image, then restore the exact target field retained by preparation.
    /// The baseline check guarantees exact gate restoration without an
    /// invented clear of RTC CONTROL bit 21.
    pub fn restore_station_tbtt_wake(&mut self, restore: StaTbttWakeRestore) {
        self.set_station_tsf_wakeup(false);
        crate::generated::publish_station_tbtt_target(
            &self.peripherals.wifi_mac.wifi_mac_sta_tbtt_target,
            crate::generated::StationTbttTargetBits35To10::new(restore.previous_target_bits_35_10)
                .expect("saved station-TBTT target is a reviewed 26-bit value"),
        );
        device_fence();
    }
}
