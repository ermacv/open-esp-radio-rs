use super::*;
use crate::le::peripheral::{
    deadlines::Deadlines, procedure::PeripheralProcedureDeadline,
    supervision::PeripheralSupervisionDeadline,
};
fn at(us: u32) -> crate::SchedulerInstant {
    crate::SchedulerInstant::from_image(us)
}
fn budget() -> PeripheralMaintenanceBudget {
    PeripheralMaintenanceBudget::new(
        NonZeroU32::new(100).unwrap(),
        NonZeroU32::new(50).unwrap(),
        NonZeroU32::new(25).unwrap(),
    )
    .unwrap()
}
fn deadlines() -> Deadlines {
    Deadlines {
        termination: None,
        procedure: None,
        supervision: None,
    }
}
#[test]
fn fresh_window_reserves_restoration_and_guard_without_extending_execution() {
    let window = budget()
        .admit(
            at(1000),
            at(2000),
            at(2100),
            10_000,
            10_100,
            50,
            deadlines(),
        )
        .unwrap();
    assert_eq!(window.execution.expires_at_micros(), 10_200);
    assert_eq!(window.restoration.expires_at_micros(), 10_250);
    assert_eq!(
        window.execution.started_at_micros(),
        window.restoration.started_at_micros()
    );
}
#[test]
fn acquisition_latency_and_equality_consume_available_airtime() {
    for now in [10_800, 10_950, 11_000] {
        assert_eq!(
            budget()
                .admit(at(1000), at(2000), at(2100), 10_000, now, 50, deadlines())
                .unwrap_err(),
            if now >= 10_950 {
                PeripheralMaintenanceBlocked::EventAlreadyLate
            } else {
                PeripheralMaintenanceBlocked::WindowTooShort
            }
        );
    }
    assert!(
        budget()
            .admit(
                at(1000),
                at(2000),
                at(2100),
                10_000,
                10_799,
                50,
                deadlines()
            )
            .is_ok()
    );
}
#[test]
fn supervision_must_cover_the_whole_successor_and_margin() {
    let mut limits = deadlines();
    for expiry in [1000, 2000, 2100, 2125] {
        limits.supervision = Some(PeripheralSupervisionDeadline::new(at(0), expiry));
        assert_eq!(
            budget()
                .admit(at(1000), at(2000), at(2100), 10_000, 10_000, 50, limits)
                .unwrap_err(),
            PeripheralMaintenanceBlocked::ProtocolDeadline
        );
    }
    limits.supervision = Some(PeripheralSupervisionDeadline::new(at(0), 2126));
    assert!(
        budget()
            .admit(at(1000), at(2000), at(2100), 10_000, 10_000, 50, limits)
            .is_ok()
    );
}
#[test]
fn procedure_margin_and_controller_wrap_are_independent_of_monotonic_time() {
    let mut limits = deadlines();
    limits.procedure = Some(PeripheralProcedureDeadline::new(at(0)));
    assert_eq!(
        budget()
            .admit(
                at(39_999_000),
                at(39_999_900),
                at(39_999_975),
                1_000,
                1_010,
                50,
                limits
            )
            .unwrap_err(),
        PeripheralMaintenanceBlocked::ProtocolDeadline
    );
    let window = budget()
        .admit(
            at(u32::MAX - 999),
            at(0),
            at(100),
            123_000,
            123_010,
            50,
            deadlines(),
        )
        .unwrap();
    assert_eq!(window.restoration.expires_at_micros(), 123_160);
}
#[test]
fn missing_clock_order_overflow_and_past_events_never_grant_a_window() {
    assert_eq!(
        budget()
            .admit(at(1000), at(2000), at(2100), 10_000, 9999, 50, deadlines())
            .unwrap_err(),
        PeripheralMaintenanceBlocked::Clock
    );
    assert_eq!(
        budget()
            .admit(
                at(1000),
                at(2000),
                at(2100),
                u64::MAX - 20,
                u64::MAX - 10,
                50,
                deadlines()
            )
            .unwrap_err(),
        PeripheralMaintenanceBlocked::Clock
    );
    assert_eq!(
        budget()
            .admit(at(1000), at(900), at(1100), 10_000, 10_000, 50, deadlines())
            .unwrap_err(),
        PeripheralMaintenanceBlocked::EventAlreadyLate
    );
    assert!(
        PeripheralMaintenanceBudget::new(
            NonZeroU32::new(i32::MAX as u32).unwrap(),
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap()
        )
        .is_none()
    );
}

#[test]
fn real_epoch_projects_both_window_edges_across_raw_controller_wrap() {
    use crate::{ControllerSchedulerEpoch, ControllerTimeSample, scheduler::SchedulerRawWindow};
    let scale = oer_esp32s31_pac::BluetoothControllerHalInitConfig::reviewed_standalone()
        .controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(
        ControllerTimeSample::for_validation(u32::MAX - 99),
        50_000,
        scale,
    );
    let window = SchedulerRawWindow::from_projected_scheduler_window(100, 500).unwrap();
    assert_eq!(epoch.project_peripheral_event_start(window), 50_100);
    assert_eq!(epoch.project_peripheral_event_end(window), 50_300);
}
