use open_esp_radio_bluetooth_ll::scanning::{
    LegacyPassiveScanParameters, LegacyScanInterval, LegacyScanWindow,
};

use super::BluetoothPassiveScanEventWindow;
use crate::{BluetoothSchedulerInstant, BluetoothSchedulerSoftwareConfig};

fn parameters(interval: u16, window: u16) -> LegacyPassiveScanParameters {
    LegacyPassiveScanParameters::new(
        LegacyScanInterval::new(interval).expect("the interval is valid"),
        LegacyScanWindow::new(window).expect("the window is valid"),
    )
    .expect("the window fits the interval")
}

#[test]
fn later_readiness_moves_the_complete_window_without_shortening_reception() {
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
    let parameters = parameters(16, 16);
    let current = BluetoothSchedulerInstant::from_image(1_000);
    let nominal = BluetoothPassiveScanEventWindow::first(
        config,
        current,
        BluetoothSchedulerInstant::from_image(1_000),
        parameters,
    );
    let delayed = BluetoothPassiveScanEventWindow::first(
        config,
        current,
        BluetoothSchedulerInstant::from_image(2_000),
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
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
    let parameters = parameters(16, 8);
    let first = BluetoothPassiveScanEventWindow::first(
        config,
        BluetoothSchedulerInstant::from_image(1_000),
        BluetoothSchedulerInstant::from_image(1_000),
        parameters,
    );
    let current = BluetoothSchedulerInstant::from_image(first.anchor.image() + 25_000);
    let next = BluetoothPassiveScanEventWindow::recurring(
        config,
        current,
        BluetoothSchedulerInstant::from_image(1_000),
        first.phase(),
        parameters,
    );

    assert_eq!(
        next.anchor.image().wrapping_sub(first.anchor.image()) % parameters.interval().micros(),
        0
    );
    assert!(!next.start.is_before(current));
}
