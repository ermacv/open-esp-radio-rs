use super::{BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage};

#[test]
fn failed_binding_returns_the_same_opaque_allocation() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
    let original = core::ptr::from_mut(storage);
    let base = BluetoothBlePhyEngineModelAddress::new(0x2f07_fffc)
        .expect("model base uses the controller-SRAM encoding");
    let failure = match BluetoothBlePhyEngineStorage::pin_static_model(storage, base) {
        Ok(_) => panic!("both retained extents cross physical SRAM"),
        Err(failure) => failure,
    };
    let (storage, _) = failure.into_parts();
    assert_eq!(core::ptr::from_mut(storage), original);
}

#[test]
fn le_1m_calibration_preserves_elapsed_controller_time() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
    let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_0100)
        .expect("model base uses the controller-SRAM encoding");
    let owner = BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
        .expect("complete model storage fits physical SRAM");
    let calibration = owner.le_1m_packet_start_calibration();

    let first = calibration.normalize_controller_micros(1_000);
    let second = calibration.normalize_controller_micros(1_001);

    assert_ne!(first, 1_000, "the initialized calibration is not zero");
    assert_eq!(second.wrapping_sub(first), 1);
}
