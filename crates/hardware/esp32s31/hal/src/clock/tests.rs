use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_pac::{WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::{PlatformClockError, PlatformClockProvider, PowerEntry};

use super::{
    ClockPort, CommonRadioPower, CommonRadioPowerError, PlatformClockRefs, PowerEpoch, RadioClient,
    WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    AcquirePll160m,
    ReleasePll160m,
    AcquireAnalogI2c,
    ReleaseAnalogI2c,
    RestorePower(WifiPowerBaseline),
    PowerSequence(PowerEntry),
}

type Log = Rc<RefCell<Vec<Operation>>>;

struct Port {
    power: WifiPowerBaseline,
    power_readback: Result<(), WifiPowerRestoreReadback>,
    /// Fail the sequence after it took the 160 MHz reference.
    power_sequence_failure: Option<crate::power::PowerError>,
    log: Log,
}

impl Port {
    fn new(log: &Log) -> Self {
        Self {
            power: WifiPowerBaseline::for_validation(false),
            power_readback: Ok(()),
            power_sequence_failure: None,
            log: log.clone(),
        }
    }
}

impl ClockPort for Port {
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.power
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback> {
        self.log
            .borrow_mut()
            .push(Operation::RestorePower(baseline));
        self.power_readback
    }
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &mut impl PlatformClockProvider,
        entry: PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        self.log.borrow_mut().push(Operation::PowerSequence(entry));
        refs.acquire_pll_f160m(platform).unwrap();
        if let Some(error) = self.power_sequence_failure {
            return Err(error);
        }
        refs.acquire_analog_i2c(platform).unwrap();
        Ok(())
    }
}

struct Platform {
    refuse_analog_i2c_release: bool,
    log: Log,
}

impl Platform {
    fn new(log: &Log) -> Self {
        Self {
            refuse_analog_i2c_release: false,
            log: log.clone(),
        }
    }
}

impl PlatformClockProvider for Platform {
    fn acquire_pll_f160m(&mut self) -> Result<(), PlatformClockError> {
        self.log.borrow_mut().push(Operation::AcquirePll160m);
        Ok(())
    }
    fn release_pll_f160m(&mut self) -> Result<(), PlatformClockError> {
        self.log.borrow_mut().push(Operation::ReleasePll160m);
        Ok(())
    }
    fn acquire_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError> {
        self.log.borrow_mut().push(Operation::AcquireAnalogI2c);
        Ok(())
    }
    fn release_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError> {
        if self.refuse_analog_i2c_release {
            return Err(PlatformClockError);
        }
        self.log.borrow_mut().push(Operation::ReleaseAnalogI2c);
        Ok(())
    }
}

fn setup() -> (Log, Port, Platform) {
    let log = Log::default();
    let port = Port::new(&log);
    let platform = Platform::new(&log);
    (log, port, platform)
}

#[test]
fn power_retry_preserves_the_original_cold_baseline_until_commit() {
    let original = WifiPowerBaseline::for_validation(false);
    let retry_observation = WifiPowerBaseline::for_validation(true);
    let (log, mut port, _) = setup();
    port.power = original;
    let mut epoch = PowerEpoch::default();

    epoch.prepare(&port);
    port.power = retry_observation;
    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(*log.borrow(), [Operation::RestorePower(original)]);

    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(
        log.borrow()[1..],
        [Operation::RestorePower(retry_observation)]
    );
}

#[test]
fn power_restore_failure_retains_the_baseline_for_retry() {
    let (log, mut port, _) = setup();
    let mut epoch = PowerEpoch::default();
    epoch.prepare(&port);

    port.power_readback = Err(WifiPowerRestoreReadback::ModemSourceClocks);
    assert_eq!(
        epoch.restore(&mut port),
        Err(WifiPowerRestoreCheckpoint::ModemSourceClocks)
    );
    port.power_readback = Ok(());
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(log.borrow().len(), 2);
}

#[test]
fn common_power_holds_one_platform_reference_of_each_shared_gate() {
    let cold = WifiPowerBaseline::for_validation(false);
    let (log, mut port, mut platform) = setup();
    port.power = cold;
    let mut power = CommonRadioPower::default();

    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Wifi),
        Ok(())
    );
    // A later protocol must not pulse the Wi-Fi resets or take references.
    port.power = WifiPowerBaseline::for_validation(true);
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Bluetooth),
        Ok(())
    );
    assert_eq!(
        *log.borrow(),
        [
            Operation::PowerSequence(PowerEntry::FirstSinceBoot),
            Operation::AcquirePll160m,
            Operation::AcquireAnalogI2c,
        ]
    );
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::AlreadyEntered)
    );

    log.borrow_mut().clear();
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Wifi),
        Ok(())
    );
    assert!(log.borrow().is_empty());
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Bluetooth),
        Ok(())
    );
    // The last client closes the analog-I2C gate, restores the baseline
    // captured before the first edge, then drops the 160 MHz source.
    assert_eq!(
        *log.borrow(),
        [
            Operation::ReleaseAnalogI2c,
            Operation::RestorePower(cold),
            Operation::ReleasePll160m,
        ]
    );
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::NotEntered)
    );
}

#[test]
fn a_failed_power_sequence_admits_no_client_and_retries_without_a_second_reference() {
    let (log, mut port, mut platform) = setup();
    let error = crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    };
    port.power_sequence_failure = Some(error);
    let mut power = CommonRadioPower::default();
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Ieee802154),
        Err(CommonRadioPowerError::Power(error))
    );
    assert!(!power.holds(RadioClient::Ieee802154));
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Ieee802154),
        Err(CommonRadioPowerError::NotEntered)
    );

    port.power_sequence_failure = None;
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Ieee802154),
        Ok(())
    );
    let acquisitions = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::AcquirePll160m)
        .count();
    assert_eq!(acquisitions, 1);
}

#[test]
fn a_failed_restore_keeps_the_last_client_for_retry() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Wifi),
        Ok(())
    );
    port.power_readback = Err(WifiPowerRestoreReadback::ModemSyscon);
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Wifi),
        Err(CommonRadioPowerError::Restore(
            WifiPowerRestoreCheckpoint::ModemSyscon
        ))
    );
    assert!(power.holds(RadioClient::Wifi));
    port.power_readback = Ok(());
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Wifi),
        Ok(())
    );
    let releases = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::ReleaseAnalogI2c)
        .count();
    assert_eq!(releases, 1);
}

#[test]
fn a_refused_release_keeps_the_last_client_for_retry() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Bluetooth),
        Ok(())
    );
    platform.refuse_analog_i2c_release = true;
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::PlatformClock(PlatformClockError))
    );
    assert!(power.holds(RadioClient::Bluetooth));
    assert!(
        !log.borrow()
            .iter()
            .any(|operation| matches!(operation, Operation::RestorePower(_)))
    );
    platform.refuse_analog_i2c_release = false;
    assert_eq!(
        power.exit(&mut port, &mut platform, RadioClient::Bluetooth),
        Ok(())
    );
}

#[test]
fn power_up_after_every_client_left_keeps_the_wifi_resets() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();

    for step in 0..3 {
        let result = if step == 1 {
            power.exit(&mut port, &mut platform, RadioClient::Ieee802154)
        } else {
            power.enter(&mut port, &mut platform, RadioClient::Ieee802154)
        };
        assert_eq!(result, Ok(()));
    }
    let sequences: Vec<_> = log
        .borrow()
        .iter()
        .filter_map(|operation| match operation {
            Operation::PowerSequence(entry) => Some(*entry),
            _ => None,
        })
        .collect();
    assert_eq!(
        sequences,
        [PowerEntry::FirstSinceBoot, PowerEntry::Repeated],
    );
}

#[test]
fn a_failed_first_power_up_still_pulses_the_wifi_resets_on_retry() {
    let (log, mut port, mut platform) = setup();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ResetReleased,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(
        power
            .enter(&mut port, &mut platform, RadioClient::Wifi)
            .is_err()
    );
    port.power_sequence_failure = None;
    assert_eq!(
        power.enter(&mut port, &mut platform, RadioClient::Wifi),
        Ok(())
    );
    let sequences: Vec<_> = log
        .borrow()
        .iter()
        .filter_map(|operation| match operation {
            Operation::PowerSequence(entry) => Some(*entry),
            _ => None,
        })
        .collect();
    assert_eq!(
        sequences,
        [PowerEntry::FirstSinceBoot, PowerEntry::FirstSinceBoot]
    );
}
