//! Safe generated-PAC transactions for connected STA modem wakeup state.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence, svd};

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

pub(crate) fn set_beacon_miss_timeout(registers: &svd::WifiMacRtcTimerUpdate, value: u16) -> u32 {
    registers
        .rx_beacon_time_low()
        .modify(|_, writer| writer.value().set(value));
    u32::from(value)
}

pub(crate) fn set_beacon_miss_limit(registers: &svd::WifiMacRtcTimerUpdate, value: u8) -> u32 {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.beacon_miss_limit().set(value));
    u32::from(value)
}

pub(crate) fn enable_beacon_miss_limit_wakeup(registers: &svd::WifiMacRtcTimerUpdate) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.beacon_miss_limit_wakeup_enable().set_bit());
}

pub(crate) fn set_modem_state_sleep_limit(
    registers: &svd::WifiMacRtcTimerUpdate,
    value: u16,
) -> u32 {
    let mut published = 0;
    registers
        .modem_sleep_limit_control()
        .modify(|reader, writer| {
            published = (reader.bits() & !0x0000_7fe0) | (u32::from(value) << 5);
            writer.modem_state_sleep_limit().set(value)
        });
    published
}

pub(crate) fn enable_modem_state_sleep_limit_wakeup(registers: &svd::WifiMacRtcTimerUpdate) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.modem_state_sleep_limit_wakeup_enable().set_bit());
}

pub(crate) fn enable_modem_state_wakeup_protect(registers: &svd::WifiMacRtcTimerUpdate) {
    registers
        .sta_tsf_control()
        .modify(|_, writer| writer.modem_state_wakeup_protect_enable().set_bit());
}

pub(crate) fn set_wakeup_protect_early_time(
    registers: &svd::WifiMacRegdmaControl,
    value: u16,
) -> u32 {
    registers
        .control()
        .modify(|_, writer| writer.modem_wakeup_protect_early_time().set(value));
    u32::from(value)
}

pub(crate) fn enable_tbtt_auto_period(registers: &svd::WifiMacRegdmaControl) {
    registers
        .control()
        .modify(|_, writer| writer.modem_tbtt_auto_period_enable().set_bit());
}

pub(crate) fn disable_tbtt_auto_period(registers: &svd::WifiMacRegdmaControl) {
    registers
        .control()
        .modify(|_, writer| writer.modem_tbtt_auto_period_enable().clear_bit());
}

pub(crate) fn set_tbtt_auto_period(registers: &svd::WifiMacRegdmaControl, value: u16) -> u32 {
    let mut published = 0;
    registers.control().modify(|reader, writer| {
        published = (reader.bits() & !0x7fe0_0000) | (u32::from(value) << 21);
        writer.modem_tbtt_auto_period_interval().set(value)
    });
    published
}

impl RadioRegisters {
    /// Disable modem-state wakeup protection for an always-awake STA.
    ///
    /// SOURCE: complete
    /// `libpp.a[hal_pwr.o]::
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
            .modify(|_, writer| writer.modem_state_wakeup_protect_enable().clear_bit());
        device_fence();
    }

    /// Apply the finite modem counter/wakeup subset of the vendor connected
    /// power configuration.
    ///
    /// SOURCE: complete `libpp.a[pm.o]::
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
        set_beacon_miss_timeout(rtc, config.beacon_miss_timeout);
        set_beacon_miss_limit(rtc, config.beacon_miss_limit.get());
        enable_beacon_miss_limit_wakeup(rtc);
        set_modem_state_sleep_limit(rtc, config.modem_sleep_limit.get());
        enable_modem_state_sleep_limit_wakeup(rtc);
        enable_modem_state_wakeup_protect(rtc);

        let regdma = &self.peripherals.wifi_mac_regdma_control;
        set_wakeup_protect_early_time(regdma, config.wakeup_protect_early_time);
        match config.tbtt_auto_period {
            Some(period) => {
                // Preserve the vendor order: enable first, then interval.
                enable_tbtt_auto_period(regdma);
                set_tbtt_auto_period(regdma, period.get());
            }
            None => disable_tbtt_auto_period(regdma),
        }
        device_fence();
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
