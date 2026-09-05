//! Safe generated-PAC ownership for the station TSF transaction.

#![forbid(unsafe_code)]

use crate::{WifiRadioRegisters, device_fence, svd};

/// Failure to start the reviewed station-TBTT wake prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTbttWakePrepareError {
    AlreadyPrepared,
    /// The complete vendor disable leaf leaves RTC CONTROL bit 21 asserted.
    /// Entry therefore requires that exact idle image: synthesizing a clear
    /// during rollback would not be evidence-backed.
    WakeGateBaselineUnsupported,
}

/// Failure to consume one station-TBTT rollback obligation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTbttWakeRestoreError {
    NotPrepared,
}

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

/// Failed rollback retaining the unique obligation token.
pub struct StaTbttWakeRestoreFailure {
    pub error: StaTbttWakeRestoreError,
    pub restore: StaTbttWakeRestore,
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
    ) -> Result<StaTbttWakeRestore, StaTbttWakePrepareError> {
        if self.station_tbtt_wake_prepared {
            return Err(StaTbttWakePrepareError::AlreadyPrepared);
        }
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        if rtc
            .sta_tsf_control()
            .read()
            .sta_tsf_wakeup_enable()
            .bit_is_set()
            || rtc.control().read().sta_tsf_wakeup_enable().bit_is_clear()
        {
            return Err(StaTbttWakePrepareError::WakeGateBaselineUnsupported);
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
        self.station_tbtt_wake_prepared = true;
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
    pub fn restore_station_tbtt_wake(
        &mut self,
        restore: StaTbttWakeRestore,
    ) -> Result<(), StaTbttWakeRestoreFailure> {
        if !self.station_tbtt_wake_prepared {
            return Err(StaTbttWakeRestoreFailure {
                error: StaTbttWakeRestoreError::NotPrepared,
                restore,
            });
        }
        self.set_station_tsf_wakeup(false);
        crate::generated::publish_station_tbtt_target(
            &self.peripherals.wifi_mac.wifi_mac_sta_tbtt_target,
            crate::generated::StationTbttTargetBits35To10::new(restore.previous_target_bits_35_10)
                .expect("saved station-TBTT target is a reviewed 26-bit value"),
        );
        self.station_tbtt_wake_prepared = false;
        device_fence();
        Ok(())
    }
}
