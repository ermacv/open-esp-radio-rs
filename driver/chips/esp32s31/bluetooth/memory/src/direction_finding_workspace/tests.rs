use super::{
    BluetoothDirectionFindingWorkspaceBindError, BluetoothDirectionFindingWorkspaceModelAddress,
    BluetoothDirectionFindingWorkspaceStorage,
};

fn storage() -> &'static mut BluetoothDirectionFindingWorkspaceStorage {
    std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothDirectionFindingWorkspaceStorage::new(),
    ))
}

#[test]
fn model_binding_initializes_the_disabled_cte_workspace() {
    let base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f00_1000)
        .expect("model base is encodable");
    let owner = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(storage(), base)
        .expect("the complete workspace fits physical SRAM");

    assert!(owner.is_disabled_baseline_initialized());
}

#[test]
fn model_binding_rejects_a_crossing_extent() {
    let base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f07_fff0)
        .expect("crossing base itself is encodable");
    let failure = match BluetoothDirectionFindingWorkspaceStorage::pin_static_model(storage(), base)
    {
        Ok(_) => panic!("the complete workspace must fit physical SRAM"),
        Err(failure) => failure,
    };

    assert_eq!(
        failure.error(),
        BluetoothDirectionFindingWorkspaceBindError::ExtentOutsidePhysicalSram
    );
}
