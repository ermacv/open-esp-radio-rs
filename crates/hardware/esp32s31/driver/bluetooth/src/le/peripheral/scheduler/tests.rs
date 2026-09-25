//! Role scheduler scenarios over the validation hardware runtime.
#[allow(unused_imports)]
use crate::le::peripheral::scheduler::PeripheralConnectionScheduling;

use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample,
    clock::ClockedResources,
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
    scheduler::BluetoothSchedulerHardwareListIndex,
};

use oer_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest,
    LePeripheralConnection,
};

use oer_esp32s31_bluetooth_memory::{
    DirectionFindingWorkspaceLink, DirectionFindingWorkspaceModelAddress,
    DirectionFindingWorkspaceStorage, NonScanningRxMemoryModelAddress, NonScanningRxMemoryStorage,
    PeripheralConnectionDefaultTxPowerDbm, PeripheralConnectionMemoryGraphModelAddress,
    PeripheralConnectionMemoryGraphStorage,
};

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use crate::scheduler::core::SchedulerEmptyListMergeError;

fn peripheral_connection_candidate() -> (
    crate::le::peripheral::PeripheralConnectionRuntimeResources,
    crate::le::peripheral::connection::PeripheralConnectionFirstEventCandidate,
    DirectionFindingWorkspaceLink,
) {
    let graph_storage = std::boxed::Box::leak(std::boxed::Box::new(
        PeripheralConnectionMemoryGraphStorage::new(),
    ));
    let receive_storage =
        std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let graph_base = PeripheralConnectionMemoryGraphModelAddress::new(0x2f00_3000)
        .expect("the model connection graph base is valid");
    let receive_base = NonScanningRxMemoryModelAddress::new(0x2f00_5000)
        .expect("the model receive-pool base is valid");
    let mut runtime =
        crate::le::peripheral::PeripheralConnectionRuntimeResources::claim_static_model(
            graph_storage,
            graph_base,
            receive_storage,
            receive_base,
            crate::le::peripheral::PeripheralConnectionRuntimeConfig::new(
                PeripheralConnectionDefaultTxPowerDbm::new(0),
            ),
        )
        .expect("the connection graph and receive pool fit controller SRAM");
    let request = LeLegacyConnectionRequest::decode(&connection_request())
        .expect("the fixed CONNECT_IND is valid");
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch =
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(300), 20_000, scale);
    let candidate = runtime
        .begin_event()
        .expect("the sole connection allocation starts idle")
        .prepare_first_event(
            LePeripheralConnection::from_request(
                request,
                oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
            ),
            crate::le::peripheral::Le1MPacketStartTiming::from_scheduler_micros(21_000),
        )
        .project_scheduler_window(
            epoch,
            crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
            &ControllerTimeSample::for_validation(40_000),
        )
        .unwrap_or_else(|_| panic!("the fixed first connection window projects"));

    let workspace_storage =
        std::boxed::Box::leak(std::boxed::Box::new(DirectionFindingWorkspaceStorage::new()));
    let workspace_base = DirectionFindingWorkspaceModelAddress::new(0x2f00_7000)
        .expect("the model direction-finding workspace base is valid");
    let workspace =
        DirectionFindingWorkspaceStorage::pin_static_model(workspace_storage, workspace_base)
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

#[test]
fn connection_pre_sequence_cancellation_releases_the_timeline() {
    struct ConnectionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        ConnectionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let (mut connection_runtime, candidate, _) = peripheral_connection_candidate();
    let admission_sample = candidate.requested_window().start().wrapping_sub(1_000);
    let admitted = task
        .admit_peripheral_connection_first_event(
            candidate,
            crate::le::peripheral::PeripheralConnectionAdmissionObservation {
                sample: ControllerTimeSample::for_validation(admission_sample),
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
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let (mut connection_runtime, candidate, workspace) = peripheral_connection_candidate();
    let requested = candidate.requested_window();
    let admitted = task
        .admit_peripheral_connection_first_event(
            candidate,
            crate::le::peripheral::PeripheralConnectionAdmissionObservation {
                sample: ControllerTimeSample::for_validation(requested.start().wrapping_sub(1_000)),
            },
        )
        .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
    let event = task
        .prepare_peripheral_connection_first_event(
            admitted,
            crate::le::peripheral::PeripheralConnectionSequenceObservation {
                sample: ControllerTimeSample::for_validation(requested.start().wrapping_sub(500)),
            },
            PeripheralConnectionDefaultTxPowerDbm::new(0),
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
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let (mut connection_runtime, candidate, _) = peripheral_connection_candidate();
    let requested = candidate.requested_window();
    let blocker = task
        .runtime
        .scheduler_timeline_mut()
        .reserve_initial_window(
            requested.start(),
            requested.end(),
            crate::scheduler::SchedulerTimingPolicy::from_scheduler_config(
                task.config,
                task.time_scale,
            ),
            ControllerTimeSample::for_validation(requested.start().wrapping_sub(1_000)),
        )
        .expect("the pristine timeline accepts the blocking window");
    let failure = match task.admit_peripheral_connection_first_event(
        candidate,
        crate::le::peripheral::PeripheralConnectionAdmissionObservation {
            sample: ControllerTimeSample::for_validation(requested.start().wrapping_sub(1_000)),
        },
    ) {
        Ok(_) => panic!("the occupied timeline must reject the connection window"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        crate::le::peripheral::PeripheralConnectionFirstEventPreparationError::Timeline(
            crate::scheduler::SchedulerReservationError::TimelineFull,
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
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let (mut connection_runtime, candidate, workspace) = peripheral_connection_candidate();
    let requested = candidate.requested_window();
    let admitted = task
        .admit_peripheral_connection_first_event(
            candidate,
            crate::le::peripheral::PeripheralConnectionAdmissionObservation {
                sample: ControllerTimeSample::for_validation(requested.start().wrapping_sub(1_000)),
            },
        )
        .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
    let event = task
        .prepare_peripheral_connection_first_event(
            admitted,
            crate::le::peripheral::PeripheralConnectionSequenceObservation {
                sample: ControllerTimeSample::for_validation(requested.start().wrapping_sub(500)),
            },
            PeripheralConnectionDefaultTxPowerDbm::new(0),
            workspace,
        )
        .unwrap_or_else(|_| panic!("the second connection deadline must remain open"));
    let occupied = oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("the occupying item lies in controller SRAM");
    task._scheduler_list
        .prepare_first_item(occupied)
        .expect("the common list starts empty");

    let failure = match task.prepare_peripheral_connection_empty_list_merge(event) {
        Ok(_) => panic!("the occupied common list must reject the connection item"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), SchedulerEmptyListMergeError::ListNotEmpty);
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
