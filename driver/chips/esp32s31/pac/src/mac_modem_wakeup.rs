//! Safe generated-PAC transactions for connected STA modem wakeup state.

#![forbid(unsafe_code)]

use super::{WifiRadioRegisters, device_fence, svd};

/// Raw sixteen-bit image used by the modem beacon-miss timeout leaf.
///
/// Reviewed evidence proves the field width and RMW transaction, but not its
/// time unit. A dedicated type prevents association/executor microseconds from
/// being passed here without an explicit, later-qualified conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconMissTimeoutRaw(u16);

impl StaBeaconMissTimeoutRaw {
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

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

/// Raw sixteen-bit image used by the modem wake-protect lead-time leaf.
///
/// Its hardware unit is deliberately absent: the reviewed setter proves only
/// the exact field projection, not a conversion from TSF or executor time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaWakeProtectEarlyTimeRaw(u16);

impl StaWakeProtectEarlyTimeRaw {
    pub const fn new(raw: u16) -> Self {
        Self(raw)
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
    pub beacon_miss_timeout: StaBeaconMissTimeoutRaw,
    pub beacon_miss_limit: StaBeaconMissLimit,
    pub modem_sleep_limit: StaModemSleepLimit,
    pub wakeup_protect_early_time: StaWakeProtectEarlyTimeRaw,
    pub tbtt_auto_period: Option<StaTbttAutoPeriod>,
}

/// A second modem-wakeup transaction cannot overlap the first one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaModemWakePrepareError {
    AlreadyConfigured,
}

/// Failure to consume one modem-wakeup rollback obligation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaModemWakeRestoreError {
    NotConfigured,
}

/// Pure ownership state kept beside the unique PAC owner.
///
/// Forgetting a restore token intentionally leaves this state configured: no
/// `Drop` implementation may touch MMIO through a lost borrow. The radio owner
/// is then fail-closed (`AlreadyConfigured`) until an explicit restore or the
/// enclosing hardware-reset lifecycle replaces it.
pub(crate) struct StaModemWakeOwnership {
    configured: bool,
}

impl StaModemWakeOwnership {
    pub(crate) const fn new() -> Self {
        Self { configured: false }
    }

    fn acquire(&mut self) -> Result<(), StaModemWakePrepareError> {
        if self.configured {
            return Err(StaModemWakePrepareError::AlreadyConfigured);
        }
        self.configured = true;
        Ok(())
    }

    fn require_configured(&self) -> Result<(), StaModemWakeRestoreError> {
        if self.configured {
            Ok(())
        } else {
            Err(StaModemWakeRestoreError::NotConfigured)
        }
    }

    fn complete_restore(&mut self) {
        debug_assert!(self.configured);
        self.configured = false;
    }

    const fn configured(&self) -> bool {
        self.configured
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StaModemWakeSnapshot {
    beacon_miss_timeout: u16,
    beacon_miss_limit: u8,
    beacon_miss_limit_wakeup_enabled: bool,
    modem_sleep_limit: u16,
    modem_sleep_limit_wakeup_enabled: bool,
    wakeup_protect_enabled: bool,
    wakeup_protect_early_time: u16,
    tbtt_auto_period: u16,
    tbtt_auto_period_enabled: bool,
}

/// Affine obligation to restore every field changed by one configuration.
///
/// The token intentionally exposes no constructor and is neither `Copy` nor
/// `Clone`. Unrelated bits in the three shared registers are never captured
/// or restored, so channel REGDMA ownership is preserved. Dropping or
/// forgetting it performs no implicit MMIO and therefore poisons this PAC
/// owner against another configuration until the enclosing radio reset.
#[must_use = "a configured station modem-wakeup transaction must be restored"]
pub struct StaModemWakeRestore {
    previous: StaModemWakeSnapshot,
}

/// Failed rollback retaining the unique obligation token.
pub struct StaModemWakeRestoreFailure {
    pub error: StaModemWakeRestoreError,
    pub restore: StaModemWakeRestore,
}

pub(crate) fn set_beacon_miss_timeout(registers: &svd::WifiMacRtcTimerUpdate, value: u16) {
    registers
        .rx_beacon_time_low()
        .modify(|_, writer| writer.value().set(value));
}

pub(crate) fn set_beacon_miss_limit(registers: &svd::WifiMacRtcTimerUpdate, value: u8) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.beacon_miss_limit().set(value));
}

pub(crate) fn enable_beacon_miss_limit_wakeup(registers: &svd::WifiMacRtcTimerUpdate) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.beacon_miss_limit_wakeup_enable().set_bit());
}

pub(crate) fn set_beacon_miss_limit_wakeup(registers: &svd::WifiMacRtcTimerUpdate, enabled: bool) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.beacon_miss_limit_wakeup_enable().bit(enabled));
}

pub(crate) fn set_modem_state_sleep_limit(registers: &svd::WifiMacRtcTimerUpdate, value: u16) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.modem_state_sleep_limit().set(value));
}

pub(crate) fn enable_modem_state_sleep_limit_wakeup(registers: &svd::WifiMacRtcTimerUpdate) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.modem_state_sleep_limit_wakeup_enable().set_bit());
}

pub(crate) fn set_modem_state_sleep_limit_wakeup(
    registers: &svd::WifiMacRtcTimerUpdate,
    enabled: bool,
) {
    registers
        .modem_sleep_limit_control()
        .modify(|_, writer| writer.modem_state_sleep_limit_wakeup_enable().bit(enabled));
}

pub(crate) fn enable_modem_state_wakeup_protect(registers: &svd::WifiMacRtcTimerUpdate) {
    registers
        .sta_tsf_control()
        .modify(|_, writer| writer.modem_state_wakeup_protect_enable().set_bit());
}

pub(crate) fn set_modem_state_wakeup_protect(
    registers: &svd::WifiMacRtcTimerUpdate,
    enabled: bool,
) {
    registers
        .sta_tsf_control()
        .modify(|_, writer| writer.modem_state_wakeup_protect_enable().bit(enabled));
}

pub(crate) fn set_wakeup_protect_early_time(registers: &svd::WifiMacRegdmaControl, value: u16) {
    registers
        .control()
        .modify(|_, writer| writer.modem_wakeup_protect_early_time().set(value));
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

pub(crate) fn set_tbtt_auto_period(registers: &svd::WifiMacRegdmaControl, value: u16) {
    registers
        .control()
        .modify(|_, writer| writer.modem_tbtt_auto_period_interval().set(value));
}

/// Shared transaction shape used by production MMIO and the host register
/// image model. Keeping ordering here makes gate-before-value rollback a
/// tested property of the same source that drives hardware.
trait StaModemWakeTransaction {
    fn set_beacon_miss_timeout(&mut self, value: u16);
    fn set_beacon_miss_limit(&mut self, value: u8);
    fn set_beacon_miss_wakeup(&mut self, enabled: bool);
    fn set_modem_sleep_limit(&mut self, value: u16);
    fn set_modem_sleep_wakeup(&mut self, enabled: bool);
    fn set_wakeup_protect(&mut self, enabled: bool);
    fn set_wakeup_protect_early_time(&mut self, value: u16);
    fn set_tbtt_auto_period_enabled(&mut self, enabled: bool);
    fn set_tbtt_auto_period(&mut self, value: u16);
}

struct StaModemWakeMmio<'registers> {
    rtc: &'registers svd::WifiMacRtcTimerUpdate,
    regdma: &'registers svd::WifiMacRegdmaControl,
}

impl StaModemWakeTransaction for StaModemWakeMmio<'_> {
    fn set_beacon_miss_timeout(&mut self, value: u16) {
        set_beacon_miss_timeout(self.rtc, value);
    }

    fn set_beacon_miss_limit(&mut self, value: u8) {
        set_beacon_miss_limit(self.rtc, value);
    }

    fn set_beacon_miss_wakeup(&mut self, enabled: bool) {
        if enabled {
            enable_beacon_miss_limit_wakeup(self.rtc);
        } else {
            set_beacon_miss_limit_wakeup(self.rtc, false);
        }
    }

    fn set_modem_sleep_limit(&mut self, value: u16) {
        set_modem_state_sleep_limit(self.rtc, value);
    }

    fn set_modem_sleep_wakeup(&mut self, enabled: bool) {
        if enabled {
            enable_modem_state_sleep_limit_wakeup(self.rtc);
        } else {
            set_modem_state_sleep_limit_wakeup(self.rtc, false);
        }
    }

    fn set_wakeup_protect(&mut self, enabled: bool) {
        if enabled {
            enable_modem_state_wakeup_protect(self.rtc);
        } else {
            set_modem_state_wakeup_protect(self.rtc, false);
        }
    }

    fn set_wakeup_protect_early_time(&mut self, value: u16) {
        set_wakeup_protect_early_time(self.regdma, value);
    }

    fn set_tbtt_auto_period_enabled(&mut self, enabled: bool) {
        if enabled {
            enable_tbtt_auto_period(self.regdma);
        } else {
            disable_tbtt_auto_period(self.regdma);
        }
    }

    fn set_tbtt_auto_period(&mut self, value: u16) {
        set_tbtt_auto_period(self.regdma, value);
    }
}

fn apply_station_modem_wakeup_config(
    transaction: &mut impl StaModemWakeTransaction,
    config: StaModemWakeConfig,
) {
    transaction.set_beacon_miss_timeout(config.beacon_miss_timeout.get());
    transaction.set_beacon_miss_limit(config.beacon_miss_limit.get());
    transaction.set_beacon_miss_wakeup(true);
    transaction.set_modem_sleep_limit(config.modem_sleep_limit.get());
    transaction.set_modem_sleep_wakeup(true);
    transaction.set_wakeup_protect(true);
    transaction.set_wakeup_protect_early_time(config.wakeup_protect_early_time.get());
    match config.tbtt_auto_period {
        Some(period) => {
            // Preserve the vendor order: enable first, then interval.
            transaction.set_tbtt_auto_period_enabled(true);
            transaction.set_tbtt_auto_period(period.get());
        }
        None => transaction.set_tbtt_auto_period_enabled(false),
    }
}

fn apply_station_modem_wakeup_restore(
    transaction: &mut impl StaModemWakeTransaction,
    previous: StaModemWakeSnapshot,
) {
    // No counter/control value moves while one of the transaction's wake
    // gates remains asserted.
    transaction.set_beacon_miss_wakeup(false);
    transaction.set_modem_sleep_wakeup(false);
    transaction.set_wakeup_protect(false);
    transaction.set_tbtt_auto_period_enabled(false);

    transaction.set_beacon_miss_timeout(previous.beacon_miss_timeout);
    transaction.set_beacon_miss_limit(previous.beacon_miss_limit);
    transaction.set_modem_sleep_limit(previous.modem_sleep_limit);
    transaction.set_wakeup_protect_early_time(previous.wakeup_protect_early_time);
    transaction.set_tbtt_auto_period(previous.tbtt_auto_period);

    transaction.set_beacon_miss_wakeup(previous.beacon_miss_limit_wakeup_enabled);
    transaction.set_modem_sleep_wakeup(previous.modem_sleep_limit_wakeup_enabled);
    transaction.set_wakeup_protect(previous.wakeup_protect_enabled);
    transaction.set_tbtt_auto_period_enabled(previous.tbtt_auto_period_enabled);
}

impl WifiRadioRegisters {
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
            .wifi_mac
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
    pub fn configure_station_modem_wakeup(
        &mut self,
        config: StaModemWakeConfig,
    ) -> Result<StaModemWakeRestore, StaModemWakePrepareError> {
        self.station_modem_wakeup.acquire()?;
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        let limits = rtc.modem_sleep_limit_control().read();
        let previous = StaModemWakeSnapshot {
            beacon_miss_timeout: rtc.rx_beacon_time_low().read().value().bits(),
            beacon_miss_limit: limits.beacon_miss_limit().bits(),
            beacon_miss_limit_wakeup_enabled: limits.beacon_miss_limit_wakeup_enable().bit_is_set(),
            modem_sleep_limit: limits.modem_state_sleep_limit().bits(),
            modem_sleep_limit_wakeup_enabled: limits
                .modem_state_sleep_limit_wakeup_enable()
                .bit_is_set(),
            wakeup_protect_enabled: rtc
                .sta_tsf_control()
                .read()
                .modem_state_wakeup_protect_enable()
                .bit_is_set(),
            wakeup_protect_early_time: self
                .peripherals
                .wifi_mac
                .wifi_mac_regdma_control
                .control()
                .read()
                .modem_wakeup_protect_early_time()
                .bits(),
            tbtt_auto_period: self
                .peripherals
                .wifi_mac
                .wifi_mac_regdma_control
                .control()
                .read()
                .modem_tbtt_auto_period_interval()
                .bits(),
            tbtt_auto_period_enabled: self
                .peripherals
                .wifi_mac
                .wifi_mac_regdma_control
                .control()
                .read()
                .modem_tbtt_auto_period_enable()
                .bit_is_set(),
        };

        let regdma = &self.peripherals.wifi_mac.wifi_mac_regdma_control;
        apply_station_modem_wakeup_config(&mut StaModemWakeMmio { rtc, regdma }, config);
        device_fence();
        Ok(StaModemWakeRestore { previous })
    }

    /// Restore exactly the fields captured by
    /// [`Self::configure_station_modem_wakeup`].
    ///
    /// Wake gates are cleared before their values move and then restored to
    /// the captured booleans. This is a source-composed rollback while the
    /// radio is awake; it does not claim a qualified live-sleep reconfigure
    /// edge or infer any counter unit.
    pub fn restore_station_modem_wakeup(
        &mut self,
        restore: StaModemWakeRestore,
    ) -> Result<(), StaModemWakeRestoreFailure> {
        if let Err(error) = self.station_modem_wakeup.require_configured() {
            return Err(StaModemWakeRestoreFailure { error, restore });
        }

        let previous = restore.previous;
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        let regdma = &self.peripherals.wifi_mac.wifi_mac_regdma_control;
        apply_station_modem_wakeup_restore(&mut StaModemWakeMmio { rtc, regdma }, previous);
        self.station_modem_wakeup.complete_restore();
        device_fence();
        Ok(())
    }

    /// Whether an affine modem-wakeup rollback is still outstanding.
    ///
    /// This value-only diagnostic grants no recovery authority. `true` after
    /// a forgotten token means the PAC owner is intentionally quarantined.
    pub const fn station_modem_wakeup_restore_pending(&self) -> bool {
        self.station_modem_wakeup.configured()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        BeaconMissTimeout(u16),
        BeaconMissLimit(u8),
        BeaconMissWake(bool),
        ModemSleepLimit(u16),
        ModemSleepWake(bool),
        WakeupProtect(bool),
        WakeupProtectEarly(u16),
        TbttAutoEnabled(bool),
        TbttAutoPeriod(u16),
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RegisterModel {
        beacon_miss_timeout: u16,
        beacon_miss_limit: u8,
        beacon_miss_limit_wakeup_enabled: bool,
        modem_sleep_limit: u16,
        modem_sleep_limit_wakeup_enabled: bool,
        wakeup_protect_enabled: bool,
        wakeup_protect_early_time: u16,
        tbtt_auto_period: u16,
        tbtt_auto_period_enabled: bool,
        operations: Vec<Operation>,
    }

    impl RegisterModel {
        fn snapshot(&self) -> StaModemWakeSnapshot {
            StaModemWakeSnapshot {
                beacon_miss_timeout: self.beacon_miss_timeout,
                beacon_miss_limit: self.beacon_miss_limit,
                beacon_miss_limit_wakeup_enabled: self.beacon_miss_limit_wakeup_enabled,
                modem_sleep_limit: self.modem_sleep_limit,
                modem_sleep_limit_wakeup_enabled: self.modem_sleep_limit_wakeup_enabled,
                wakeup_protect_enabled: self.wakeup_protect_enabled,
                wakeup_protect_early_time: self.wakeup_protect_early_time,
                tbtt_auto_period: self.tbtt_auto_period,
                tbtt_auto_period_enabled: self.tbtt_auto_period_enabled,
            }
        }
    }

    impl StaModemWakeTransaction for RegisterModel {
        fn set_beacon_miss_timeout(&mut self, value: u16) {
            self.operations.push(Operation::BeaconMissTimeout(value));
            self.beacon_miss_timeout = value;
        }

        fn set_beacon_miss_limit(&mut self, value: u8) {
            self.operations.push(Operation::BeaconMissLimit(value));
            self.beacon_miss_limit = value;
        }

        fn set_beacon_miss_wakeup(&mut self, enabled: bool) {
            self.operations.push(Operation::BeaconMissWake(enabled));
            self.beacon_miss_limit_wakeup_enabled = enabled;
        }

        fn set_modem_sleep_limit(&mut self, value: u16) {
            self.operations.push(Operation::ModemSleepLimit(value));
            self.modem_sleep_limit = value;
        }

        fn set_modem_sleep_wakeup(&mut self, enabled: bool) {
            self.operations.push(Operation::ModemSleepWake(enabled));
            self.modem_sleep_limit_wakeup_enabled = enabled;
        }

        fn set_wakeup_protect(&mut self, enabled: bool) {
            self.operations.push(Operation::WakeupProtect(enabled));
            self.wakeup_protect_enabled = enabled;
        }

        fn set_wakeup_protect_early_time(&mut self, value: u16) {
            self.operations.push(Operation::WakeupProtectEarly(value));
            self.wakeup_protect_early_time = value;
        }

        fn set_tbtt_auto_period_enabled(&mut self, enabled: bool) {
            self.operations.push(Operation::TbttAutoEnabled(enabled));
            self.tbtt_auto_period_enabled = enabled;
        }

        fn set_tbtt_auto_period(&mut self, value: u16) {
            self.operations.push(Operation::TbttAutoPeriod(value));
            self.tbtt_auto_period = value;
        }
    }

    fn config() -> StaModemWakeConfig {
        StaModemWakeConfig {
            beacon_miss_timeout: StaBeaconMissTimeoutRaw::new(0x1234),
            beacon_miss_limit: StaBeaconMissLimit::new(10).unwrap(),
            modem_sleep_limit: StaModemSleepLimit::new(511).unwrap(),
            wakeup_protect_early_time: StaWakeProtectEarlyTimeRaw::new(0x5678),
            tbtt_auto_period: Some(StaTbttAutoPeriod::new(100).unwrap()),
        }
    }

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
        let config = config();
        assert_eq!(config.beacon_miss_timeout.get(), 0x1234);
        assert_eq!(config.beacon_miss_limit.get(), 10);
        assert_eq!(config.modem_sleep_limit.get(), 511);
        assert_eq!(config.wakeup_protect_early_time.get(), 0x5678);
        assert_eq!(config.tbtt_auto_period.unwrap().get(), 100);
    }

    #[test]
    fn raw_time_fields_do_not_claim_microsecond_units() {
        assert_eq!(StaBeaconMissTimeoutRaw::new(u16::MAX).get(), u16::MAX);
        assert_eq!(StaWakeProtectEarlyTimeRaw::new(u16::MAX).get(), u16::MAX);
    }

    #[test]
    fn configure_restore_is_field_exact_and_gate_ordered() {
        let initial = RegisterModel {
            beacon_miss_timeout: 0x1357,
            beacon_miss_limit: 0x0b,
            beacon_miss_limit_wakeup_enabled: true,
            modem_sleep_limit: 0x155,
            modem_sleep_limit_wakeup_enabled: true,
            wakeup_protect_enabled: true,
            wakeup_protect_early_time: 0x2468,
            tbtt_auto_period: 0x155,
            tbtt_auto_period_enabled: true,
            operations: Vec::new(),
        };
        let previous = initial.snapshot();
        let mut model = initial.clone();
        apply_station_modem_wakeup_config(&mut model, config());

        assert_eq!(model.beacon_miss_timeout, 0x1234);
        assert_eq!(model.beacon_miss_limit, 10);
        assert!(model.beacon_miss_limit_wakeup_enabled);
        assert_eq!(model.modem_sleep_limit, 511);
        assert!(model.modem_sleep_limit_wakeup_enabled);
        assert!(model.wakeup_protect_enabled);
        assert_eq!(model.wakeup_protect_early_time, 0x5678);
        assert_eq!(model.tbtt_auto_period, 100);
        assert!(model.tbtt_auto_period_enabled);
        assert_eq!(
            model.operations,
            [
                Operation::BeaconMissTimeout(0x1234),
                Operation::BeaconMissLimit(10),
                Operation::BeaconMissWake(true),
                Operation::ModemSleepLimit(511),
                Operation::ModemSleepWake(true),
                Operation::WakeupProtect(true),
                Operation::WakeupProtectEarly(0x5678),
                Operation::TbttAutoEnabled(true),
                Operation::TbttAutoPeriod(100),
            ]
        );

        model.operations.clear();
        apply_station_modem_wakeup_restore(&mut model, previous);
        assert_eq!(
            model.operations,
            [
                Operation::BeaconMissWake(false),
                Operation::ModemSleepWake(false),
                Operation::WakeupProtect(false),
                Operation::TbttAutoEnabled(false),
                Operation::BeaconMissTimeout(0x1357),
                Operation::BeaconMissLimit(0x0b),
                Operation::ModemSleepLimit(0x155),
                Operation::WakeupProtectEarly(0x2468),
                Operation::TbttAutoPeriod(0x155),
                Operation::BeaconMissWake(true),
                Operation::ModemSleepWake(true),
                Operation::WakeupProtect(true),
                Operation::TbttAutoEnabled(true),
            ]
        );
        model.operations.clear();
        assert_eq!(model, initial);
    }

    #[test]
    fn forgotten_restore_token_permanently_poisons_the_owner() {
        let mut ownership = StaModemWakeOwnership::new();
        ownership.acquire().unwrap();
        let restore = StaModemWakeRestore {
            previous: StaModemWakeSnapshot {
                beacon_miss_timeout: 0,
                beacon_miss_limit: 0,
                beacon_miss_limit_wakeup_enabled: false,
                modem_sleep_limit: 0,
                modem_sleep_limit_wakeup_enabled: false,
                wakeup_protect_enabled: false,
                wakeup_protect_early_time: 0,
                tbtt_auto_period: 0,
                tbtt_auto_period_enabled: false,
            },
        };
        drop(restore);

        assert!(ownership.configured());
        assert_eq!(
            ownership.acquire(),
            Err(StaModemWakePrepareError::AlreadyConfigured)
        );
    }
}
