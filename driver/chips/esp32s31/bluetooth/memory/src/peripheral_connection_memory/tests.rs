use open_esp_radio_esp32s31_hal::{
    BluetoothRxMemoryListPublished, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use super::{
    BluetoothPeripheralConnectionCapturedAnchorAvailability,
    BluetoothPeripheralConnectionCapturedAnchorTime, BluetoothPeripheralConnectionDataChannel,
    BluetoothPeripheralConnectionDefaultTxPowerDbm, BluetoothPeripheralConnectionEventSpan,
    BluetoothPeripheralConnectionIdentity, BluetoothPeripheralConnectionIntervalTicks,
    BluetoothPeripheralConnectionMemoryGraphBindError,
    BluetoothPeripheralConnectionMemoryGraphCompletionObservation,
    BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
    BluetoothPeripheralConnectionMemoryGraphModelAddress,
    BluetoothPeripheralConnectionMemoryGraphRecycleError,
    BluetoothPeripheralConnectionMemoryGraphRunning,
    BluetoothPeripheralConnectionMemoryGraphStorage, BluetoothPeripheralConnectionReceiveWait,
    BluetoothPeripheralConnectionRecurringReceiveWait,
    BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    BluetoothPeripheralConnectionSchedulerPriority, BluetoothPeripheralConnectionSchedulerWindow,
};
use crate::{
    BluetoothDirectionFindingWorkspaceModelAddress, BluetoothDirectionFindingWorkspaceStorage,
    BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
    BluetoothRxMemoryListClass,
};

fn storage() -> &'static mut BluetoothPeripheralConnectionMemoryGraphStorage {
    std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPeripheralConnectionMemoryGraphStorage::new(),
    ))
}

fn completed_graph(
    graph_base: u32,
    status: BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    capture: BluetoothPeripheralConnectionCapturedAnchorAvailability,
) -> BluetoothPeripheralConnectionMemoryGraphCompletionObserved {
    let owner = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(
        storage(),
        BluetoothPeripheralConnectionMemoryGraphModelAddress::new(graph_base)
            .expect("the model graph address is controller-encodable"),
    )
    .expect("the connection graph fits controller SRAM");
    let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothNonScanningRxMemoryStorage::new(),
    ));
    let receive_pool = BluetoothNonScanningRxMemoryStorage::pin_static_model(
        receive_storage,
        BluetoothNonScanningRxMemoryModelAddress::new(graph_base + 0x1000)
            .expect("the model RX address is controller-encodable"),
    )
    .expect("the receive graph fits controller SRAM");
    let workspace_storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothDirectionFindingWorkspaceStorage::new(),
    ));
    let workspace = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(
        workspace_storage,
        BluetoothDirectionFindingWorkspaceModelAddress::new(graph_base + 0x2000)
            .expect("the model workspace address is controller-encodable"),
    )
    .expect("the direction-finding workspace fits controller SRAM");
    let event_span =
        BluetoothPeripheralConnectionEventSpan::new(23_000).expect("the event span is nonempty");
    let prepared = owner
        .prepare_identity(BluetoothPeripheralConnectionIdentity::new(
            [0xd4, 0xc3, 0xb2, 0xa1],
            [0x33, 0x22, 0x11],
        ))
        .attach_receive_pool(receive_pool)
        .prepare_reviewed_first_event_fields(
            BluetoothPeripheralConnectionDataChannel::new(0).expect("data channel zero is valid"),
            BluetoothPeripheralConnectionIntervalTicks::new(24_000)
                .expect("the connection interval is nonzero"),
            event_span,
            BluetoothPeripheralConnectionSchedulerWindow::new(100, 200)
                .expect("the scheduler window is nonempty"),
            BluetoothPeripheralConnectionReceiveWait::new(1_250, 16)
                .expect("the first receive wait fits its short form"),
            BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
            BluetoothPeripheralConnectionSchedulerPriority::FIRST_EVENT,
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
    let running = BluetoothPeripheralConnectionMemoryGraphRunning {
        prepared: prepared.prepared,
        _rx_publication: BluetoothRxMemoryListPublished::from_parts_for_validation(
            BluetoothRxMemoryListClass::NonScanning.selector(),
            scheduler_item_address,
        ),
    };
    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };
    match running.observe_completion(observed) {
        BluetoothPeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(
            completed,
        ) => completed,
        _ => panic!("the non-sentinel status completes the model event"),
    }
}

fn removal_ready(
    index: BluetoothSchedulerHardwareListIndex,
    address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
) -> BluetoothSchedulerSoftwareListRemovalReady {
    let head = BluetoothSchedulerHardwareListHead::from_address(address)
        .expect("the connection item forms a nonempty scheduler head");
    let empty =
        BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(index, head);
    BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(empty)
}

fn active_graph(graph_base: u32) -> super::BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned {
    let completed = completed_graph(
        graph_base,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent,
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
    let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    let owner = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
        .expect("the complete graph fits physical controller SRAM");

    assert!(owner.has_recovered_scheduler_pool());
    assert!(owner.has_empty_receive_queue());
    assert!(owner.has_empty_transmit_queue());
}

#[test]
fn recurring_receive_wait_rejects_lossy_or_unrepresentable_durations() {
    assert_eq!(
        BluetoothPeripheralConnectionRecurringReceiveWait::new(0)
            .expect("the reviewed zero-duration form is valid")
            .total_micros(),
        0
    );
    assert_eq!(
        BluetoothPeripheralConnectionRecurringReceiveWait::new(65_534)
            .expect("the largest short duration is valid")
            .total_micros(),
        65_534
    );
    assert_eq!(
        BluetoothPeripheralConnectionRecurringReceiveWait::new(65_536)
            .expect("the first exact long duration is valid")
            .total_micros(),
        65_536
    );
    assert_eq!(
        BluetoothPeripheralConnectionRecurringReceiveWait::new(131_070)
            .expect("the largest exact long duration is valid")
            .total_micros(),
        131_070
    );
    assert!(BluetoothPeripheralConnectionRecurringReceiveWait::new(65_535).is_none());
    assert!(BluetoothPeripheralConnectionRecurringReceiveWait::new(131_069).is_none());
    assert!(BluetoothPeripheralConnectionRecurringReceiveWait::new(131_071).is_none());
}

#[test]
fn recurring_preparation_is_cancellable_without_replacing_persistent_owners() {
    let active = active_graph(0x2f01_1000);
    let graph_identity = active.identity();
    let receive_identity = active.receive_identity();
    let connection_identity = active.connection_identity();
    let workspace = active.direction_finding_workspace();
    let channel =
        BluetoothPeripheralConnectionDataChannel::new(19).expect("data channel nineteen is valid");
    let event_span = BluetoothPeripheralConnectionEventSpan::new(47_000)
        .expect("the recurring event span is nonempty");
    let window = BluetoothPeripheralConnectionSchedulerWindow::new(2_000, 3_500)
        .expect("the recurring scheduler window is nonempty");
    let receive_wait = BluetoothPeripheralConnectionRecurringReceiveWait::new(70_000)
        .expect("the recurring wait is exactly representable");

    let prepared = active.prepare_reviewed_recurring_event_fields(
        channel,
        event_span,
        window,
        receive_wait,
        BluetoothPeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
    );
    assert_eq!(prepared.channel(), channel);
    assert_eq!(prepared.event_span(), event_span);
    assert_eq!(prepared.window(), window);
    assert_eq!(prepared.receive_wait(), receive_wait);
    assert_eq!(
        prepared.priority(),
        BluetoothPeripheralConnectionSchedulerPriority::RECURRING_BASELINE
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
        BluetoothPeripheralConnectionSchedulerPriority::RECURRING_BASELINE
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
    let event_span = BluetoothPeripheralConnectionEventSpan::new(41_000)
        .expect("the recurring event span is nonempty");
    let admission = active
        .prepare_reviewed_recurring_event_fields(
            BluetoothPeripheralConnectionDataChannel::new(7).expect("data channel seven is valid"),
            event_span,
            BluetoothPeripheralConnectionSchedulerWindow::new(8_000, 9_250)
                .expect("the recurring scheduler window is nonempty"),
            BluetoothPeripheralConnectionRecurringReceiveWait::new(8_750)
                .expect("the recurring wait has exact short representation"),
            BluetoothPeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        )
        .prepare_scheduler_admission();
    let scheduler_head = admission.scheduler_head();
    let publication = admission.prepare_publication();
    let receive_head = publication.receive_head();
    let published = publication
        .into_rx_published(BluetoothRxMemoryListPublished::from_parts_for_validation(
            BluetoothRxMemoryListClass::NonScanning.selector(),
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
                BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero,
                BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent,
            )
    );
    let super::BluetoothPeripheralConnectionMemoryGraphRxPublished {
        prepared,
        rx_publication,
    } = published;
    let running = BluetoothPeripheralConnectionMemoryGraphRunning {
        prepared,
        _rx_publication: rx_publication,
    };
    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };
    let completed = match running.observe_completion(observed) {
        BluetoothPeripheralConnectionMemoryGraphCompletionObservation::CompletionObserved(
            completed,
        ) => completed,
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
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
    assert_eq!(
        capture,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent
    );
}

#[test]
fn identity_preparation_is_affine_and_cancellable() {
    let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_1000)
        .expect("the model base uses controller SRAM syntax");
    let owner = BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage(), base)
        .expect("the complete graph fits physical controller SRAM");
    let identity =
        BluetoothPeripheralConnectionIdentity::new([0xd4, 0xc3, 0xb2, 0xa1], [0x33, 0x22, 0x11]);

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
    let base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f07_fff0)
        .expect("the final aligned controller SRAM address is syntactically valid");
    let failure =
        match BluetoothPeripheralConnectionMemoryGraphStorage::pin_static_model(storage, base) {
            Ok(_) => panic!("the complete graph crosses the physical SRAM boundary"),
            Err(failure) => failure,
        };

    assert_eq!(
        failure.error(),
        BluetoothPeripheralConnectionMemoryGraphBindError::ExtentOutsidePhysicalSram
    );
    let (storage, _) = failure.into_parts();
    assert_eq!(core::ptr::addr_of!(*storage), identity);
}

#[test]
fn scheduler_status_separates_in_flight_from_opaque_completion() {
    let zero = completed_graph(
        0x2f00_3000,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    assert_eq!(
        zero.status(),
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero
    );

    let nonzero = completed_graph(
        0x2f00_4000,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    assert_eq!(
        nonzero.status(),
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
}

#[test]
fn recycle_rejects_a_foreign_item_without_mutating_the_connection() {
    let completed = completed_graph(
        0x2f00_5000,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent,
    );
    let address = completed.scheduler_item_address();
    let foreign =
        open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(address.address() + 4)
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
        BluetoothPeripheralConnectionMemoryGraphRecycleError::SchedulerItemMismatch
    );
    let (completed, _) = failure.into_parts();
    assert_eq!(completed.scheduler_item_address(), address);
    assert_eq!(
        completed.status(),
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
}

#[test]
fn recycle_returns_an_available_capture_without_resetting_the_active_owner() {
    let captured_anchor =
        BluetoothPeripheralConnectionCapturedAnchorTime::from_controller_sram_word(0x1234);
    let completed = completed_graph(
        0x2f00_9000,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor),
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
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor)
    );
    let extracted = prepared
        .extract_received()
        .unwrap_or_else(|_| panic!("an event without a received packet is valid"));
    assert!(extracted.batch().is_empty());

    let recycled = extracted.commit();
    assert_eq!(
        recycled.captured_anchor_availability(),
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor)
    );
    let (active, batch, status, returned_capture) = recycled.into_parts();

    assert!(active.event_resources_are_recycled());
    assert!(batch.is_empty());
    assert_eq!(
        status,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
    assert_eq!(
        returned_capture,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Available(captured_anchor)
    );
}

#[test]
fn recycle_preserves_an_event_without_a_capture() {
    let completed = completed_graph(
        0x2f00_d000,
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent,
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
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent
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
        BluetoothPeripheralConnectionSchedulerItemCompletionStatus::Zero
    );
    assert_eq!(
        capture,
        BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent
    );
}
