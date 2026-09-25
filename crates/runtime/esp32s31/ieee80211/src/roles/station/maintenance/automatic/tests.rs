use core::num::NonZeroU64;

use oer_esp32s31_phy::tracking::{
    CalibrationProgress, PhyParamTrackRequest, PhyParamTrackingOutcome,
};

use super::*;

fn config() -> Config {
    Config::new(NonZeroU64::new(1_000).unwrap())
}

fn outcome(common: bool, wifi: bool) -> PhyParamTrackingOutcome {
    PhyParamTrackingOutcome {
        clients: PhyParamTrackRequest::new(true, false),
        tracking_inhibited: false,
        calibration: CalibrationProgress {
            common,
            wifi,
            bluetooth_ieee802154: false,
        },
    }
}

#[test]
fn completed_operations_accumulate_counts_and_pause_time() {
    let control = Control::new();
    control.configure(Some(config()));
    assert!(control.try_begin(config(), Operation::CommonCalibration));
    control.completed(Operation::CommonCalibration, 40, Some(outcome(true, true)));
    assert!(control.try_begin(config(), Operation::Temperature));
    control.completed(Operation::Temperature, 10, Some(outcome(false, false)));

    let report = control.measurements();
    assert_eq!(report.operations, [1, 0, 0, 1, 0, 0]);
    assert_eq!(report.common_calibrated, 1);
    assert_eq!(report.wifi_calibrated, 1);
    assert_eq!(report.pause_micros, 50);
    assert_eq!(report.maximum_pause_micros, 40);
    assert_eq!(
        control.status(),
        Status::Completed {
            operation: Operation::Temperature,
            elapsed_micros: 10,
        }
    );
}

#[test]
fn missing_outcome_is_deferred_work_not_a_completed_operation() {
    let control = Control::new();
    control.configure(Some(config()));
    assert!(control.try_begin(config(), Operation::Rfpll));
    control.completed(Operation::Rfpll, 5, None);
    let report = control.measurements();
    assert_eq!(report.operations, [0; 6]);
    assert_eq!(report.deferred, 1);
    assert_eq!(report.pause_micros, 5);
    assert_eq!(control.status(), Status::Deferred(Operation::Rfpll));
}

#[test]
fn only_the_current_configuration_may_begin_one_operation() {
    let control = Control::new();
    assert!(!control.try_begin(config(), Operation::Temperature));
    control.configure(Some(config()));
    let other = Config::new(NonZeroU64::new(2_000).unwrap());
    assert!(!control.try_begin(other, Operation::Temperature));
    assert!(control.try_begin(config(), Operation::Temperature));
    assert!(!control.try_begin(config(), Operation::WifiPower));
}

#[test]
fn failure_requires_a_new_configuration() {
    let control = Control::new();
    control.configure(Some(config()));
    control.report(Status::Failed(PauseError::PhyTracking));
    assert_eq!(control.snapshot(), (None, false));
    assert!(control.measurements().failed);
    assert!(!control.try_begin(config(), Operation::Temperature));

    // Leaving the epoch does not hide the retained failure.
    control.end_epoch();
    assert_eq!(control.status(), Status::Failed(PauseError::PhyTracking));
    control.configure(Some(config()));
    assert_eq!(control.status(), Status::Disabled);
    assert!(!control.measurements().failed);
}

#[test]
fn waiting_and_suspension_are_ignored_without_a_configuration() {
    let control = Control::new();
    control.report(Status::Waiting { deadline_micros: 9 });
    control.report(Status::Suspended(Suspension::Clock));
    assert_eq!(control.status(), Status::Disabled);
    assert!(!control.measurements().suspended);

    control.configure(Some(config()));
    control.report(Status::Suspended(Suspension::Clock));
    assert_eq!(control.status(), Status::Suspended(Suspension::Clock));
    assert!(control.measurements().suspended);
}
