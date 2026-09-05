use core::sync::atomic::{AtomicUsize, Ordering};

use crate::controller::time::BluetoothControllerTimeWorkerPhase;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDirectionFindingWorkspaceModelAddress, BluetoothDirectionFindingWorkspaceStorage,
    BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
    BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanMemoryGraphModelAddress,
    BluetoothPassiveScanMemoryGraphPublicationError,
    BluetoothPassiveScanMemoryGraphPublicationPrepared, BluetoothPassiveScanMemoryGraphStorage,
    BluetoothPassiveScanPrimaryChannel, BluetoothPassiveScanResetConfig,
    BluetoothPassiveScanSchedulerAllocationConfig, BluetoothPassiveScanSchedulerWindow,
    BluetoothPassiveScanStartSelection, BluetoothPeripheralConnectionDataChannel,
    BluetoothPeripheralConnectionDefaultTxPowerDbm, BluetoothPeripheralConnectionEventSpan,
    BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionIntervalTicks,
    BluetoothPeripheralConnectionMemoryGraphModelAddress,
    BluetoothPeripheralConnectionMemoryGraphPublicationError,
    BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
    BluetoothPeripheralConnectionMemoryGraphStorage, BluetoothPeripheralConnectionReceiveWait,
    BluetoothPeripheralConnectionSchedulerPriority, BluetoothPeripheralConnectionSchedulerWindow,
    BluetoothRxMemoryListClass,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerLatchedTime, BluetoothRxMemoryListPublished, SharedPhyAccess,
};

use super::{
    BluetoothRadioHardware, BluetoothStopped, BluetoothTeardownPendingPlatform,
    join_passive_scan_rx_publication, join_peripheral_connection_rx_publication,
    separate_interrupt_owner,
};

static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

struct PlatformDropCounter;

impl Drop for PlatformDropCounter {
    fn drop(&mut self) {
        PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

fn passive_scan_publication_prepared() -> BluetoothPassiveScanMemoryGraphPublicationPrepared {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPassiveScanMemoryGraphStorage::new(),
    ));
    let graph = BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
        storage,
        BluetoothPassiveScanMemoryGraphModelAddress::new(0x2f00_1000)
            .expect("the model scanner address is controller-encodable"),
        BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
            BluetoothPassiveScanDefaultTxPowerDbm::new(0),
            BluetoothControllerLatchedTime::from_bits(0),
        ),
        BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the restricted scanner allocation fits"),
    )
    .expect("the model scanner graph fits controller SRAM");
    graph
        .prepare_first_event(
            BluetoothPassiveScanPrimaryChannel::Channel37,
            BluetoothPassiveScanSchedulerWindow::from_controller_ticks(100, 200)
                .expect("the model scanner window is nonempty"),
            BluetoothPassiveScanStartSelection::Requested,
            BluetoothControllerLatchedTime::from_bits(0),
        )
        .prepare_scheduler_admission()
        .prepare_publication()
}

fn peripheral_connection_publication_prepared()
-> BluetoothPeripheralConnectionMemoryGraphPublicationPrepared {
    let graph_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPeripheralConnectionMemoryGraphStorage::new(),
    ));
    let graph = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(
        graph_storage,
        BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f01_1000)
            .expect("the model connection address is controller-encodable"),
    )
    .expect("the model connection graph fits controller SRAM");
    let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothNonScanningRxMemoryStorage::new(),
    ));
    let receive_pool = BluetoothNonScanningRxMemoryStorage::pin_static_model(
        receive_storage,
        BluetoothNonScanningRxMemoryModelAddress::new(0x2f01_3000)
            .expect("the model RX address is controller-encodable"),
    )
    .expect("the model RX graph fits controller SRAM");
    let workspace_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothDirectionFindingWorkspaceStorage::new(),
    ));
    let workspace = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(
        workspace_storage,
        BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f01_5000)
            .expect("the model workspace address is controller-encodable"),
    )
    .expect("the model workspace fits controller SRAM");

    graph
        .prepare_identity(BluetoothPeripheralConnectionIdentity::new(
            [0xd4, 0xc3, 0xb2, 0xa1],
            [0x33, 0x22, 0x11],
        ))
        .attach_receive_pool(receive_pool)
        .prepare_reviewed_first_event_fields(
            BluetoothPeripheralConnectionDataChannel::new(0).expect("data channel zero is valid"),
            BluetoothPeripheralConnectionIntervalTicks::new(24_000)
                .expect("the connection interval is nonzero"),
            BluetoothPeripheralConnectionEventSpan::new(23_000)
                .expect("the event span is nonempty"),
            BluetoothPeripheralConnectionSchedulerWindow::new(100, 200)
                .expect("the scheduler window is nonempty"),
            BluetoothPeripheralConnectionReceiveWait::new(1_250, 16)
                .expect("the first receive wait is representable"),
            BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
            BluetoothPeripheralConnectionSchedulerPriority::FIRST_EVENT,
        )
        .install_direction_finding_workspace(workspace.binding().link())
        .prepare_scheduler_admission()
        .prepare_publication()
}

#[test]
fn pending_phy_teardown_suppresses_implicit_platform_release() {
    PLATFORM_DROPS.store(0, Ordering::Relaxed);
    drop(BluetoothTeardownPendingPlatform::new(PlatformDropCounter));
    assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
}

#[test]
fn passive_scan_publication_mismatch_returns_graph_and_hal_owner() {
    let prepared = passive_scan_publication_prepared();
    let foreign = BluetoothRxMemoryListPublished::from_parts_for_validation(
        BluetoothRxMemoryListClass::NonScanning.selector(),
        prepared.head(),
    );
    let mismatch = match join_passive_scan_rx_publication(prepared, foreign) {
        Ok(_) => panic!("a non-scanning publication cannot own the scanner graph"),
        Err(mismatch) => mismatch,
    };
    assert_eq!(
        mismatch.error(),
        BluetoothPassiveScanMemoryGraphPublicationError::SelectorMismatch
    );
    let (prepared, foreign) = mismatch.into_parts();
    let matching = BluetoothRxMemoryListPublished::from_parts_for_validation(
        prepared.selector(),
        prepared.head(),
    );
    let published = match join_passive_scan_rx_publication(prepared, matching) {
        Ok(published) => published,
        Err(_) => panic!("the recovered scanner graph accepts its matching publication"),
    };
    let _retained_owners = (published, foreign);
}

#[test]
fn peripheral_publication_mismatch_returns_graph_and_hal_owner() {
    let prepared = peripheral_connection_publication_prepared();
    let foreign = BluetoothRxMemoryListPublished::from_parts_for_validation(
        BluetoothRxMemoryListClass::Scanning.selector(),
        prepared.receive_head(),
    );
    let mismatch = match join_peripheral_connection_rx_publication(prepared, foreign) {
        Ok(_) => panic!("a scanner publication cannot own the connection graph"),
        Err(mismatch) => mismatch,
    };
    assert_eq!(
        mismatch.error(),
        BluetoothPeripheralConnectionMemoryGraphPublicationError::SelectorMismatch
    );
    let (prepared, foreign) = mismatch.into_parts();
    let matching = BluetoothRxMemoryListPublished::from_parts_for_validation(
        prepared.selector(),
        prepared.receive_head(),
    );
    let published = match join_peripheral_connection_rx_publication(prepared, matching) {
        Ok(published) => published,
        Err(_) => panic!("the recovered connection graph accepts its matching publication"),
    };
    let _retained_owners = (published, foreign);
}

#[test]
fn task_and_interrupt_owners_reunite_into_the_same_radio_root() {
    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, ()) = stopped.into_parts();
    let (task, setup) = separate_interrupt_owner(registers);
    assert_eq!(
        task.controller_time_phase(),
        BluetoothControllerTimeWorkerPhase::Idle
    );
    let hardware = task
        .reunite(setup)
        .expect("untouched owners remain cold-reunitable")
        .release()
        .expect("an untouched Bluetooth route can be released");

    // Re-entering Wi-Fi proves that every inactive protocol and shared
    // owner survived the complete Bluetooth ownership roundtrip.
    let _wifi = hardware.into_wifi();
}

#[test]
fn mutable_shared_phy_borrow_arms_fail_stop_reunion() {
    fn accepts_shared_phy(_: &mut impl SharedPhyAccess) {}

    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, ()) = stopped.into_parts();
    let (mut task, setup) = separate_interrupt_owner(registers);
    {
        let mut phy = task.shared_phy_hal();
        accepts_shared_phy(&mut phy);
    }

    let failure = match task.reunite(setup) {
        Ok(_) => panic!("a mutable shared-PHY borrow requires verified rollback"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        open_esp_radio_esp32s31_hal::BluetoothTaskOwnerReuniteError::HardwareLifecycleNotRestored
    );
}
