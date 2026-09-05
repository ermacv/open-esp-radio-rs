use super::*;
use crate::{BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage};
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
};

const ADVERTISER: [u8; 6] = [1, 2, 3, 4, 5, 6];
const ADV_IND_PDU: [u8; 11] = [0x60, 9, 1, 2, 3, 4, 5, 6, 2, 1, 6];
const SCAN_RESPONSE_PDU: [u8; 8] = [0x44, 6, 1, 2, 3, 4, 5, 6];

fn owner(base: u32) -> BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::new(),
    ));
    let base = BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(base)
        .expect("the model graph base belongs to controller SRAM");
    BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
        .expect("the response-capable graph fits controller SRAM")
}

fn pool(base: u32) -> BluetoothNonScanningRxMemoryCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothNonScanningRxMemoryStorage::new(),
    ));
    let base = BluetoothNonScanningRxMemoryModelAddress::new(base)
        .expect("the model RX base belongs to controller SRAM");
    BluetoothNonScanningRxMemoryStorage::pin_static_model(storage, base)
        .expect("the RX pool fits controller SRAM")
}

fn input(
    primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
) -> BluetoothLegacyConnectableAdvertisingMemoryInput<'static> {
    BluetoothLegacyConnectableAdvertisingMemoryInput::new(
        BluetoothLegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&ADV_IND_PDU, 9)
            .expect("the portable ADV_IND fits the S31 packet allocation"),
        BluetoothLegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
            &SCAN_RESPONSE_PDU,
            6,
        )
        .expect("the portable SCAN_RSP fits the S31 packet allocation"),
        BluetoothLegacyConnectableAdvertisingOwnAddress::Random(ADVERTISER),
        primary_channel,
    )
}

fn running(
    graph_base: u32,
    pool_base: u32,
) -> (
    BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
    BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    BluetoothNonScanningRxMemoryIdentity,
) {
    let owner = owner(graph_base);
    let graph_identity = owner.identity();
    let pool = pool(pool_base);
    let pool_identity = pool.identity();
    let receive_head = pool.head();
    let published = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
            pool,
            0,
        )
        .expect("the disjoint response graph is supported")
        .prepare_event_fields(6_000, 6_200)
        .expect("the pristine one-item graph accepts event fields")
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link()
        .prepare_publication()
        .into_rx_published(BluetoothRxMemoryListPublished::from_parts_for_validation(
            BluetoothRxMemoryListClass::NonScanning.selector(),
            receive_head,
        ))
        .unwrap_or_else(|_| panic!("the exact RX publication must join this graph"));
    let BluetoothLegacyConnectableAdvertisingMemoryGraphRxPublished {
        prepared,
        rx_publication,
    } = published;
    (
        BluetoothLegacyConnectableAdvertisingMemoryGraphRunning {
            prepared,
            _rx_publication: rx_publication,
        },
        graph_identity,
        pool_identity,
    )
}

fn finished_list(index: u8) -> BluetoothSchedulerFinishedHardwareListObserved {
    let observation =
        BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[index])
            .expect("the selected hardware list is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains the selected list")
    };
    observed
}

fn removal_ready(
    index: BluetoothSchedulerHardwareListIndex,
    address: BluetoothControllerSramAddress,
) -> BluetoothSchedulerSoftwareListRemovalReady {
    let head = BluetoothSchedulerHardwareListHead::from_address(address)
        .expect("the scheduler item forms a nonempty head");
    let empty =
        BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(index, head);
    BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(empty)
}

#[test]
fn response_graph_owns_both_pdus_and_receive_pool_until_cancelled() {
    let owner = owner(0x2f00_0100);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_4000);
    let pool_identity = pool.identity();
    let prepared = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
            pool,
            0,
        )
        .expect("the disjoint response graph is supported");

    assert_eq!(prepared.identity(), graph_identity);
    assert_eq!(prepared.receive_identity(), pool_identity);
    assert_eq!(prepared.adv_ind_pdu(), &ADV_IND_PDU);
    assert_eq!(prepared.scan_response_pdu(), &SCAN_RESPONSE_PDU);
    assert_eq!(prepared.post_anchor_duration().as_micros(), 156);
    assert_eq!(
        prepared.primary_channel(),
        BluetoothLegacyAdvertisingPrimaryChannel::Channel37
    );
    assert!(prepared.is_ready_for_scheduler_lowering());

    let (owner, pool) = prepared.cancel();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
}

#[test]
fn event_fields_and_common_list_preparation_cancel_in_reverse() {
    let owner = owner(0x2f00_1000);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_5000);
    let pool_identity = pool.identity();
    let prepared = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel38),
            pool,
            -4,
        )
        .expect("the disjoint response graph is supported");

    let event = prepared
        .prepare_event_fields(1_200, 1_400)
        .expect("the pristine one-item graph accepts event fields");
    let scheduler_item = event.scheduler_item_address();
    let bookkeeping = event.prepare_scheduler_bookkeeping();
    assert_eq!(bookkeeping.scheduler_item_address(), scheduler_item);
    let empty = bookkeeping.prepare_empty_list_link();
    assert_eq!(empty.scheduler_item_address(), scheduler_item);
    let publication = empty.prepare_publication();
    assert_eq!(publication.identity(), graph_identity);
    assert_eq!(publication.receive_identity(), pool_identity);
    assert_eq!(publication.scheduler_head(), scheduler_item);

    let empty = publication.cancel();
    let bookkeeping = empty.cancel();
    let event = bookkeeping.cancel();
    let prepared = event.cancel();
    assert!(prepared.is_ready_for_scheduler_lowering());
    assert_eq!(prepared.identity(), graph_identity);
    assert_eq!(prepared.receive_identity(), pool_identity);
    assert_eq!(prepared.adv_ind_pdu(), &ADV_IND_PDU);
    assert_eq!(prepared.scan_response_pdu(), &SCAN_RESPONSE_PDU);

    let (owner, pool) = prepared.cancel();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
}

#[test]
fn matching_rx_publication_surrenders_cpu_rollback_and_retains_identities() {
    let owner = owner(0x2f00_2400);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_6400);
    let pool_identity = pool.identity();
    let receive_head = pool.head();
    let publication = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
            pool,
            -2,
        )
        .expect("the disjoint response graph is supported")
        .prepare_event_fields(3_000, 3_200)
        .expect("the pristine one-item graph accepts event fields")
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link()
        .prepare_publication();
    let scheduler_head = publication.scheduler_head();
    assert_eq!(publication.identity(), graph_identity);
    assert_eq!(publication.receive_identity(), pool_identity);

    let published = publication
        .into_rx_published(BluetoothRxMemoryListPublished::from_parts_for_validation(
            BluetoothRxMemoryListClass::NonScanning.selector(),
            receive_head,
        ))
        .unwrap_or_else(|_| panic!("the exact RX publication must join this graph"));
    assert_eq!(published.scheduler_head(), scheduler_head);
    assert_eq!(
        published.rx_publication().selector(),
        BluetoothRxMemoryListClass::NonScanning.selector()
    );
    assert_eq!(published.rx_publication().head(), receive_head);
}

#[test]
fn completion_requires_list_zero_and_a_non_sentinel_item_status() {
    let (running, _, _) = running(0x2f00_3000, 0x2f00_7000);
    let scheduler_item = running.scheduler_item_address();
    let running = match running.observe_completion(finished_list(0)) {
        BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::StillInFlight(
            running,
        ) => running,
        _ => panic!("the in-flight sentinel must retain hardware ownership"),
    };
    running.model_controller_completion(
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
    );

    let running = match running.observe_completion(finished_list(1)) {
        BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::ListMismatch {
            running,
            observed,
        } => {
            assert_ne!(observed.index(), BluetoothSchedulerHardwareListIndex::ZERO);
            running
        }
        _ => panic!("an unrelated finished list must retain both owners"),
    };
    assert_eq!(running.scheduler_item_address(), scheduler_item);

    let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the matching finished list may consume a non-sentinel status"),
        };
    assert_eq!(completed.scheduler_item_address(), scheduler_item);
    assert_eq!(
        completed.status(),
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
    );
}

#[test]
fn matching_removal_extracts_then_rearms_both_cpu_owned_graphs() {
    let (running, graph_identity, pool_identity) = running(0x2f00_3400, 0x2f00_7400);
    let scheduler_item = running.scheduler_item_address();
    running.model_controller_completion(
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero,
    );
    let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled non-sentinel status completes the item"),
        };
    let extracted = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            scheduler_item,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof must authorize RX extraction"))
        .extract_received()
        .unwrap_or_else(|_| panic!("an event without a received PDU is a valid empty batch"));
    assert!(extracted.batch().is_empty());

    let (owner, pool, batch, status) = extracted.commit().into_parts();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
    assert!(batch.is_empty());
    assert_eq!(
        status,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero
    );
}

#[test]
fn copied_receive_batch_is_admitted_to_role_dispatch_after_reclamation() {
    let (running, graph_identity, pool_identity) = running(0x2f00_3500, 0x2f00_7500);
    let scheduler_item = running.scheduler_item_address();
    let received = [0x03, 6, 1, 2, 3, 4, 5, 6];
    running.model_controller_receive(0, &received, -31, 12_345);
    running.model_controller_completion(
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
    );
    let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled event must complete"),
        };
    let recycled = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            scheduler_item,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof must authorize reclamation"))
        .extract_received()
        .unwrap_or_else(|_| panic!("the completed receive node must be extractable"))
        .commit();

    let dispatch = recycled
        .prepare_rx_dispatch()
        .unwrap_or_else(|_| panic!("every completed observation retains its PDU"));
    assert_eq!(dispatch.batch().len(), 1);
    assert_eq!(
        dispatch.batch().packet(0).map(|packet| packet.as_bytes()),
        Some(received.as_slice())
    );
    let (owner, pool, _, status) = dispatch.into_parts();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
    assert_eq!(
        status,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
    );
}

#[test]
fn discarded_receive_observation_cannot_become_no_connection() {
    let (running, graph_identity, pool_identity) = running(0x2f00_3600, 0x2f00_7600);
    let scheduler_item = running.scheduler_item_address();
    running.model_controller_discard(0);
    running.model_controller_completion(
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero,
    );
    let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled event must complete"),
        };
    let recycled = completed
        .prepare_recycle_after_software_list_removal(removal_ready(
            BluetoothSchedulerHardwareListIndex::ZERO,
            scheduler_item,
        ))
        .unwrap_or_else(|_| panic!("the exact removal proof must authorize reclamation"))
        .extract_received()
        .unwrap_or_else(|_| panic!("a hardware discard is a bounded receive observation"))
        .commit();

    let blocked = match recycled.prepare_rx_dispatch() {
        Ok(_) => panic!("missing PDU bytes must not reach role dispatch"),
        Err(blocked) => blocked,
    };
    assert_eq!(blocked.discarded_count(), 1);
    let recycled = blocked.into_recycled();
    assert_eq!(recycled.identity(), graph_identity);
    assert_eq!(recycled.receive_identity(), pool_identity);
}

#[test]
fn removal_list_mismatch_retains_the_completed_graph_and_proof() {
    let (running, _, _) = running(0x2f00_3800, 0x2f00_7800);
    let scheduler_item = running.scheduler_item_address();
    running.model_controller_completion(
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
    );
    let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled non-sentinel status completes the item"),
        };
    let another_list = BluetoothSchedulerHardwareListIndex::new(1)
        .expect("the second scheduler list is representable");

    let failure = match completed
        .prepare_recycle_after_software_list_removal(removal_ready(another_list, scheduler_item))
    {
        Ok(_) => panic!("another scheduler list must not authorize reclamation"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError::HardwareListMismatch
    );
    let (completed, removal) = failure.into_parts();
    assert_eq!(completed.scheduler_item_address(), scheduler_item);
    assert_eq!(removal.index(), another_list);
}

#[test]
fn removal_head_mismatch_retains_the_completed_graph_and_proof() {
    let (running, _, _) = running(0x2f00_3c00, 0x2f00_7c00);
    let scheduler_item = running.scheduler_item_address();
    running.model_controller_completion(
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
    );
    let completed = match running.observe_completion(finished_list(0)) {
            BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                completed,
            ) => completed,
            _ => panic!("the modeled non-sentinel status completes the item"),
        };
    let foreign = owner(0x2f00_8000);
    let foreign_item = foreign.binding.scheduler_item_address();

    let failure = match completed.prepare_recycle_after_software_list_removal(removal_ready(
        BluetoothSchedulerHardwareListIndex::ZERO,
        foreign_item,
    )) {
        Ok(_) => panic!("another scheduler item must not authorize reclamation"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothLegacyConnectableAdvertisingMemoryGraphRecycleError::SchedulerItemMismatch
    );
    let (completed, removal) = failure.into_parts();
    assert_eq!(completed.scheduler_item_address(), scheduler_item);
    assert_eq!(removal.completed_head().address(), Some(foreign_item));
    assert_ne!(completed.scheduler_item_address(), foreign_item);
}

#[test]
fn selector_mismatch_retains_publication_and_all_cpu_rollback_authority() {
    let owner = owner(0x2f00_2800);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_6800);
    let pool_identity = pool.identity();
    let receive_head = pool.head();
    let publication = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel38),
            pool,
            1,
        )
        .expect("the disjoint response graph is supported")
        .prepare_event_fields(4_000, 4_200)
        .expect("the pristine one-item graph accepts event fields")
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link()
        .prepare_publication();
    let mismatched = BluetoothRxMemoryListPublished::from_parts_for_validation(
        BluetoothRxMemoryListClass::Scanning.selector(),
        receive_head,
    );

    let mismatch = match publication.into_rx_published(mismatched) {
        Ok(_) => panic!("a scanner-list publication must not join this graph"),
        Err(mismatch) => mismatch,
    };
    assert_eq!(
        mismatch.error(),
        BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError::SelectorMismatch
    );
    let (publication, mismatched) = mismatch.into_parts();
    assert_eq!(publication.identity(), graph_identity);
    assert_eq!(publication.receive_identity(), pool_identity);
    assert_eq!(mismatched.head(), receive_head);
    assert_eq!(
        mismatched.selector(),
        BluetoothRxMemoryListClass::Scanning.selector()
    );

    let prepared = publication.cancel().cancel().cancel().cancel();
    let (owner, pool) = prepared.cancel();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
}

#[test]
fn receive_head_mismatch_retains_both_affine_owners() {
    let owner = owner(0x2f00_2c00);
    let graph_identity = owner.identity();
    let receive_pool = pool(0x2f00_6c00);
    let pool_identity = receive_pool.identity();
    let other_pool = pool(0x2f00_7400);
    let other_head = other_pool.head();
    let publication = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel39),
            receive_pool,
            2,
        )
        .expect("the disjoint response graph is supported")
        .prepare_event_fields(5_000, 5_200)
        .expect("the pristine one-item graph accepts event fields")
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link()
        .prepare_publication();
    let mismatched = BluetoothRxMemoryListPublished::from_parts_for_validation(
        BluetoothRxMemoryListClass::NonScanning.selector(),
        other_head,
    );

    let mismatch = match publication.into_rx_published(mismatched) {
        Ok(_) => panic!("another receive pool must not join this graph"),
        Err(mismatch) => mismatch,
    };
    assert_eq!(
        mismatch.error(),
        BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError::HeadMismatch
    );
    let (publication, mismatched) = mismatch.into_parts();
    assert_eq!(publication.identity(), graph_identity);
    assert_eq!(publication.receive_identity(), pool_identity);
    assert_eq!(mismatched.head(), other_head);

    let prepared = publication.cancel().cancel().cancel().cancel();
    let (owner, pool) = prepared.cancel();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
    assert!(other_pool.is_initialized());
}

#[test]
fn event_field_rejection_retains_both_affine_owners() {
    let owner = owner(0x2f00_2000);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_6000);
    let pool_identity = pool.identity();
    let prepared = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel39),
            pool,
            0,
        )
        .expect("the disjoint response graph is supported");
    prepared
        .storage
        .as_ref()
        .get_ref()
        .graph
        .emulate_missing_scheduler_head();

    let failure = match prepared.prepare_event_fields(2_000, 2_200) {
        Ok(_) => panic!("a graph without its private scheduler head must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError::SchedulerHeadMismatch
        );
    let (prepared, error) = failure.into_parts();
    assert_eq!(
            error,
            BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError::SchedulerHeadMismatch
        );
    assert_eq!(prepared.identity(), graph_identity);
    assert_eq!(prepared.receive_identity(), pool_identity);

    let (owner, pool) = prepared.cancel();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
}

#[test]
fn packet_fit_is_proved_without_interpreting_protocol_headers() {
    let oversized = [0; 40];
    assert_eq!(
        BluetoothLegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&oversized, 38),
        Err(
            BluetoothLegacyConnectableAdvertisingPduFitError::PayloadExceedsAllocation {
                payload_bytes: 38,
                capacity: 37,
            }
        )
    );
    assert_eq!(
        BluetoothLegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
            &[0xaa, 0xbb, 0xcc],
            2,
        ),
        Err(
            BluetoothLegacyConnectableAdvertisingPduFitError::EncodedExtentMismatch {
                expected_bytes: 4,
                actual_bytes: 3,
            }
        )
    );
}

#[test]
fn missing_rx_consumer_link_blocks_lowering_and_cancel_recovers_both_owners() {
    let owner = owner(0x2f00_1800);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_5800);
    let pool_identity = pool.identity();
    let prepared = owner
        .prepare_response_capable_event(
            input(BluetoothLegacyAdvertisingPrimaryChannel::Channel39),
            pool,
            0,
        )
        .expect("the complete response topology is initially ready");
    prepared
        .storage
        .as_ref()
        .get_ref()
        .graph
        .emulate_missing_rx_consumer_link();

    assert!(!prepared.is_ready_for_scheduler_lowering());
    let (owner, pool) = prepared.cancel();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
}

#[test]
fn overlapping_rx_pool_is_rejected_without_losing_either_owner() {
    let owner = owner(0x2f00_6000);
    let graph_identity = owner.identity();
    let pool = pool(0x2f00_6000);
    let pool_identity = pool.identity();
    let failure = match owner.prepare_response_capable_event(
        input(BluetoothLegacyAdvertisingPrimaryChannel::Channel37),
        pool,
        0,
    ) {
        Ok(_) => panic!("overlapping controller-memory extents must fail closed"),
        Err(failure) => failure,
    };

    assert_eq!(
        failure.error(),
        BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph
    );
    let (owner, pool, _) = failure.into_parts();
    assert_eq!(owner.identity(), graph_identity);
    assert_eq!(pool.identity(), pool_identity);
    assert!(pool.is_initialized());
}
