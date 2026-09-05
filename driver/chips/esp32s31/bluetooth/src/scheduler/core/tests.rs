use core::sync::atomic::{AtomicUsize, Ordering};

use std::{cell::RefCell, rc::Rc, vec::Vec};

use open_esp_radio_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertiser::LegacyAdvertiserStandby,
    advertising::{
        AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
        LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannelMap,
    },
    connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest,
        LePeripheralConnection,
    },
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDirectionFindingWorkspaceLink, BluetoothDirectionFindingWorkspaceModelAddress,
    BluetoothDirectionFindingWorkspaceStorage, BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyAdvertisingMemoryGraphModelAddress,
    BluetoothLegacyAdvertisingMemoryGraphStorage, BluetoothNonScanningRxMemoryModelAddress,
    BluetoothNonScanningRxMemoryStorage, BluetoothPassiveScanDefaultTxPowerDbm,
    BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanMemoryGraphStorage,
    BluetoothPassiveScanPrimaryChannel, BluetoothPassiveScanResetConfig,
    BluetoothPassiveScanSchedulerAllocationConfig, BluetoothPeripheralConnectionDefaultTxPowerDbm,
    BluetoothPeripheralConnectionMemoryGraphModelAddress,
    BluetoothPeripheralConnectionMemoryGraphStorage,
};
use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use crate::{
    BluetoothClockedResources, BluetoothControllerRuntimeResources,
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
    BluetoothDtmPhy, BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow,
    BluetoothDtmSchedulerItemEvent, BluetoothRadioHardware, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerInstant, BluetoothStopped, controller::time::BluetoothControllerSchedulerNow,
};

fn legacy_advertiser_enabled()
-> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
    let advertisement = LegacyNonconnectableAdvertisement::new(
        LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
        LegacyAdvertisingData::new(&[2, 1, 6]).expect("the fixed data fits"),
    );
    LegacyAdvertiserStandby::new()
        .configure(LegacyNonconnectableAdvertisingSet::new(
            advertisement,
            PrimaryAdvertisingChannelMap::all(),
            AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS)
                .expect("the minimum interval is valid"),
        ))
        .enable()
        .expect("the first generation is available")
}

fn legacy_advertising_memory() -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
        .expect("the advertising graph fits physical controller SRAM")
}

fn passive_scan_candidate() -> super::BluetoothPassiveScanFirstEventCandidate {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPassiveScanMemoryGraphStorage::new(),
    ));
    let base = BluetoothPassiveScanMemoryGraphModelAddress::new(0x2f00_1000)
        .expect("the model base uses controller SRAM syntax");
    let reset = BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
        BluetoothPassiveScanDefaultTxPowerDbm::new(0),
        BluetoothControllerLatchedTime::from_bits(10_000),
    );
    let allocation = BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
        .expect("the restricted product limits fit the scanner graph");
    let graph =
        BluetoothPassiveScanMemoryGraphStorage::pin_static_model(storage, base, reset, allocation)
            .expect("the scanner graph fits physical controller SRAM");
    super::BluetoothPassiveScanFirstEventCandidate::new(
        graph,
        BluetoothPassiveScanPrimaryChannel::Channel37,
        crate::BluetoothSchedulerRawWindow::from_projected_scheduler_window(11_000, 12_000)
            .expect("the scanner window is non-empty and forward"),
        BluetoothControllerLatchedTime::from_bits(10_100),
    )
}

fn peripheral_connection_candidate() -> (
    crate::BluetoothPeripheralConnectionRuntimeResources,
    crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventCandidate,
    BluetoothDirectionFindingWorkspaceLink,
) {
    let graph_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPeripheralConnectionMemoryGraphStorage::new(),
    ));
    let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothNonScanningRxMemoryStorage::new(),
    ));
    let graph_base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_3000)
        .expect("the model connection graph base is valid");
    let receive_base = BluetoothNonScanningRxMemoryModelAddress::new(0x2f00_5000)
        .expect("the model receive-pool base is valid");
    let mut runtime = crate::BluetoothPeripheralConnectionRuntimeResources::claim_static_model(
        graph_storage,
        graph_base,
        receive_storage,
        receive_base,
        crate::BluetoothPeripheralConnectionRuntimeConfig::new(
            BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
        ),
    )
    .expect("the connection graph and receive pool fit controller SRAM");
    let request = LeLegacyConnectionRequest::decode(&connection_request())
        .expect("the fixed CONNECT_IND is valid");
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(300),
        20_000,
        scale,
    );
    let candidate = runtime
        .begin_event()
        .expect("the sole connection allocation starts idle")
        .prepare_first_event(
            LePeripheralConnection::from_request(request),
            crate::BluetoothLe1MPacketStartTiming::from_scheduler_micros(21_000),
        )
        .project_scheduler_window(
            epoch,
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        )
        .unwrap_or_else(|_| panic!("the fixed first connection window projects"));

    let workspace_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothDirectionFindingWorkspaceStorage::new(),
    ));
    let workspace_base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f00_7000)
        .expect("the model direction-finding workspace base is valid");
    let workspace = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(
        workspace_storage,
        workspace_base,
    )
    .expect("the direction-finding workspace fits controller SRAM");
    (runtime, candidate, workspace.binding().link())
}

fn connection_request() -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = 0x25;
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    pdu[35] = 5;
    pdu
}

use super::{
    BluetoothDtmControllerEventPreparationError, BluetoothSchedulerEmptyListMergeError,
    BluetoothSchedulerExclusiveListEpoch, BluetoothSchedulerFinishedListDrainState,
    BluetoothSchedulerHardwareListsCleared,
};

static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

struct FakePlatform;

impl Drop for FakePlatform {
    fn drop(&mut self) {
        PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn finished_list_drain_exposes_owner_only_after_the_capture_is_exhausted() {
    let drained_owner = Rc::new(());
    let drained_identity = Rc::clone(&drained_owner);
    let drained = BluetoothSchedulerFinishedListDrainState::from_worker_step(drained_owner, false);
    let BluetoothSchedulerFinishedListDrainState::Drained(drained_owner) = drained else {
        panic!("an exhausted capture must return the ordinary owner");
    };
    assert!(Rc::ptr_eq(&drained_owner, &drained_identity));

    let pending_owner = Rc::new(());
    let pending_identity = Rc::clone(&pending_owner);
    let pending = BluetoothSchedulerFinishedListDrainState::from_worker_step(pending_owner, true);
    let BluetoothSchedulerFinishedListDrainState::Pending(pending) = pending else {
        panic!("a retained capture must keep continuation provenance");
    };
    assert!(Rc::ptr_eq(pending.owner(), &pending_identity));
    assert!(Rc::ptr_eq(&pending.into_owner(), &pending_identity));
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ModelSingleItemIdentity {
    Expected,
    Foreign,
}

#[test]
fn single_item_identity_mismatch_returns_the_exact_owner() {
    let owner = Rc::new(());
    let identity = Rc::clone(&owner);
    let Err((expected, returned)) = super::retain_matching_single_item_identity(
        ModelSingleItemIdentity::Expected,
        ModelSingleItemIdentity::Foreign,
        owner,
    ) else {
        panic!("a foreign role item must fail closed");
    };
    assert!(matches!(expected, ModelSingleItemIdentity::Expected));
    assert!(Rc::ptr_eq(&returned, &identity));
}

#[test]
fn exclusive_empty_epoch_rejects_alias_and_wrong_identity_cancel() {
    let mut list = BluetoothSchedulerExclusiveListEpoch::new(
        BluetoothSchedulerHardwareListsCleared::for_validation(),
    );
    let first = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("first item lies in controller SRAM");
    let other = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0200)
        .expect("second item lies in controller SRAM");

    assert_eq!(list.prepare_first_item(first), Ok(()));
    assert_eq!(
        list.prepare_first_item(other),
        Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
    );
    assert!(!list.cancel_first_item(other));
    assert!(list.cancel_first_item(first));
    assert_eq!(list.prepare_first_item(other), Ok(()));
}

#[test]
fn passive_scanner_merge_cancellation_restores_both_cpu_owned_lists() {
    struct ScannerPlatform;

    let stopped =
        BluetoothStopped::from_hardware(ScannerPlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let admitted = task
        .admit_passive_scan_first_event(
            passive_scan_candidate(),
            super::BluetoothPassiveScanAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(10_000),
            },
        )
        .unwrap_or_else(|_| panic!("the requested scanner window must be admitted"));
    let event = task
        .prepare_passive_scan_first_event(
            admitted,
            super::BluetoothPassiveScanSequenceObservation {
                sample: BluetoothControllerTimeSample::for_validation(10_001),
            },
        )
        .unwrap_or_else(|_| panic!("the retained scanner deadline must remain open"));
    let channel = event.channel();
    let window = event.window();
    let merged = task
        .prepare_passive_scan_empty_list_merge(event)
        .unwrap_or_else(|_| panic!("the pristine common list must accept the scanner item"));
    let event = task
        .cancel_passive_scan_empty_list_merge(merged)
        .unwrap_or_else(|_| panic!("the same epoch must restore the scanner item"));
    assert_eq!(event.channel(), channel);
    assert_eq!(event.window(), window);

    let merged = task
        .prepare_passive_scan_empty_list_merge(event)
        .unwrap_or_else(|_| panic!("cancellation must reopen the common list"));
    let event = task
        .cancel_passive_scan_empty_list_merge(merged)
        .unwrap_or_else(|_| panic!("the restored private chain must remain cancellable"));
    assert_eq!(event.channel(), channel);
    assert_eq!(event.window(), window);
    let _graph = task.cancel_passive_scan_first_event(event);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn passive_scanner_pre_sequence_cancellation_releases_the_timeline() {
    struct ScannerPlatform;

    let stopped =
        BluetoothStopped::from_hardware(ScannerPlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let admitted = task
        .admit_passive_scan_first_event(
            passive_scan_candidate(),
            super::BluetoothPassiveScanAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(10_000),
            },
        )
        .unwrap_or_else(|_| panic!("the first scanner candidate must be admitted"));
    let _graph = task.cancel_passive_scan_first_pre_sequence(admitted);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn connection_pre_sequence_cancellation_releases_the_timeline() {
    struct ConnectionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        ConnectionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let (mut connection_runtime, candidate, _) = peripheral_connection_candidate();
    let admission_sample = candidate.requested_window().start().wrapping_sub(1_000);
    let admitted = task
        .admit_peripheral_connection_first_event(
            candidate,
            super::BluetoothPeripheralConnectionAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(admission_sample),
            },
        )
        .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
    let (allocation, connection) = task.cancel_peripheral_connection_first_pre_sequence(admitted);

    connection_runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(connection_runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn connection_merge_cancellation_restores_private_and_common_lists() {
    struct ConnectionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        ConnectionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let (mut connection_runtime, candidate, workspace) = peripheral_connection_candidate();
    let requested = candidate.requested_window();
    let admitted = task
        .admit_peripheral_connection_first_event(
            candidate,
            super::BluetoothPeripheralConnectionAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    requested.start().wrapping_sub(1_000),
                ),
            },
        )
        .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
    let event = task
        .prepare_peripheral_connection_first_event(
            admitted,
            super::BluetoothPeripheralConnectionSequenceObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    requested.start().wrapping_sub(500),
                ),
            },
            BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
            workspace,
        )
        .unwrap_or_else(|_| panic!("the second connection deadline must remain open"));
    assert_eq!(event.requested_window(), requested);
    assert_eq!(event.resolved_window(), requested);

    let merged = task
        .prepare_peripheral_connection_empty_list_merge(event)
        .unwrap_or_else(|_| panic!("the empty common list must accept the connection item"));
    assert_eq!(
        merged.hardware_list_index(),
        BluetoothSchedulerHardwareListIndex::ZERO
    );
    let event = task
        .cancel_peripheral_connection_empty_list_merge(merged)
        .unwrap_or_else(|_| panic!("the same epoch must restore the connection item"));
    let merged = task
        .prepare_peripheral_connection_empty_list_merge(event)
        .unwrap_or_else(|_| panic!("restoration must reopen both scheduler lists"));
    let event = task
        .cancel_peripheral_connection_empty_list_merge(merged)
        .unwrap_or_else(|_| panic!("the repeated merge must remain reversible"));
    let (allocation, connection) = task.cancel_peripheral_connection_first_event(event);

    connection_runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(connection_runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn connection_admission_failure_returns_the_unchanged_candidate() {
    struct ConnectionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        ConnectionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let (mut connection_runtime, candidate, _) = peripheral_connection_candidate();
    let requested = candidate.requested_window();
    let blocker = task
        .runtime
        .scheduler_timeline_mut()
        .reserve_initial_window(
            requested.start(),
            requested.end(),
            super::BluetoothSchedulerTimingPolicy::from_scheduler_config(
                task.config,
                task.time_scale,
            ),
            BluetoothControllerTimeSample::for_validation(requested.start().wrapping_sub(1_000)),
        )
        .expect("the pristine timeline accepts the blocking window");
    let failure = match task.admit_peripheral_connection_first_event(
        candidate,
        super::BluetoothPeripheralConnectionAdmissionObservation {
            sample: BluetoothControllerTimeSample::for_validation(
                requested.start().wrapping_sub(1_000),
            ),
        },
    ) {
        Ok(_) => panic!("the occupied timeline must reject the connection window"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        super::BluetoothPeripheralConnectionFirstEventPreparationError::Timeline(
            super::BluetoothSchedulerReservationError::TimelineFull,
        )
    );
    let (allocation, connection) = failure.into_candidate().cancel();
    connection_runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(connection_runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);
    task.release_scheduler_reservation(blocker);

    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn connection_merge_failure_preserves_the_prepared_event() {
    struct ConnectionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        ConnectionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let (mut connection_runtime, candidate, workspace) = peripheral_connection_candidate();
    let requested = candidate.requested_window();
    let admitted = task
        .admit_peripheral_connection_first_event(
            candidate,
            super::BluetoothPeripheralConnectionAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    requested.start().wrapping_sub(1_000),
                ),
            },
        )
        .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
    let event = task
        .prepare_peripheral_connection_first_event(
            admitted,
            super::BluetoothPeripheralConnectionSequenceObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    requested.start().wrapping_sub(500),
                ),
            },
            BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
            workspace,
        )
        .unwrap_or_else(|_| panic!("the second connection deadline must remain open"));
    let occupied = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("the occupying item lies in controller SRAM");
    task._scheduler_list
        .prepare_first_item(occupied)
        .expect("the common list starts empty");

    let failure = match task.prepare_peripheral_connection_empty_list_merge(event) {
        Ok(_) => panic!("the occupied common list must reject the connection item"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothSchedulerEmptyListMergeError::ListNotEmpty
    );
    let event = failure.into_prepared();
    assert!(task._scheduler_list.cancel_first_item(occupied));
    let (allocation, connection) = task.cancel_peripheral_connection_first_event(event);
    connection_runtime
        .restore_idle(allocation)
        .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
    assert!(connection_runtime.allocation_is_idle());
    assert_eq!(connection.event_counter(), 0);

    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn published_first_item_cannot_be_cancelled_or_replaced() {
    let mut list = BluetoothSchedulerExclusiveListEpoch::new(
        BluetoothSchedulerHardwareListsCleared::for_validation(),
    );
    let first = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("first item lies in controller SRAM");
    let other = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0200)
        .expect("second item lies in controller SRAM");

    assert_eq!(list.prepare_first_item(first), Ok(()));
    assert!(list.can_publish_first_item(first));
    assert!(!list.can_publish_first_item(other));
    list.retain_published_first_item(first);

    assert!(!list.can_publish_first_item(first));
    assert!(!list.cancel_first_item(first));
    list.retain_running_first_item(first);
    assert!(list.retains_running_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.retain_completion_observed_first_item(first);
    assert!(list.retains_completion_observed_first_item(first));
    assert!(!list.retains_running_first_item(first));
    assert!(!list.cancel_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.retain_hardware_head_empty_first_item(first);
    assert!(!list.retains_completion_observed_first_item(first));
    assert!(list.retains_hardware_head_empty_first_item(first));
    assert!(list.unlink_software_list_first_item(first));
    assert!(!list.unlink_software_list_first_item(first));
    assert!(list.retains_unlinked_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.retain_software_list_removal_ready_first_item(first);
    assert!(list.retains_software_list_removal_ready_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.commit_recycled_first_item();
    assert_eq!(list.prepare_first_item(other), Ok(()));
}

#[test]
fn powered_task_split_retains_the_same_running_list_identity() {
    struct TaskSplitPlatform;

    let stopped = BluetoothStopped::from_hardware(
        TaskSplitPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let address = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("test item lies in Controller SRAM");
    scheduler
        ._scheduler_list
        .prepare_first_item(address)
        .expect("exclusive list starts empty");
    scheduler
        ._scheduler_list
        .retain_published_first_item(address);

    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    task.retain_running_first_item(address);
    drop((interrupt, task, modem_timer));

    assert!(
        scheduler
            ._scheduler_list
            .retains_running_first_item(address)
    );
}

#[test]
fn controller_hal_precedes_complete_scheduler_init_and_arms_fail_stop() {
    PLATFORM_DROPS.store(0, Ordering::Relaxed);
    let stopped =
        BluetoothStopped::from_hardware(FakePlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let operations = Rc::new(RefCell::new(Vec::new()));
    let hal_operations = Rc::clone(&operations);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {
        hal_operations.borrow_mut().push("controller-hal");
    });
    let time_scale = initialized.controller_time_scale();
    let scheduler_operations = Rc::clone(&operations);
    let mut scheduler = initialized.initialize_scheduler_with(
        BluetoothControllerRuntimeResources::<4, 3>::new(),
        |_| {
            scheduler_operations.borrow_mut().push("scheduler-hardware");
            BluetoothSchedulerHardwareListsCleared::for_validation()
        },
    );
    assert_eq!(
        operations.borrow().as_slice(),
        ["controller-hal", "scheduler-hardware"]
    );
    assert_eq!(scheduler.controller_time_scale(), time_scale);
    assert_eq!(
        scheduler.controller_time_phase(),
        crate::controller::time::BluetoothControllerTimeWorkerPhase::Idle
    );
    assert!(!scheduler.controller_time_needs_recheck());
    assert_eq!(scheduler.modem_timer_capacity(), 4);
    assert_eq!(scheduler.scheduler_capacity(), 3);
    assert!(scheduler.runtime_is_pristine());
    let (interrupt, task, modem_timer) = scheduler.split_runtime();
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert_eq!(
        task.controller_time_phase(),
        crate::controller::time::BluetoothControllerTimeWorkerPhase::Idle
    );
    assert!(!task.controller_time_needs_recheck());
    drop((interrupt, task, modem_timer));
    drop(scheduler);
    assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
}

#[test]
fn rejected_initial_sequence_gate_releases_the_controller_owned_reservation() {
    struct AdmissionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        AdmissionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let event = BluetoothDtmSchedulerItemEvent::new_initial_receiver(
        BluetoothDtmChannel::new(5).expect("channel five is valid"),
        BluetoothDtmPhy::Le1M,
        BluetoothDtmRxInitialEventWindow::new(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothSchedulerInstant::from_image(900),
            BluetoothSchedulerInstant::from_image(1_020),
        ),
    )
    .expect("initial receiver event is role-valid");
    let time_scale = scheduler.controller_time_scale();
    let now = BluetoothControllerSchedulerNow::from_retained_epoch(
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            time_scale,
        ),
        BluetoothControllerTimeSample::for_validation(100),
    );
    assert_eq!(
        super::dtm::dtm_scheduler_current(&now),
        BluetoothSchedulerInstant::from_image(1_000)
    );

    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let reservation = task
        .admit_initial_dtm_event(
            event,
            &now,
            BluetoothControllerTimeSample::for_validation(92),
        )
        .expect("the fresh admission sample keeps the initial deadline open");
    let result = task.finish_dtm_sequence_authorization(
        reservation.authorize_sequence(BluetoothControllerTimeSample::for_validation(1_000)),
    );

    assert_eq!(
        result.expect_err("the deliberately late second sample must fail"),
        BluetoothDtmControllerEventPreparationError::SequenceAuthorization(
            crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
        )
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn first_advertising_event_uses_common_admission_and_cancels_losslessly() {
    struct AdvertisingPlatform;

    let stopped = BluetoothStopped::from_hardware(
        AdvertisingPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let scale = scheduler.controller_time_scale();
    let config = scheduler.scheduler_config();
    let prepared = crate::BluetoothLegacyAdvertisingPrepared::prepare(
        legacy_advertiser_enabled(),
        legacy_advertising_memory(),
    )
    .expect("the bounded portable packet fits");
    let reset = match prepared
        .reset_link_state(crate::BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
    {
        crate::BluetoothLegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        crate::BluetoothLegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the portable packet selects the restricted reset")
        }
    };
    let candidate = match reset.form_first_event_candidate(
        crate::BluetoothLegacyAdvertisingTimingObservation {
            current: BluetoothSchedulerInstant::from_image(10_000),
            radio_ready: BluetoothSchedulerInstant::from_image(11_999),
            epoch: BluetoothControllerSchedulerEpoch::new(
                BluetoothControllerTimeSample::for_validation(100),
                1_000,
                scale,
            ),
        },
        config,
    ) {
        crate::BluetoothLegacyAdvertisingFirstEventCandidateOutcome::Candidate(candidate) => {
            candidate
        }
        crate::BluetoothLegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(_) => {
            panic!("the first event projects into the retained epoch")
        }
    };
    let identity = candidate.identity();
    let raw_start = candidate.raw_window().start();

    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let admitted = task
        .admit_legacy_advertising_first_event(
            candidate,
            super::BluetoothLegacyAdvertisingAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(raw_start.wrapping_sub(100)),
            },
        )
        .expect("the first guarded deadline remains open");
    let (enabled, memory) = task
        .cancel_legacy_advertising_first_pre_sequence(admitted)
        .into_parts();
    let prepared = crate::BluetoothLegacyAdvertisingPrepared::prepare(enabled, memory)
        .expect("the cancelled portable packet remains bounded");
    let reset = match prepared
        .reset_link_state(crate::BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
    {
        crate::BluetoothLegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        crate::BluetoothLegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the cancelled packet retains the restricted reset")
        }
    };
    let candidate = match reset.form_first_event_candidate(
        crate::BluetoothLegacyAdvertisingTimingObservation {
            current: BluetoothSchedulerInstant::from_image(10_000),
            radio_ready: BluetoothSchedulerInstant::from_image(11_999),
            epoch: BluetoothControllerSchedulerEpoch::new(
                BluetoothControllerTimeSample::for_validation(100),
                1_000,
                scale,
            ),
        },
        config,
    ) {
        crate::BluetoothLegacyAdvertisingFirstEventCandidateOutcome::Candidate(candidate) => {
            candidate
        }
        crate::BluetoothLegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(_) => {
            panic!("the restored first event projects into the same epoch")
        }
    };
    let raw_start = candidate.raw_window().start();
    let admitted = task
        .admit_legacy_advertising_first_event(
            candidate,
            super::BluetoothLegacyAdvertisingAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(raw_start.wrapping_sub(100)),
            },
        )
        .expect("cancellation released the first guarded reservation");
    let prepared = task
        .prepare_legacy_advertising_first_event(
            admitted,
            super::BluetoothLegacyAdvertisingSequenceObservation {
                sample: BluetoothControllerTimeSample::for_validation(raw_start.wrapping_sub(50)),
            },
        )
        .expect("the second guarded deadline remains open");
    assert_eq!(prepared.identity(), identity);
    assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);

    let merged = match task.prepare_legacy_advertising_empty_list_merge(prepared) {
        Ok(merged) => merged,
        Err(_) => panic!("the pristine exclusive list must accept the advertising item"),
    };
    let prepared = match task.cancel_legacy_advertising_empty_list_merge(merged) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the same scheduler epoch must restore the unpublished event"),
    };
    let cancelled = task.cancel_legacy_advertising_first_event(prepared);
    let (enabled, memory) = cancelled.into_parts();
    assert_eq!(enabled.prepare_event().identity(), identity);
    assert!(memory.prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6]).is_ok());
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn rejected_recurring_sequence_gate_releases_the_controller_owned_reservation() {
    struct RecurringPlatform;

    let stopped = BluetoothStopped::from_hardware(
        RecurringPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler = initialized
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::<1, 1>::new());
    let event = BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
        BluetoothDtmChannel::new(5).expect("channel five is valid"),
        BluetoothDtmPhy::Le1M,
        BluetoothDtmRxRecurringEventWindow::new(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothSchedulerInstant::from_image(900),
            BluetoothSchedulerInstant::from_image(1_020),
        ),
    )
    .expect("receiver event is role-valid");
    let time_scale = scheduler.controller_time_scale();
    let now = BluetoothControllerSchedulerNow::from_retained_epoch(
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            time_scale,
        ),
        BluetoothControllerTimeSample::for_validation(100),
    );
    assert_eq!(
        super::dtm::dtm_scheduler_current(&now),
        BluetoothSchedulerInstant::from_image(1_000)
    );

    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let reservation = task
        .reserve_recurring_dtm_event(event, &now)
        .expect("the exact recurring window is initially free");
    let result = task.finish_dtm_sequence_authorization(
        reservation.authorize_sequence(BluetoothControllerTimeSample::for_validation(1_000)),
    );

    assert_eq!(
        result.expect_err("the deliberately late sequence sample must fail"),
        BluetoothDtmControllerEventPreparationError::SequenceAuthorization(
            crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
        )
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}
