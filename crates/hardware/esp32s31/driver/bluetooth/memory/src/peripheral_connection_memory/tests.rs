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
    PeripheralConnectionEventSpan, PeripheralConnectionIdentity, PeripheralConnectionIntervalTicks,
    PeripheralConnectionMemoryGraphBindError, PeripheralConnectionMemoryGraphCompletionObservation,
    PeripheralConnectionMemoryGraphCompletionObserved, PeripheralConnectionMemoryGraphModelAddress,
    PeripheralConnectionMemoryGraphRecycleError, PeripheralConnectionMemoryGraphRunning,
    PeripheralConnectionMemoryGraphStorage, PeripheralConnectionReceiveWait,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
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
            PeripheralConnectionIntervalTicks::new(24_000)
                .expect("the connection interval is nonzero"),
            event_span,
            PeripheralConnectionSchedulerWindow::new(100, 200)
                .expect("the scheduler window is nonempty"),
            PeripheralConnectionReceiveWait::new(1_250, 16)
                .expect("the first receive wait fits its short form"),
            PeripheralConnectionDefaultTxPowerDbm::new(0),
            PeripheralConnectionSchedulerPriority::FIRST_EVENT,
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
