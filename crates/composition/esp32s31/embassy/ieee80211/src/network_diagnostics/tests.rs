use super::*;
use core::cell::Cell;

#[test]
fn snapshots_follow_the_selected_endpoint_and_preserve_unavailable_state() {
    let monitors = Monitors::new();
    let station = Cell::new(3);
    let ap = Cell::new(7);
    for interface in [NetworkInterface::Station, NetworkInterface::AccessPoint] {
        assert_eq!(
            monitors.snapshot(interface, |value: &&Cell<u32>| value.get()),
            None
        );
    }
    monitors.initialize(&station, &ap);
    ap.set(11);
    assert_eq!(
        monitors.snapshot(NetworkInterface::Station, |value| value.get()),
        Some(3)
    );
    assert_eq!(
        monitors.snapshot(NetworkInterface::AccessPoint, |value| value.get()),
        Some(11)
    );
    station.set(0);
    assert_eq!(
        monitors.snapshot(NetworkInterface::Station, |value| value.get()),
        Some(0)
    );
    assert_eq!(
        monitors.snapshot(NetworkInterface::AccessPoint, |value| value.get()),
        Some(11)
    );
}
