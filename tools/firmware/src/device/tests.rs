use super::*;

#[test]
fn aliases_and_checkouts_share_one_device_owner() {
    let directory = tempfile::tempdir().unwrap();
    let owner =
        DeviceLease::acquire_identity(directory.path(), "usb:test", Path::new("/dev/by-id/test"))
            .unwrap();
    assert!(
        DeviceLease::acquire_identity(directory.path(), "usb:test", Path::new("/dev/ttyACM0"))
            .is_err()
    );
    let independent =
        DeviceLease::acquire_identity(directory.path(), "usb:second", Path::new("/dev/ttyACM1"))
            .unwrap();
    drop(owner);
    assert!(
        DeviceLease::acquire_identity(directory.path(), "usb:test", Path::new("/dev/ttyACM0"))
            .is_ok()
    );
    drop(independent);
}
