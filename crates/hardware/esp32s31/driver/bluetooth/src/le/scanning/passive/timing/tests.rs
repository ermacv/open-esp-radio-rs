use crate::{SchedulerInstant, scheduler::SchedulerSoftwareConfig};

use oer_bluetooth_ll::scanning::{
    LegacyPassiveScanParameters, LegacyScanInterval, LegacyScanWindow,
};

use super::PassiveScanEventWindow;

fn parameters(interval: u16, window: u16) -> LegacyPassiveScanParameters {
    LegacyPassiveScanParameters::new(
        LegacyScanInterval::new(interval).expect("the interval is valid"),
        LegacyScanWindow::new(window).expect("the window is valid"),
    )
    .expect("the window fits the interval")
}

#[test]
fn later_readiness_moves_the_complete_window_without_shortening_reception() {
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let parameters = parameters(16, 16);
    let current = SchedulerInstant::from_image(1_000);
    let nominal = PassiveScanEventWindow::first(
        config,
        current,
        SchedulerInstant::from_image(1_000),
        parameters,
    );
    let delayed = PassiveScanEventWindow::first(
        config,
        current,
        SchedulerInstant::from_image(2_000),
        parameters,
    );

    assert_eq!(
        delayed.end.image().wrapping_sub(delayed.start.image()),
        nominal.end.image().wrapping_sub(nominal.start.image())
    );
    assert!(nominal.start.is_before(delayed.start));
}

#[test]
fn recurring_window_preserves_phase_and_skips_expired_intervals() {
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let parameters = parameters(16, 8);
    let first = PassiveScanEventWindow::first(
        config,
        SchedulerInstant::from_image(1_000),
        SchedulerInstant::from_image(1_000),
        parameters,
    );
    let current = SchedulerInstant::from_image(first.anchor.image() + 25_000);
    let next = PassiveScanEventWindow::recurring(
        config,
        current,
        SchedulerInstant::from_image(1_000),
        first.phase(),
        parameters,
    );

    assert_eq!(
        next.anchor.image().wrapping_sub(first.anchor.image()) % parameters.interval().micros(),
        0
    );
    assert!(!next.start.is_before(current));
}
