//! Generated-PAC ownership for the STA TSF and modem wakeup control.

use super::{RadioRegisters, device_fence, generated};

/// Four-bit hardware limit used by the modem beacon-miss counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconMissLimit(u8);

impl StaBeaconMissLimit {
    pub const MAX: u8 = 0x0f;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Ten-bit hardware limit used by the modem-state sleep counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaModemSleepLimit(u16);

impl StaModemSleepLimit {
    pub const MAX: u16 = 0x03ff;

    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Ten-bit period used by the hardware TBTT auto-period gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTbttAutoPeriod(u16);

impl StaTbttAutoPeriod {
    pub const MAX: u16 = 0x03ff;

    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Finite hardware-only subset of the vendor connected modem-wakeup context.
///
/// The large vendor `g_pm_cfg` remains deliberately absent. Association and
/// Embassy policy produce this typed value, and the PAC consumes it once.
/// Optional beacon filtering, RF clock ownership, sleep timers and RTOS state
/// are separate responsibilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaModemWakeConfig {
    pub beacon_miss_timeout: u16,
    pub beacon_miss_limit: StaBeaconMissLimit,
    pub modem_sleep_limit: StaModemSleepLimit,
    pub wakeup_protect_early_time: u16,
    pub tbtt_auto_period: Option<StaTbttAutoPeriod>,
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
        generated::hal_get_sta_tsf::generated_hal_get_sta_tsf(
            &self.peripherals.wifi_mac_sta_tsf_load,
            Some(&mut low),
            Some(&mut high),
        );
        u64::from(low) | (u64::from(high) << 32)
    }

    /// Publish a station TSF value and enable the station TSF scheduler.
    ///
    /// SOURCE: complete `_oracles/libpp.a[hal_tsf.o]`: `hal_set_sta_tsf`
    /// writes the low word, writes the high word and then asserts bit four at
    /// `0x2010_d814` through a fresh-read RMW. Complete
    /// `hal_enable_sta_tsf` performs two further fresh-read RMWs at
    /// `0x2010_d858`: first it sets bits 27 and 31, then it replaces bits
    /// 22:19 with one.
    pub fn start_station_tsf(&mut self, value: u64) {
        let load = &self.peripherals.wifi_mac_sta_tsf_load;
        // SAFETY: each `u32` exactly fills the generated 32-bit VALUE field.
        unsafe {
            load.value_low()
                .write_with_zero(|w| w.value().bits(value as u32));
            load.value_high()
                .write_with_zero(|w| w.value().bits((value >> 32) as u32));
        }
        load.control().modify(|_, w| w.load_station_tsf().set_bit());

        let control = self.peripherals.wifi_mac_rtc_timer_update.sta_tsf_control();
        control.modify(|_, w| {
            w.sta_tsf_enable_low()
                .set_bit()
                .sta_tsf_enable_high()
                .set_bit()
        });
        // SAFETY: one is the instruction-exact mode selected by
        // `hal_enable_sta_tsf` and fits the generated four-bit field.
        control.modify(|_, w| unsafe { w.sta_tsf_mode().bits(1) });
        device_fence();
    }

    /// Disable modem-state wakeup protection for an always-awake STA.
    ///
    /// SOURCE: complete
    /// `_oracles/libpp.a[hal_pwr.o]::
    /// pwr_hal_set_mac_modem_state_wakeup_protect_disable`.
    ///
    /// That leaf performs one fresh-read RMW which clears bit 24 at
    /// `0x2010_d858`. The vendor pre-auth path has this bit clear after its
    /// power-management lifecycle. A standalone HIL A/B showed that copying
    /// this final bit image without that lifecycle does not recover q0 ACKs,
    /// so callers must not use it as a generic TX-enable operation.
    pub fn disable_mac_modem_state_wakeup_protect(&mut self) {
        self.peripherals
            .wifi_mac_rtc_timer_update
            .sta_tsf_control()
            .modify(|_, w| w.modem_state_wakeup_protect_enable().clear_bit());
        device_fence();
    }

    /// Apply the finite modem counter/wakeup subset of the vendor connected
    /// power configuration.
    ///
    /// SOURCE: complete `_oracles/libpp.a[pm.o]::
    /// pm_mac_enable_tsf_tbtt_modem_wakeup` selects, in this relative order,
    /// the complete no-call `hal_pwr.o` leaves for beacon timeout, beacon
    /// limit, beacon-limit wake, modem-state sleep limit, sleep-limit wake,
    /// wakeup protection, wakeup lead time and optional TBTT auto period.
    ///
    /// This method does not claim that the modem is asleep. It neither
    /// disables RF/PHY nor changes platform clocks and it does not own the
    /// WDEVPWR interrupt bank. Those edges must be qualified and scheduled by
    /// their separate owners before HIL enables this transaction.
    pub fn configure_station_modem_wakeup(&mut self, config: StaModemWakeConfig) {
        let rtc = &self.peripherals.wifi_mac_rtc_timer_update;

        // pwr_hal_set_mac_modem_beacon_miss_timeout
        generated::pwr_hal_set_mac_modem_beacon_miss_timeout::generated_pwr_hal_set_mac_modem_beacon_miss_timeout(
            rtc,
            u32::from(config.beacon_miss_timeout),
        );
        // pwr_hal_set_mac_modem_beacon_miss_limit
        generated::pwr_hal_set_mac_modem_beacon_miss_limit::generated_pwr_hal_set_mac_modem_beacon_miss_limit(
            rtc,
            u32::from(config.beacon_miss_limit.get()),
        );
        // pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable
        generated::pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable::generated_pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(
            rtc,
            0,
        );
        // pwr_hal_set_mac_modem_state_sleep_limit
        generated::pwr_hal_set_mac_modem_state_sleep_limit::generated_pwr_hal_set_mac_modem_state_sleep_limit(
            rtc,
            u32::from(config.modem_sleep_limit.get()),
        );
        // pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable
        generated::pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable::generated_pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable(
            rtc,
            0,
        );

        // pwr_hal_set_mac_modem_state_wakeup_protect_enable
        generated::pwr_hal_set_mac_modem_state_wakeup_protect_enable::generated_pwr_hal_set_mac_modem_state_wakeup_protect_enable(
            rtc,
            0,
        );

        // pwr_hal_set_mac_modem_state_wakeup_protect_early_time
        generated::pwr_hal_set_mac_modem_state_wakeup_protect_early_time::generated_pwr_hal_set_mac_modem_state_wakeup_protect_early_time(
            &self.peripherals.wifi_mac_regdma_control,
            u32::from(config.wakeup_protect_early_time),
        );
        match config.tbtt_auto_period {
            Some(period) => {
                // Preserve the vendor order: enable first, then interval.
                generated::pwr_hal_set_mac_modem_tbtt_auto_period_enable::generated_pwr_hal_set_mac_modem_tbtt_auto_period_enable(
                    &self.peripherals.wifi_mac_regdma_control,
                    0,
                );
                generated::pwr_hal_set_mac_modem_tbtt_auto_period_interval::generated_pwr_hal_set_mac_modem_tbtt_auto_period_interval(
                    &self.peripherals.wifi_mac_regdma_control,
                    u32::from(period.get()),
                );
            }
            None => {
                generated::pwr_hal_set_mac_modem_tbtt_auto_period_disable::generated_pwr_hal_set_mac_modem_tbtt_auto_period_disable(
                    &self.peripherals.wifi_mac_regdma_control,
                    0,
                );
            }
        }
        device_fence();
    }

    /// Enable or disable the station TSF wake signal as one exact two-word
    /// transaction.
    ///
    /// SOURCE: complete `_oracles/libpp.a[hal_tsf.o]::
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_power_fields_reject_silent_truncation() {
        assert_eq!(StaBeaconMissLimit::new(15).unwrap().get(), 15);
        assert_eq!(StaBeaconMissLimit::new(16), None);
        assert_eq!(StaModemSleepLimit::new(1023).unwrap().get(), 1023);
        assert_eq!(StaModemSleepLimit::new(1024), None);
        assert_eq!(StaTbttAutoPeriod::new(1023).unwrap().get(), 1023);
        assert_eq!(StaTbttAutoPeriod::new(1024), None);
    }

    #[test]
    fn config_is_owned_data_without_vendor_context_layout() {
        let config = StaModemWakeConfig {
            beacon_miss_timeout: 0x1234,
            beacon_miss_limit: StaBeaconMissLimit::new(10).unwrap(),
            modem_sleep_limit: StaModemSleepLimit::new(511).unwrap(),
            wakeup_protect_early_time: 0x5678,
            tbtt_auto_period: Some(StaTbttAutoPeriod::new(100).unwrap()),
        };
        assert_eq!(config.beacon_miss_limit.get(), 10);
        assert_eq!(config.modem_sleep_limit.get(), 511);
        assert_eq!(config.tbtt_auto_period.unwrap().get(), 100);
    }
}
