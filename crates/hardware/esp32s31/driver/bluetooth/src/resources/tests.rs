use core::sync::atomic::{AtomicUsize, Ordering};

use crate::controller::time::ControllerTimeWorkerPhase;

use oer_esp32s31_bluetooth_memory::{
    DirectionFindingWorkspaceModelAddress, DirectionFindingWorkspaceStorage,
    NonScanningRxMemoryModelAddress, NonScanningRxMemoryStorage, PassiveScanDefaultTxPowerDbm,
    PassiveScanMemoryGraphModelAddress, PassiveScanMemoryGraphPublicationError,
    PassiveScanMemoryGraphPublicationPrepared, PassiveScanMemoryGraphStorage,
    PassiveScanPrimaryChannel, PassiveScanResetConfig, PassiveScanSchedulerAllocationConfig,
    PassiveScanSchedulerWindow, PassiveScanStartSelection, PeripheralConnectionDataChannel,
    PeripheralConnectionDefaultTxPowerDbm, PeripheralConnectionEventSpan,
    PeripheralConnectionIdentity, PeripheralConnectionIntervalTicks,
    PeripheralConnectionMemoryGraphModelAddress, PeripheralConnectionMemoryGraphPublicationError,
    PeripheralConnectionMemoryGraphPublicationPrepared, PeripheralConnectionMemoryGraphStorage,
    PeripheralConnectionReceiveWait, PeripheralConnectionSchedulerPriority,
    PeripheralConnectionSchedulerWindow, RxMemoryListClass,
};

use oer_esp32s31_hal::{
    bluetooth::{BluetoothControllerLatchedTime, RxMemoryListPublished},
    owner::SharedPhyAccess,
};

use super::{
    BluetoothRadioHardware, BluetoothStopped, TeardownPendingPlatform,
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

fn passive_scan_publication_prepared() -> PassiveScanMemoryGraphPublicationPrepared {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(PassiveScanMemoryGraphStorage::new()));
    let graph = PassiveScanMemoryGraphStorage::pin_static_model(
        storage,
        PassiveScanMemoryGraphModelAddress::new(0x2f00_1000)
            .expect("the model scanner address is controller-encodable"),
        PassiveScanResetConfig::le_1m_public_accept_all(
            PassiveScanDefaultTxPowerDbm::new(0),
            BluetoothControllerLatchedTime::from_bits(0),
        ),
        PassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the restricted scanner allocation fits"),
    )
    .expect("the model scanner graph fits controller SRAM");
    graph
        .prepare_first_event(
            PassiveScanPrimaryChannel::Channel37,
            PassiveScanSchedulerWindow::from_controller_ticks(100, 200)
                .expect("the model scanner window is nonempty"),
            PassiveScanStartSelection::Requested,
            BluetoothControllerLatchedTime::from_bits(0),
        )
        .prepare_scheduler_admission()
        .prepare_publication()
}

fn peripheral_connection_publication_prepared() -> PeripheralConnectionMemoryGraphPublicationPrepared
{
    let graph_storage = std::boxed::Box::leak(std::boxed::Box::new(
        PeripheralConnectionMemoryGraphStorage::new(),
    ));
    let graph = PeripheralConnectionMemoryGraphStorage::pin_static_model(
        graph_storage,
        PeripheralConnectionMemoryGraphModelAddress::new(0x2f01_1000)
            .expect("the model connection address is controller-encodable"),
    )
    .expect("the model connection graph fits controller SRAM");
    let receive_storage =
        std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let receive_pool = NonScanningRxMemoryStorage::pin_static_model(
        receive_storage,
        NonScanningRxMemoryModelAddress::new(0x2f01_3000)
            .expect("the model RX address is controller-encodable"),
    )
    .expect("the model RX graph fits controller SRAM");
    let workspace_storage =
        std::boxed::Box::leak(std::boxed::Box::new(DirectionFindingWorkspaceStorage::new()));
    let workspace = DirectionFindingWorkspaceStorage::pin_static_model(
        workspace_storage,
        DirectionFindingWorkspaceModelAddress::new(0x2f01_5000)
            .expect("the model workspace address is controller-encodable"),
    )
    .expect("the model workspace fits controller SRAM");

    graph
        .prepare_identity(PeripheralConnectionIdentity::new(
            [0xd4, 0xc3, 0xb2, 0xa1],
            [0x33, 0x22, 0x11],
        ))
        .attach_receive_pool(receive_pool)
        .prepare_reviewed_first_event_fields(
            PeripheralConnectionDataChannel::new(0).expect("data channel zero is valid"),
            PeripheralConnectionIntervalTicks::new(24_000)
                .expect("the connection interval is nonzero"),
            PeripheralConnectionEventSpan::new(23_000).expect("the event span is nonempty"),
            PeripheralConnectionSchedulerWindow::new(100, 200)
                .expect("the scheduler window is nonempty"),
            PeripheralConnectionReceiveWait::new(1_250, 16)
                .expect("the first receive wait is representable"),
            PeripheralConnectionDefaultTxPowerDbm::new(0),
            PeripheralConnectionSchedulerPriority::FIRST_EVENT,
            92,
        )
        .install_direction_finding_workspace(workspace.binding().link())
        .prepare_scheduler_admission()
        .prepare_publication()
}

#[test]
fn pending_phy_teardown_suppresses_implicit_platform_release() {
    PLATFORM_DROPS.store(0, Ordering::Relaxed);
    drop(TeardownPendingPlatform::new(PlatformDropCounter));
    assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
}

#[test]
fn passive_scan_publication_mismatch_returns_graph_and_hal_owner() {
    let prepared = passive_scan_publication_prepared();
    let foreign = RxMemoryListPublished::from_parts_for_validation(
        RxMemoryListClass::NonScanning.selector(),
        prepared.head(),
    );
    let mismatch = match join_passive_scan_rx_publication(prepared, foreign) {
        Ok(_) => panic!("a non-scanning publication cannot own the scanner graph"),
        Err(mismatch) => mismatch,
    };
    assert_eq!(
        mismatch.error(),
        PassiveScanMemoryGraphPublicationError::SelectorMismatch
    );
    let (prepared, foreign) = mismatch.into_parts();
    let matching =
        RxMemoryListPublished::from_parts_for_validation(prepared.selector(), prepared.head());
    let published = match join_passive_scan_rx_publication(prepared, matching) {
        Ok(published) => published,
        Err(_) => panic!("the recovered scanner graph accepts its matching publication"),
    };
    let _retained_owners = (published, foreign);
}

#[test]
fn peripheral_publication_mismatch_returns_graph_and_hal_owner() {
    let prepared = peripheral_connection_publication_prepared();
    let foreign = RxMemoryListPublished::from_parts_for_validation(
        RxMemoryListClass::Scanning.selector(),
        prepared.receive_head(),
    );
    let mismatch = match join_peripheral_connection_rx_publication(prepared, foreign) {
        Ok(_) => panic!("a scanner publication cannot own the connection graph"),
        Err(mismatch) => mismatch,
    };
    assert_eq!(
        mismatch.error(),
        PeripheralConnectionMemoryGraphPublicationError::SelectorMismatch
    );
    let (prepared, foreign) = mismatch.into_parts();
    let matching = RxMemoryListPublished::from_parts_for_validation(
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
        ControllerTimeWorkerPhase::Idle
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
        oer_esp32s31_hal::bluetooth::TaskOwnerReuniteError::HardwareLifecycleNotRestored
    );
}
