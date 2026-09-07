use super::ClockCoordinator;

#[test]
fn bluetooth_platform_lease_is_exclusive_and_drop_releases_it() {
    let coordinator = ClockCoordinator::new();
    let first = coordinator.try_bluetooth().unwrap();
    assert!(coordinator.try_bluetooth().is_err());
    drop(first);
    assert!(coordinator.try_bluetooth().is_ok());
}
