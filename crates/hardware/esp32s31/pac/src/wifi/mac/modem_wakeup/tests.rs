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
