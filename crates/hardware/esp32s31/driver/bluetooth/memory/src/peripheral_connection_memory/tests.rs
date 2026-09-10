use crate::{
    DirectionFindingWorkspaceModelAddress, DirectionFindingWorkspaceStorage,
    NonScanningRxMemoryModelAddress, NonScanningRxMemoryStorage, RxMemoryListClass,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerSoftwareListRemovalReady,
    RxMemoryListPublished,
};

use super::{
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionDataChannel, PeripheralConnectionDefaultTxPowerDbm,
    PeripheralConnectionEventSpan, PeripheralConnectionIdentity,
    PeripheralConnectionMemoryGraphBindError, PeripheralConnectionMemoryGraphCompletionObservation,
    PeripheralConnectionMemoryGraphCompletionObserved, PeripheralConnectionMemoryGraphModelAddress,
    PeripheralConnectionMemoryGraphRecycleError, PeripheralConnectionMemoryGraphRunning,
    PeripheralConnectionMemoryGraphStorage, PeripheralConnectionReceiveTime,
    PeripheralConnectionReceiveWait, PeripheralConnectionRecurringReceiveWait,
    PeripheralConnectionSchedulerItemCompletionStatus, PeripheralConnectionSchedulerPriority,
    PeripheralConnectionSchedulerWindow,
};

fn storage() -> &'static mut PeripheralConnectionMemoryGraphStorage {
    std::boxed::Box::leak(std::boxed::Box::new(
        PeripheralConnectionMemoryGraphStorage::new(),
    ))
}

fn completed_graph(
    graph_base: u32,
    status: PeripheralConnectionSchedulerItemCompletionStatus,
    capture: PeripheralConnectionCapturedAnchorAvailability,
) -> PeripheralConnectionMemoryGraphCompletionObserved {
    let owner = PeripheralConnectionMemoryGraphStorage::pin_static_model(
        storage(),
        PeripheralConnectionMemoryGraphModelAddress::new(graph_base)
            .expect("the model graph address is controller-encodable"),
    )
    .expect("the connection graph fits controller SRAM");
    let receive_storage =
        std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let receive_pool = NonScanningRxMemoryStorage::pin_static_model(
        receive_storage,
        NonScanningRxMemoryModelAddress::new(graph_base + 0x1000)
            .expect("the model RX address is controller-encodable"),
    )
    .expect("the receive graph fits controller SRAM");
    let workspace_storage =
        std::boxed::Box::leak(std::boxed::Box::new(DirectionFindingWorkspaceStorage::new()));
    let workspace = DirectionFindingWorkspaceStorage::pin_static_model(
        workspace_storage,
        DirectionFindingWorkspaceModelAddress::new(graph_base + 0x2000)
            .expect("the model workspace address is controller-encodable"),
    )
    .expect("the direction-finding workspace fits controller SRAM");
    let event_span =
        PeripheralConnectionEventSpan::new(23_000).expect("the event span is nonempty");
    let prepared = owner
        .prepare_identity(PeripheralConnectionIdentity::new(
            [0xd4, 0xc3, 0xb2, 0xa1],
            [0x33, 0x22, 0x11],
        ))
        .attach_receive_pool(receive_pool)
        .prepare_reviewed_first_event_fields(
            PeripheralConnectionDataChannel::new(0).expect("data channel zero is valid"),
            PeripheralConnectionReceiveTime::from_controller_ticks(24_000),
            event_span,
            PeripheralConnectionSchedulerWindow::new(100, 200)
                .expect("the scheduler window is nonempty"),
            PeripheralConnectionReceiveWait::new(1_250, 16)
                .expect("the first receive wait fits its short form"),
            PeripheralConnectionDefaultTxPowerDbm::new(0),
            PeripheralConnectionSchedulerPriority::FIRST_EVENT,
            92,
        )
        .install_direction_finding_workspace(workspace.binding().link())
        .prepare_scheduler_admission();
    let scheduler_item_address = prepared.scheduler_head();
    assert!(
        prepared
            .prepared
            .storage()
            .model_controller_complete_event(event_span, status, capture)
    );
    let running = PeripheralConnectionMemoryGraphRunning {
        prepared: prepared.prepared,
        _rx_publication: RxMemoryListPublished::from_parts_for_validation(
            RxMemoryListClass::NonScanning.selector(),
            scheduler_item_address,
        ),
    };
    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };
    match running.observe_completion(observed) {
        PeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(completed) => {
            completed
        }
        _ => panic!("the non-sentinel status completes the model event"),
    }
}

fn removal_ready(
    index: BluetoothSchedulerHardwareListIndex,
    address: oer_esp32s31_hal::types::BluetoothControllerSramAddress,
) -> BluetoothSchedulerSoftwareListRemovalReady {
    let head = BluetoothSchedulerHardwareListHead::from_address(address)
        .expect("the connection item forms a nonempty scheduler head");
    let empty =
        BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(index, head);
    BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(empty)
}

fn active_graph(graph_base: u32) -> super::PeripheralConnectionMemoryGraphActiveCpuOwned {
    let completed = completed_graph(
        graph_base,
        PeripheralConnectionSchedulerItemCompletionStatus::Zero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    let address = completed.scheduler_item_address();
    completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            address,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof authorizes reclamation"))
        .extract_received()
        .unwrap_or_else(|_| panic!("an event without a received packet is valid"))
        .commit()
        .into_parts()
        .0
}

#[test]
fn first_sequence_preserves_the_admitted_duration_after_its_lead() {
    let completed = completed_graph(
        0x2f04_1000,
        PeripheralConnectionSchedulerItemCompletionStatus::Zero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    let graph = completed.running.prepared.storage();
    // The helper admits 100..200 and supplies a 92-tick sequence lead.
    for now in [99, 191, 192, 200, 291] {
        assert!(!graph.model_controller_sequence_elapsed(now), "now={now}");
    }
    assert!(graph.model_controller_sequence_elapsed(292));
}

#[test]
fn peripheral_publication_receives_both_successors_without_a_chain_gap() {
    let completed = completed_graph(
        0x2f05_1000,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    let pool = completed.running.prepared.receive_pool();
    let cursor = completed.running.prepared.receive_head();
    let first = [0x01, 0];
    let second = [0x02, 1, 0x55];
    let cursor = pool.model_controller_receive_after_current(cursor, &first);
    let first_batch = pool
        .extract_completed_rx_batch()
        .expect("the first packet starts the completed prefix");
    assert_eq!(first_batch.packet(0).unwrap().as_bytes(), first);
    pool.model_controller_receive_after_current(cursor, &second);
    let address = completed.scheduler_item_address();
    let recycled = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            address,
        ))
        .unwrap_or_else(|_| panic!("exact removal"))
        .extract_received()
        .unwrap_or_else(|_| panic!("both packets form a contiguous completed prefix"))
        .commit();
    let (active, batch, _, _) = recycled.into_parts();
    assert_eq!(batch.packet(0).unwrap().as_bytes(), first);
    assert_eq!(batch.packet(1).unwrap().as_bytes(), second);
    let prepared = active
        .prepare_reviewed_recurring_event_fields(
            PeripheralConnectionDataChannel::new(3).unwrap(),
            PeripheralConnectionEventSpan::new(2_000).unwrap(),
            PeripheralConnectionSchedulerWindow::new(1_000, 2_000).unwrap(),
            PeripheralConnectionRecurringReceiveWait::new(100).unwrap(),
            PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
            92,
        )
        .prepare_scheduler_admission()
        .prepare_publication();
    let pool = prepared.prepared.prepared.receive_pool();
    pool.model_controller_receive_after_current(prepared.receive_head(), &first);
    assert_eq!(
        pool.extract_completed_connection_rx_batch()
            .unwrap()
            .packet(0)
            .unwrap()
            .as_bytes(),
        first
    );
}

#[test]
fn peripheral_radio_requests_survive_recycle_and_recurring_cancellation() {
    let completed = completed_graph(
        0x2f04_9000,
        PeripheralConnectionSchedulerItemCompletionStatus::Zero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    assert!(
        completed
            .running
            .prepared
            .storage()
            .model_controller_can_request_radio()
    );
    let active = active_graph(0x2f04_d000);
    let prepared = active.prepare_reviewed_recurring_event_fields(
        PeripheralConnectionDataChannel::new(3).unwrap(),
        PeripheralConnectionEventSpan::new(2_000).unwrap(),
        PeripheralConnectionSchedulerWindow::new(1_000, 2_000).unwrap(),
        PeripheralConnectionRecurringReceiveWait::new(100).unwrap(),
        PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        92,
    );
    assert!(
        prepared
            .active
            .storage
            .as_ref()
            .get_ref()
            .model_controller_can_request_radio()
    );
    let active = prepared.prepare_scheduler_admission().cancel().cancel();
    assert!(
        active
            .storage
            .as_ref()
            .get_ref()
            .model_controller_can_request_radio()
    );
}

#[test]
fn recurring_sequence_replaces_old_timing_across_clock_wrap() {
    let active = active_graph(0x2f04_5000);
    let prepared = active.prepare_reviewed_recurring_event_fields(
        PeripheralConnectionDataChannel::new(19).unwrap(),
        PeripheralConnectionEventSpan::new(2_000).unwrap(),
        PeripheralConnectionSchedulerWindow::new(u32::MAX - 99, 100).unwrap(),
        PeripheralConnectionRecurringReceiveWait::new(100).unwrap(),
        PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        92,
    );
    let graph = prepared.active.storage.as_ref().get_ref();
    // With a 92-tick lead, the sequencer starts at MAX-7 and ends at 192.
    for now in [u32::MAX - 8, u32::MAX - 7, 0, 100, 191] {
        assert!(!graph.model_controller_sequence_elapsed(now), "now={now}");
    }
    assert!(graph.model_controller_sequence_elapsed(192));
    let active = prepared.cancel();
    let prepared = active.prepare_reviewed_recurring_event_fields(
        PeripheralConnectionDataChannel::new(7).unwrap(),
        PeripheralConnectionEventSpan::new(2_000).unwrap(),
        PeripheralConnectionSchedulerWindow::new(1_000, 1_300).unwrap(),
        PeripheralConnectionRecurringReceiveWait::new(100).unwrap(),
        PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        92,
    );
    let graph = prepared.active.storage.as_ref().get_ref();
    assert!(!graph.model_controller_sequence_elapsed(1_391));
    assert!(graph.model_controller_sequence_elapsed(1_392));
}

#[test]
fn binding_builds_the_recovered_allocation_topology() {
    let base = PeripheralConnectionMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    let owner = PeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
        .expect("the complete graph fits physical controller SRAM");

    assert!(owner.has_recovered_scheduler_pool());
    assert!(owner.has_empty_receive_queue());
    assert!(owner.has_empty_transmit_queue());
}

#[test]
fn recurring_receive_wait_rejects_lossy_or_unrepresentable_durations() {
    assert_eq!(
        PeripheralConnectionRecurringReceiveWait::new(0)
            .expect("the reviewed zero-duration form is valid")
            .total_micros(),
        0
    );
    assert_eq!(
        PeripheralConnectionRecurringReceiveWait::new(65_534)
            .expect("the largest short duration is valid")
            .total_micros(),
        65_534
    );
    assert_eq!(
        PeripheralConnectionRecurringReceiveWait::new(65_536)
            .expect("the first exact long duration is valid")
            .total_micros(),
        65_536
    );
    assert_eq!(
        PeripheralConnectionRecurringReceiveWait::new(131_070)
            .expect("the largest exact long duration is valid")
            .total_micros(),
        131_070
    );
    assert!(PeripheralConnectionRecurringReceiveWait::new(65_535).is_none());
    assert!(PeripheralConnectionRecurringReceiveWait::new(131_069).is_none());
    assert!(PeripheralConnectionRecurringReceiveWait::new(131_071).is_none());
}

#[test]
fn recurring_preparation_is_cancellable_without_replacing_persistent_owners() {
    let active = active_graph(0x2f01_1000);
    let graph_identity = active.identity();
    let receive_identity = active.receive_identity();
    let connection_identity = active.connection_identity();
    let workspace = active.direction_finding_workspace();
    let channel = PeripheralConnectionDataChannel::new(19).expect("data channel nineteen is valid");
    let event_span =
        PeripheralConnectionEventSpan::new(47_000).expect("the recurring event span is nonempty");
    let window = PeripheralConnectionSchedulerWindow::new(2_000, 3_500)
        .expect("the recurring scheduler window is nonempty");
    let receive_wait = PeripheralConnectionRecurringReceiveWait::new(70_000)
        .expect("the recurring wait is exactly representable");

    let prepared = active.prepare_reviewed_recurring_event_fields(
        channel,
        event_span,
        window,
        receive_wait,
        PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        92,
    );
    assert_eq!(prepared.channel(), channel);
    assert_eq!(prepared.event_span(), event_span);
    assert_eq!(prepared.window(), window);
    assert_eq!(prepared.receive_wait(), receive_wait);
    assert_eq!(
        prepared.priority(),
        PeripheralConnectionSchedulerPriority::RECURRING_BASELINE
    );

    let admission = prepared.prepare_scheduler_admission();
    let scheduler_head = admission.scheduler_head();
    let prepared = admission.cancel();
    assert_eq!(prepared.channel(), channel);
    assert_eq!(prepared.event_span(), event_span);
    assert_eq!(prepared.window(), window);
    assert_eq!(prepared.receive_wait(), receive_wait);
    assert_eq!(
        prepared.priority(),
        PeripheralConnectionSchedulerPriority::RECURRING_BASELINE
    );

    let repeated_admission = prepared.prepare_scheduler_admission();
    assert_eq!(repeated_admission.scheduler_head(), scheduler_head);
    let active = repeated_admission.cancel().cancel();
    assert_eq!(active.identity(), graph_identity);
    assert_eq!(active.receive_identity(), receive_identity);
    assert_eq!(active.connection_identity(), connection_identity);
    assert_eq!(active.direction_finding_workspace(), workspace);
    assert!(active.event_resources_are_recycled());
}

#[test]
fn recurring_event_converges_on_the_common_publication_and_completion_lifecycle() {
    let active = active_graph(0x2f01_5000);
    let graph_identity = active.identity();
    let receive_identity = active.receive_identity();
    let connection_identity = active.connection_identity();
    let workspace = active.direction_finding_workspace();
    let event_span =
        PeripheralConnectionEventSpan::new(41_000).expect("the recurring event span is nonempty");
    let admission = active
        .prepare_reviewed_recurring_event_fields(
            PeripheralConnectionDataChannel::new(7).expect("data channel seven is valid"),
            event_span,
            PeripheralConnectionSchedulerWindow::new(8_000, 9_250)
                .expect("the recurring scheduler window is nonempty"),
            PeripheralConnectionRecurringReceiveWait::new(8_750)
                .expect("the recurring wait has exact short representation"),
            PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
            92,
        )
        .prepare_scheduler_admission();
    let scheduler_head = admission.scheduler_head();
    let publication = admission.prepare_publication();
    let receive_head = publication.receive_head();
    let published = publication
        .into_rx_published(RxMemoryListPublished::from_parts_for_validation(
            RxMemoryListClass::NonScanning.selector(),
            receive_head,
        ))
        .unwrap_or_else(|_| panic!("the matching RX publication must join this graph"));
    assert_eq!(published.scheduler_head(), scheduler_head);
    assert!(
        published
            .prepared
            .storage()
            .model_controller_complete_event(
                event_span,
                PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
                PeripheralConnectionCapturedAnchorAvailability::Absent,
            )
    );
    let super::PeripheralConnectionMemoryGraphRxPublished {
        prepared,
        rx_publication,
    } = published;
    let running = PeripheralConnectionMemoryGraphRunning {
        prepared,
        _rx_publication: rx_publication,
    };
    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };
    let completed = match running.observe_completion(observed) {
        PeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(completed) => {
            completed
        }
        _ => panic!("the recurring event must use the common completion lifecycle"),
    };
    let recycled = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            scheduler_head,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof authorizes reclamation"))
        .extract_received()
        .unwrap_or_else(|_| panic!("an event without a received packet is valid"))
        .commit();
    let (active, batch, status, capture) = recycled.into_parts();

    assert_eq!(active.identity(), graph_identity);
    assert_eq!(active.receive_identity(), receive_identity);
    assert_eq!(active.connection_identity(), connection_identity);
    assert_eq!(active.direction_finding_workspace(), workspace);
    assert!(active.event_resources_are_recycled());
    assert!(batch.is_empty());
    assert_eq!(
        status,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
    assert_eq!(
        capture,
        PeripheralConnectionCapturedAnchorAvailability::Absent
    );
}

#[test]
fn identity_preparation_is_affine_and_cancellable() {
    let base = PeripheralConnectionMemoryGraphModelAddress::new(0x2f00_1000)
        .expect("the model base uses controller SRAM syntax");
    let owner = PeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
        .expect("the complete graph fits physical controller SRAM");
    let identity = PeripheralConnectionIdentity::new([0xd4, 0xc3, 0xb2, 0xa1], [0x33, 0x22, 0x11]);

    let prepared = owner.prepare_identity(identity);
    assert_eq!(prepared.identity(), identity);

    let owner = prepared.cancel();
    assert!(owner.has_recovered_scheduler_pool());
    assert!(owner.has_empty_receive_queue());
    assert!(owner.has_empty_transmit_queue());
}

#[test]
fn out_of_window_binding_returns_the_same_storage() {
    let storage = storage();
    let identity = core::ptr::addr_of!(*storage);
    let base = PeripheralConnectionMemoryGraphModelAddress::new(0x2f07_fff0)
        .expect("the final aligned controller SRAM address is syntactically valid");
    let failure = match PeripheralConnectionMemoryGraphStorage::pin_static_model(storage, base) {
        Ok(_) => panic!("the complete graph crosses the physical SRAM boundary"),
        Err(failure) => failure,
    };

    assert_eq!(
        failure.error(),
        PeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram
    );
    let (storage, _) = failure.into_parts();
    assert_eq!(core::ptr::addr_of!(*storage), identity);
}

#[test]
fn scheduler_status_separates_in_flight_from_opaque_completion() {
    let zero = completed_graph(
        0x2f00_3000,
        PeripheralConnectionSchedulerItemCompletionStatus::Zero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    assert_eq!(
        zero.status(),
        PeripheralConnectionSchedulerItemCompletionStatus::Zero
    );

    let nonzero = completed_graph(
        0x2f00_4000,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    assert_eq!(
        nonzero.status(),
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
}

#[test]
fn recycle_rejects_a_foreign_item_without_mutating_the_connection() {
    let completed = completed_graph(
        0x2f00_5000,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    let address = completed.scheduler_item_address();
    let foreign =
        oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(address.address() + 4)
            .expect("the adjacent model address remains controller-encodable");
    let failure = match completed.prepare_recycle_after_software_list_removal(removal_ready(
        BluetoothSchedulerHardwareListIndex::ZERO,
        foreign,
    )) {
        Ok(_) => panic!("a removal proof for another item must be rejected"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        PeripheralConnectionMemoryGraphRecycleError::SchedulerItemMismatch
    );
    let (completed, _) = failure.into_parts();
    assert_eq!(completed.scheduler_item_address(), address);
    assert_eq!(
        completed.status(),
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
}

#[test]
fn recycle_returns_an_available_capture_without_resetting_the_active_owner() {
    let captured_anchor = PeripheralConnectionCapturedAnchorTime::from_controller_sram_word(0x1234);
    let completed = completed_graph(
        0x2f00_9000,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        PeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor),
    );
    let address = completed.scheduler_item_address();
    let prepared = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            address,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof authorizes reclamation"));
    assert_eq!(
        prepared.captured_anchor_availability(),
        PeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor)
    );
    let extracted = prepared
        .extract_received()
        .unwrap_or_else(|_| panic!("an event without a received packet is valid"));
    assert!(extracted.batch().is_empty());

    let recycled = extracted.commit();
    assert_eq!(
        recycled.captured_anchor_availability(),
        PeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor)
    );
    let (active, batch, status, returned_capture) = recycled.into_parts();

    assert!(active.event_resources_are_recycled());
    assert!(batch.is_empty());
    assert_eq!(
        status,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
    assert_eq!(
        returned_capture,
        PeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor)
    );
}

#[test]
fn recycle_preserves_an_event_without_a_capture() {
    let completed = completed_graph(
        0x2f00_d000,
        PeripheralConnectionSchedulerItemCompletionStatus::Zero,
        PeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    let address = completed.scheduler_item_address();
    let prepared = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            address,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof authorizes reclamation"));
    assert_eq!(
        prepared.captured_anchor_availability(),
        PeripheralConnectionCapturedAnchorAvailability::Absent
    );

    let recycled = prepared
        .extract_received()
        .unwrap_or_else(|_| panic!("an event without a received packet is valid"))
        .commit();
    let (active, batch, status, capture) = recycled.into_parts();

    assert!(active.event_resources_are_recycled());
    assert!(batch.is_empty());
    assert_eq!(
        status,
        PeripheralConnectionSchedulerItemCompletionStatus::Zero
    );
    assert_eq!(
        capture,
        PeripheralConnectionCapturedAnchorAvailability::Absent
    );
}

#[test]
fn control_tx_retains_unacknowledged_payload_and_rotates_after_ack() {
    let mut owner = active_graph(0x2f00_4000);
    let first = [9, 0, 0, 0, 0, 0, 0, 0, 0];
    let second = [7, 0xf1];
    for _ in 0..3 {
        assert!(owner.enqueue_control_transmission(&first).unwrap());
        assert!(!owner.enqueue_control_transmission(&second).unwrap());
        let storage = owner.storage.as_ref().get_ref();
        let transmitted = storage
            .model_transmit_control(&owner.binding, false)
            .unwrap();
        assert_eq!(&transmitted[..2], &[3, 9]);
        assert_eq!(&transmitted[2..], &first);
        assert!(!owner.reclaim_control_transmission());
        let storage = owner.storage.as_ref().get_ref();
        assert_eq!(
            storage
                .model_transmit_control(&owner.binding, true)
                .unwrap(),
            transmitted
        );
        assert!(owner.reclaim_control_transmission());
        assert!(!owner.reclaim_control_transmission());
        assert!(owner.enqueue_control_transmission(&second).unwrap());
        let storage = owner.storage.as_ref().get_ref();
        assert_eq!(
            storage
                .model_transmit_control(&owner.binding, true)
                .unwrap(),
            [3, 2, 7, 0xf1]
        );
        assert!(owner.reclaim_control_transmission());
    }
}

#[test]
fn cancelling_recurring_preparation_preserves_a_pending_control_response() {
    let mut owner = active_graph(0x2f00_4000);
    let response = [9, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(owner.enqueue_control_transmission(&response).unwrap());
    let mut owner = owner
        .prepare_reviewed_recurring_event_fields(
            PeripheralConnectionDataChannel::new(3).unwrap(),
            PeripheralConnectionEventSpan::new(23_000).unwrap(),
            PeripheralConnectionSchedulerWindow::new(1000, 2000).unwrap(),
            PeripheralConnectionRecurringReceiveWait::new(1250).unwrap(),
            PeripheralConnectionSchedulerPriority::FIRST_EVENT,
            92,
        )
        .cancel();
    assert!(!owner.reclaim_control_transmission());
    assert!(!owner.enqueue_control_transmission(&[7, 0xf1]).unwrap());
    let pdu = owner
        .storage
        .as_ref()
        .get_ref()
        .model_transmit_control(&owner.binding, true)
        .unwrap();
    assert_eq!(&pdu[2..], &response);
    assert!(owner.reclaim_control_transmission());
}

#[test]
fn oversized_control_payload_leaves_empty_queue_reusable() {
    let mut owner = active_graph(0x2f00_4000);
    assert!(owner.enqueue_control_transmission(&[0; 28]).is_err());
    assert!(
        owner
            .storage
            .as_ref()
            .get_ref()
            .model_transmit_control(&owner.binding, false)
            .is_none()
    );
    assert!(owner.enqueue_control_transmission(&[7, 0xf1]).unwrap());
}

#[test]
fn retirement_cancels_pending_tx_and_reuses_the_same_allocations() {
    let mut active = active_graph(0x2f00_9000);
    let graph_identity = active.identity();
    let receive_identity = active.receive_identity();
    assert!(
        active
            .enqueue_control_transmission(&[9, 0, 0, 0, 0, 0, 0, 0, 0])
            .unwrap()
    );
    let current = active.pool.current_cursor();
    let next = active
        .pool
        .model_controller_receive_after_current(current, &[3, 2, 2, 0x13]);
    active
        .storage
        .as_ref()
        .get_ref()
        .model_advance_receive_current(next);
    active.restore_after_event();
    let (graph, pool) = active.retire();
    assert_eq!(graph.identity(), graph_identity);
    assert_eq!(pool.identity(), receive_identity);
    assert!(graph.has_empty_receive_queue());
    assert!(graph.has_empty_transmit_queue());
    assert!(graph.has_recovered_scheduler_pool());
    assert!(pool.is_initialized());
    assert!(pool.extract_completed_rx_batch().unwrap().is_empty());
    let next = graph
        .prepare_identity(PeripheralConnectionIdentity::new(
            [0x12, 0x34, 0x56, 0x78],
            [0x12, 0x34, 0x56],
        ))
        .attach_receive_pool(pool);
    assert!(next.receive_pool_is_initialized());
    let (_graph, _pool) = next.cancel();
}

#[test]
fn recurring_rx_retains_private_cursor_and_rotates_its_successor() {
    let mut owner = active_graph(0x2f00_4000);
    let first = [3, 9, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    let second = [2, 7, 3, 0, 4, 0, 2, 5, 2];
    for pdu in [&first[..], &second[..], &first[..]] {
        let current = owner.storage.as_ref().get_ref().model_receive_current();
        let next = owner
            .pool
            .model_controller_receive_after_current(current, pdu);
        owner
            .storage
            .as_ref()
            .get_ref()
            .model_advance_receive_current(next);
        let batch = owner
            .pool
            .extract_completed_connection_rx_batch()
            .expect("each new RX starts after the retained current cursor");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.packet(0).unwrap().as_bytes(), pdu);
        owner.restore_after_event();
        assert_eq!(
            owner.storage.as_ref().get_ref().model_receive_current(),
            next
        );
        assert!(owner.event_resources_are_recycled());
        assert!(
            owner
                .pool
                .extract_completed_connection_rx_batch()
                .unwrap()
                .is_empty()
        );
        owner.restore_after_event();
        assert!(
            owner
                .pool
                .extract_completed_connection_rx_batch()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn receive_time_survives_empty_rx_recycle_and_recurring_cancellation() {
    let mut active = active_graph(0x2f02_1000);
    assert_eq!(
        active.receive_time(),
        PeripheralConnectionReceiveTime::from_controller_ticks(24_000)
    );
    // A valid empty/duplicate reception may update time without delivering a PDU.
    // The model supplies that hardware observation, not CRC acceptance logic.
    for ticks in [u32::MAX - 10, 0, 150_000] {
        let time = PeripheralConnectionReceiveTime::from_controller_ticks(ticks);
        active
            .storage
            .as_ref()
            .get_ref()
            .model_controller_valid_receive(time);
        active.restore_after_event();
        assert_eq!(active.receive_time(), time);
        assert!(
            active
                .pool
                .extract_completed_connection_rx_batch()
                .unwrap()
                .is_empty()
        );
        let next = active.prepare_reviewed_recurring_event_fields(
            PeripheralConnectionDataChannel::new(12).unwrap(),
            PeripheralConnectionEventSpan::new(40_000).unwrap(),
            PeripheralConnectionSchedulerWindow::new(100_000, 120_000).unwrap(),
            PeripheralConnectionRecurringReceiveWait::new(1_000).unwrap(),
            PeripheralConnectionSchedulerPriority::FIRST_EVENT,
            92,
        );
        active = next.cancel();
        assert_eq!(active.receive_time(), time);
        active.restore_after_event();
        assert_eq!(active.receive_time(), time);
    }
}

#[test]
fn anchor_capture_alone_leaves_creation_receive_time_unchanged() {
    let completed = completed_graph(
        0x2f02_5000,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        PeripheralConnectionCapturedAnchorAvailability::Available(
            PeripheralConnectionCapturedAnchorTime::from_controller_sram_word(800_000),
        ),
    );
    let address = completed.scheduler_item_address();
    let (active, batch, _, _) = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            address,
        ))
        .unwrap_or_else(|_| panic!("matching unlink"))
        .extract_received()
        .unwrap_or_else(|_| panic!("empty RX"))
        .commit()
        .into_parts();
    assert!(batch.is_empty());
    assert_eq!(
        active.receive_time(),
        PeripheralConnectionReceiveTime::from_controller_ticks(24_000)
    );
}
