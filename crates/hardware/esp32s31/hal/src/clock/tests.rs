use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_pac::{WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::test_clocks::{ClockEvent, CountingPlatformClocks, held, holding};
use crate::power::{PlatformClock, PlatformClockProvider, PowerEntry};

use super::{
    ClockPort, CommonRadioPower, CommonRadioPowerError, PlatformClockRefs, PowerEpoch, RadioClient,
    WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Acquire(PlatformClock),
    Release(PlatformClock),
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
        platform: &impl PlatformClockProvider,
        entry: PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        self.log.borrow_mut().push(Operation::PowerSequence(entry));
        refs.acquire(PlatformClock::Mpll, platform).unwrap();
        refs.acquire(PlatformClock::Pll160m, platform).unwrap();
        if let Some(error) = self.power_sequence_failure {
            return Err(error);
        }
        refs.acquire(PlatformClock::AnalogI2cMaster, platform)
            .unwrap();
        Ok(())
    }
}

fn setup() -> (Log, Port, CountingPlatformClocks) {
    let log = Log::default();
    let port = Port::new(&log);
    let sink = log.clone();
    let platform = CountingPlatformClocks::logging(move |event| {
        sink.borrow_mut().push(match event {
            ClockEvent::Acquire(clock) => Operation::Acquire(clock),
            ClockEvent::Release(clock) => Operation::Release(clock),
        });
    });
    (log, port, platform)
}

const COMMON: [PlatformClock; 3] = [
    PlatformClock::Mpll,
    PlatformClock::Pll160m,
    PlatformClock::AnalogI2cMaster,
];

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
    let (log, mut port, platform) = setup();
    port.power = cold;
    let mut power = CommonRadioPower::default();

    assert_eq!(power.enter(&mut port, &platform, RadioClient::Wifi), Ok(()));
    // A later protocol must not pulse the Wi-Fi resets or take references.
    port.power = WifiPowerBaseline::for_validation(true);
    assert_eq!(
        power.enter(&mut port, &platform, RadioClient::Bluetooth),
        Ok(())
    );
    assert_eq!(
        *log.borrow(),
        [
            Operation::PowerSequence(PowerEntry::FirstSinceBoot),
            Operation::Acquire(PlatformClock::Mpll),
            Operation::Acquire(PlatformClock::Pll160m),
            Operation::Acquire(PlatformClock::AnalogI2cMaster),
        ]
    );
    assert_eq!(
        power.enter(&mut port, &platform, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::AlreadyEntered)
    );
    assert_eq!(held(), holding(&COMMON));
    let mut counted = crate::power::PlatformClockHolds::default();
    power.count_platform_clocks(&mut counted);
    assert_eq!(counted, held());

    log.borrow_mut().clear();
    assert_eq!(power.exit(&mut port, RadioClient::Wifi), Ok(()));
    assert!(log.borrow().is_empty());
    assert_eq!(power.exit(&mut port, RadioClient::Bluetooth), Ok(()));
    // The last client closes the analog-I2C gate, restores the baseline
    // captured before the first edge, then drops the 160 MHz source and MPLL.
    assert_eq!(
        *log.borrow(),
        [
            Operation::Release(PlatformClock::AnalogI2cMaster),
            Operation::RestorePower(cold),
            Operation::Release(PlatformClock::Pll160m),
            Operation::Release(PlatformClock::Mpll),
        ]
    );
    assert!(held().is_empty());
    assert_eq!(
        power.exit(&mut port, RadioClient::Bluetooth),
        Err(CommonRadioPowerError::NotEntered)
    );
}

#[test]
fn a_failed_power_sequence_admits_no_client_and_retries_without_a_second_reference() {
    let (log, mut port, platform) = setup();
    let error = crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    };
    port.power_sequence_failure = Some(error);
    let mut power = CommonRadioPower::default();
    assert_eq!(
        power.enter(&mut port, &platform, RadioClient::Ieee802154),
        Err(CommonRadioPowerError::Power(error))
    );
    assert!(!power.holds(RadioClient::Ieee802154));
    assert_eq!(
        power.exit(&mut port, RadioClient::Ieee802154),
        Err(CommonRadioPowerError::NotEntered)
    );

    port.power_sequence_failure = None;
    assert_eq!(
        power.enter(&mut port, &platform, RadioClient::Ieee802154),
        Ok(())
    );
    let acquisitions = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::Acquire(PlatformClock::Pll160m))
        .count();
    assert_eq!(acquisitions, 1);
}

#[test]
fn a_failed_restore_keeps_the_last_client_for_retry() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();
    assert_eq!(power.enter(&mut port, &platform, RadioClient::Wifi), Ok(()));
    port.power_readback = Err(WifiPowerRestoreReadback::ModemSyscon);
    assert_eq!(
        power.exit(&mut port, RadioClient::Wifi),
        Err(CommonRadioPowerError::Restore(
            WifiPowerRestoreCheckpoint::ModemSyscon
        ))
    );
    assert!(power.holds(RadioClient::Wifi));
    // The analog-I2C gate closed before the restore; the sources stay held.
    assert_eq!(
        held(),
        holding(&[PlatformClock::Mpll, PlatformClock::Pll160m])
    );
    port.power_readback = Ok(());
    assert_eq!(power.exit(&mut port, RadioClient::Wifi), Ok(()));
    let releases = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::Release(PlatformClock::AnalogI2cMaster))
        .count();
    assert_eq!(releases, 1);
    assert!(held().is_empty());
}

#[test]
fn a_failed_power_sequence_keeps_its_references_until_the_owner_drops() {
    let (_, mut port, platform) = setup();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(
        power
            .enter(&mut port, &platform, RadioClient::Bluetooth)
            .is_err()
    );
    // The retry reuses the sources taken before the failed checkpoint.
    assert_eq!(
        held(),
        holding(&[PlatformClock::Mpll, PlatformClock::Pll160m])
    );
    drop(power);
    assert!(held().is_empty());
}

#[test]
fn power_up_after_every_client_left_keeps_the_wifi_resets() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();

    for step in 0..3 {
        let result = if step == 1 {
            power.exit(&mut port, RadioClient::Ieee802154)
        } else {
            power.enter(&mut port, &platform, RadioClient::Ieee802154)
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
    let (log, mut port, platform) = setup();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ResetReleased,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(
        power
            .enter(&mut port, &platform, RadioClient::Wifi)
            .is_err()
    );
    port.power_sequence_failure = None;
    assert_eq!(power.enter(&mut port, &platform, RadioClient::Wifi), Ok(()));
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
