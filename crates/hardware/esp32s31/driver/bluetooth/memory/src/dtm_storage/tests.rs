use core::{cell::Cell, convert::Infallible, fmt::Debug};

use crate::{
    dtm_event_image::DtmRole,
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_packet::BLUETOOTH_LE_BUFFER_HEADER_BYTES,
    scheduler_context::BLUETOOTH_SCHEDULER_CONTEXT_BYTES,
    sram_link::BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH,
};

use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
        BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerSoftwareListRemovalReady,
    },
    types::BluetoothControllerSramAddress,
};

use super::{
    BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
    BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_DTM_TX_PACKET_BYTES, DtmMemoryGraphCompletionObservation, DtmMemoryGraphCpuOwned,
    DtmMemoryGraphModelAddress, DtmMemoryGraphPrepareError, DtmMemoryGraphRecycleCleaned,
    DtmMemoryGraphRecycleError, DtmMemoryGraphRecyclePrepared, DtmMemoryGraphRunning,
    DtmMemoryGraphRxSuccessObserved, DtmMemoryGraphRxSuccessRecycleError, DtmMemoryGraphStorage,
    DtmPositionalEventSeed, DtmPositionalEventWords, DtmRxResultProjection,
    DtmRxResultProjectionError, DtmSchedulerAllocationConfig, DtmSchedulerItemCompletionStatus,
    DtmTxPacketPrepareError,
    codec::{LINK_STATE_RX_TAIL_OFFSET, SCHEDULER_ITEM_STATUS_OFFSET},
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct GraphSnapshot {
    link_state: [u32; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
    scheduler_context: [u32; BLUETOOTH_SCHEDULER_CONTEXT_BYTES / 4],
    scheduler_item: [u32; BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4],
    rx_header: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
    rx_swap_reserve: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
    tx_header: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
    tx_packet: [u8; BLUETOOTH_DTM_TX_PACKET_BYTES],
    rx_packet: [u8; BLUETOOTH_DTM_RX_PACKET_BYTES],
}

fn snapshot(storage: &DtmMemoryGraphStorage) -> GraphSnapshot {
    GraphSnapshot {
        link_state: storage.link_state.snapshot(),
        scheduler_context: storage.scheduler_context.snapshot(),
        scheduler_item: storage.scheduler_item.snapshot(),
        rx_header: storage.rx_header.snapshot_words(),
        rx_swap_reserve: storage.rx_swap_reserve.snapshot_words(),
        tx_header: storage.tx_header.snapshot(),
        tx_packet: storage.tx_packet.snapshot(),
        rx_packet: storage.rx_packet.snapshot(),
    }
}

fn model_owner(base: u32) -> DtmMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(DtmMemoryGraphStorage::new()));
    let base = DtmMemoryGraphModelAddress::new(base)
        .expect("test base has valid compressed-pointer syntax");
    DtmMemoryGraphStorage::pin_static_model(storage, base, allocation_config())
        .expect("test graph fits physical controller SRAM")
}

fn running_owner_with_status(base: u32, status: u32) -> DtmMemoryGraphRunning {
    running_owner_from_cpu(model_owner(base), status)
}

fn running_owner_from_cpu(owner: DtmMemoryGraphCpuOwned, status: u32) -> DtmMemoryGraphRunning {
    let prepared = owner
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("matching anchors prepare a CPU-owned image")
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link();
    let owner = DtmMemoryGraphRunning {
        storage: prepared.storage,
        binding: prepared.binding,
    };
    owner
        .storage
        .as_ref()
        .get_ref()
        .scheduler_item
        .write_word(SCHEDULER_ITEM_STATUS_OFFSET, status);
    owner
}

fn script_current_rx_tail_returned(
    owner: &DtmMemoryGraphRunning,
    result_word: u32,
    auxiliary: u16,
) {
    let storage = owner.storage.as_ref().get_ref();
    let tail = storage.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET);
    let header = if tail == owner.binding.rx_header.controller_address().address() {
        &storage.rx_header
    } else if tail == owner.binding.rx_swap_reserve.controller_address().address() {
        &storage.rx_swap_reserve
    } else {
        panic!("the semantic fixture requires one bound RX tail");
    };
    header.model_controller_completion_observed();
    storage
        .rx_packet
        .model_controller_completion(result_word, auxiliary);
}

fn commit_rx_observed(
    observed: DtmMemoryGraphRxSuccessObserved,
) -> (
    DtmMemoryGraphRecycleCleaned,
    Option<Result<DtmRxResultProjection, DtmRxResultProjectionError>>,
) {
    observed.consume_then_commit(core::convert::identity)
}

fn rx_success_recycle_prepared(owner: DtmMemoryGraphRunning) -> DtmMemoryGraphRecyclePrepared {
    let completed = match owner.observe_completion(observed_list(0)) {
        DtmMemoryGraphCompletionObservation::CompletionObserved(completed) => completed,
        _ => panic!("the scripted RX scheduler item must be completed"),
    };
    let address = completed.scheduler_item_address();
    match completed.prepare_recycle_after_software_list_removal(removal_ready_for(
        BluetoothSchedulerHardwareListIndex::ZERO,
        address,
    )) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the exact removal proof must prepare RX recycle"),
    }
}

fn observed_list(
    index: u8,
) -> oer_esp32s31_hal::bluetooth::BluetoothSchedulerFinishedHardwareListObserved {
    let observation =
        BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[index])
            .expect("test list belongs to the scheduler domain");
    match observation.pop_lowest() {
        BluetoothSchedulerFinishedListPop::List {
            observed,
            remaining,
        } => {
            assert!(remaining.is_empty());
            observed
        }
        BluetoothSchedulerFinishedListPop::Complete => {
            unreachable!("one scripted list cannot be empty")
        }
    }
}

fn removal_ready_for(
    index: BluetoothSchedulerHardwareListIndex,
    address: BluetoothControllerSramAddress,
) -> BluetoothSchedulerSoftwareListRemovalReady {
    let head = BluetoothSchedulerHardwareListHead::from_address(address)
        .expect("test identity is a nonempty controller head");
    let head =
        BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(index, head);
    BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(head)
}

const fn allocation_config() -> DtmSchedulerAllocationConfig {
    DtmSchedulerAllocationConfig::new(2, 3, 4)
}

fn candidate_words(seed: DtmPositionalEventSeed) -> DtmPositionalEventWords {
    let current = seed.words();
    let link_state = current.link_state().apply_reset(
        Some(seed.tx_header_head_projection()),
        Some(seed.rx_header_tail_projection()),
        0,
        0,
        DtmRole::Transmitter,
    );

    DtmPositionalEventWords::new(link_state, current.scheduler_item())
}

#[test]
fn dtm_access_address_initialization_round_trips_semantically() {
    let storage = DtmMemoryGraphStorage::new();
    let reset =
        storage
            .link_state
            .reviewed_words()
            .apply_reset(None, None, 0, 0, DtmRole::Transmitter);

    assert_eq!(reset.access_address(), LeAccessAddress::DIRECT_TEST_MODE);
    storage.link_state.write_reviewed_words(reset);
    assert_eq!(
        storage.link_state.reviewed_words().access_address(),
        LeAccessAddress::DIRECT_TEST_MODE
    );
}

#[test]
fn dtm_crc_initialization_round_trips_semantically() {
    let storage = DtmMemoryGraphStorage::new();
    let reset =
        storage
            .link_state
            .reviewed_words()
            .apply_reset(None, None, 0, 0, DtmRole::Transmitter);

    assert_eq!(reset.crc_init(), LeCrcInit::LE_PRESET);
    storage.link_state.write_reviewed_words(reset);
    assert_eq!(
        storage.link_state.reviewed_words().crc_init(),
        LeCrcInit::LE_PRESET
    );
}

#[test]
fn dtm_reset_selects_and_retains_the_reviewed_link_state_profile() {
    let storage = DtmMemoryGraphStorage::new();
    let reset =
        storage
            .link_state
            .reviewed_words()
            .apply_reset(None, None, 0, 0, DtmRole::Transmitter);

    assert!(reset.profile_word_14.direct_test_mode_is_selected());
    storage.link_state.write_reviewed_words(reset);
    assert!(
        storage
            .link_state
            .reviewed_words()
            .profile_word_14
            .direct_test_mode_is_selected()
    );
}

fn assert_prepare_failure_unchanged<BuildError: Debug + Eq + PartialEq>(
    owner: DtmMemoryGraphCpuOwned,
    build: impl FnOnce(DtmPositionalEventSeed) -> Result<DtmPositionalEventWords, BuildError>,
    expected: DtmMemoryGraphPrepareError<BuildError>,
) -> DtmMemoryGraphCpuOwned {
    let before = snapshot(owner.storage.as_ref().get_ref());
    let failure = match owner.try_prepare_positional_event(build) {
        Ok(_) => panic!("invalid positional event words must be rejected"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), &expected);
    let (owner, error) = failure.into_parts();
    assert_eq!(error, expected);
    assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
    owner
}

fn assert_rejected_before_builder(
    owner: DtmMemoryGraphCpuOwned,
    expected: DtmMemoryGraphPrepareError,
) -> DtmMemoryGraphCpuOwned {
    let builder_called = Cell::new(false);
    let owner = assert_prepare_failure_unchanged(
        owner,
        |seed| {
            builder_called.set(true);
            Ok::<_, Infallible>(candidate_words(seed))
        },
        expected,
    );
    assert!(!builder_called.get());
    owner
}

#[test]
fn positional_preparation_rejects_a_foreign_tx_packet_base_before_builder() {
    let owner = model_owner(0x2f00_0100);
    let foreign = model_owner(0x2f00_2000);
    owner
        .storage
        .as_ref()
        .get_ref()
        .tx_header
        .model_retarget_packet_base(foreign.binding.tx_packet);

    let _owner = assert_rejected_before_builder(
        owner,
        DtmMemoryGraphPrepareError::CurrentTxHeaderPacketBaseMismatch,
    );
}

#[test]
fn positional_preparation_rejects_a_foreign_tx_pdu_before_builder() {
    let owner = model_owner(0x2f00_0100);
    let foreign = model_owner(0x2f00_2000);
    owner
        .storage
        .as_ref()
        .get_ref()
        .tx_header
        .model_retarget_pdu(foreign.binding.tx_packet);

    let _owner = assert_rejected_before_builder(
        owner,
        DtmMemoryGraphPrepareError::CurrentTxHeaderPduTargetMismatch,
    );
}

#[test]
fn positional_preparation_rejects_a_lost_tx_allocation_extent_before_builder() {
    let owner = model_owner(0x2f00_0100);
    owner
        .storage
        .as_ref()
        .get_ref()
        .tx_header
        .model_drop_allocation_extent();

    let _owner = assert_rejected_before_builder(
        owner,
        DtmMemoryGraphPrepareError::CurrentTxHeaderAllocationExtentMismatch,
    );
}

#[test]
fn positional_preparation_rejects_a_foreign_rx_packet_before_builder() {
    let owner = model_owner(0x2f00_0100);
    let foreign = model_owner(0x2f00_2000);
    owner
        .storage
        .as_ref()
        .get_ref()
        .rx_header
        .model_retarget_rx_packet(foreign.binding.rx_packet);

    let _owner = assert_rejected_before_builder(
        owner,
        DtmMemoryGraphPrepareError::CurrentRxTailPacketMismatch,
    );
}

#[test]
fn cancel_restores_the_complete_logical_graph_image() {
    let mut payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
    payload[..3].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
    let owner = model_owner(0x2f00_0500)
        .prepare_tx_packet(7, 3, &payload)
        .expect("standard LE Test PDU Type prepares")
        .discard_packet_readiness();
    let before = snapshot(owner.storage.as_ref().get_ref());

    let prepared = owner
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("matching anchors prepare a CPU-owned image");
    assert_ne!(snapshot(prepared.storage.as_ref().get_ref()), before);
    let owner = prepared.cancel();

    assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
}

#[test]
fn scheduler_bookkeeping_cancel_restores_the_prepared_event() {
    let prepared = model_owner(0x2f00_0900)
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("matching anchors prepare a CPU-owned image");
    let before = snapshot(prepared.storage.as_ref().get_ref());
    let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
    let prepared = scheduler_prepared.cancel();
    assert_eq!(snapshot(prepared.storage.as_ref().get_ref()), before);
}

#[test]
fn completion_observation_preserves_owners_and_classifies_status() {
    let owner = running_owner_with_status(0x2f00_1900, 0);
    let owner = match owner.observe_completion(observed_list(3)) {
        DtmMemoryGraphCompletionObservation::ListMismatch { owner, .. } => owner,
        _ => panic!("another list cannot inspect the DTM item"),
    };
    let completed = match owner.observe_completion(observed_list(0)) {
        DtmMemoryGraphCompletionObservation::CompletionObserved(completed) => completed,
        _ => panic!("zero status must produce a completion observation"),
    };
    assert_eq!(completed.status(), DtmSchedulerItemCompletionStatus::Zero);

    let owner = running_owner_with_status(0x2f00_1d00, u32::MAX);
    assert!(matches!(
        owner.observe_completion(observed_list(0)),
        DtmMemoryGraphCompletionObservation::StillInFlight(_)
    ));

    let owner = running_owner_with_status(0x2f00_2100, 7);
    let completed = match owner.observe_completion(observed_list(0)) {
        DtmMemoryGraphCompletionObservation::CompletionObserved(completed) => completed,
        _ => panic!("non-sentinel status must produce a completion observation"),
    };
    assert_eq!(
        completed.status(),
        DtmSchedulerItemCompletionStatus::NonZero(
            core::num::NonZeroU32::new(7).expect("seven is nonzero")
        )
    );
}

#[test]
fn recycle_is_lossless_before_commit_and_returns_a_reusable_cpu_graph() {
    let owner = running_owner_with_status(0x2f00_2500, 7);
    let completed = match owner.observe_completion(observed_list(0)) {
        DtmMemoryGraphCompletionObservation::CompletionObserved(completed) => completed,
        _ => panic!("non-sentinel status must produce a completion observation"),
    };
    let wrong_address =
        BluetoothControllerSramAddress::new(completed.scheduler_item_address().address() + 4)
            .expect("adjacent aligned model identity stays in controller SRAM");
    let failure = match completed.prepare_recycle_after_software_list_removal(removal_ready_for(
        BluetoothSchedulerHardwareListIndex::ZERO,
        wrong_address,
    )) {
        Ok(_) => panic!("a removal proof for another item must reject before mutation"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        DtmMemoryGraphRecycleError::SchedulerItemMismatch
    );
    let (completed, _wrong_removal) = failure.into_parts();
    assert_eq!(
        completed.status(),
        DtmSchedulerItemCompletionStatus::NonZero(
            core::num::NonZeroU32::new(7).expect("seven is nonzero")
        )
    );

    let address = completed.scheduler_item_address();
    let prepared = match completed.prepare_recycle_after_software_list_removal(removal_ready_for(
        BluetoothSchedulerHardwareListIndex::ZERO,
        address,
    )) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the bound removal proof must authorize the exact completed graph"),
    };
    let cleaned = prepared.commit();
    let (owner, status) = cleaned.into_cpu_owned().into_parts();
    assert_eq!(
        status,
        DtmSchedulerItemCompletionStatus::NonZero(
            core::num::NonZeroU32::new(7).expect("seven is nonzero")
        )
    );
    let _prepared = owner
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("the recycled CPU graph can prepare a later event");
}

#[test]
fn rx_success_without_a_returned_packet_recycles_for_a_later_event() {
    let owner = running_owner_with_status(0x2f00_2900, 0);
    let prepared = rx_success_recycle_prepared(owner)
        .prepare_receiver_success()
        .expect("an incomplete initial tail is a valid empty RX result");
    let (cleaned, projection) = commit_rx_observed(prepared.observe());

    assert_eq!(projection, None);
    let (owner, status) = cleaned.into_cpu_owned().into_parts();
    assert_eq!(status, DtmSchedulerItemCompletionStatus::Zero);
    let _prepared = owner
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("an empty RX completion leaves a reusable graph");
}

#[test]
fn rx_success_rotates_both_headers_across_recurring_events() {
    let first = running_owner_with_status(0x2f00_2d00, 0);
    script_current_rx_tail_returned(&first, 0xa500_0000, 0);
    let (cleaned, first_projection) = commit_rx_observed(
        rx_success_recycle_prepared(first)
            .prepare_receiver_success()
            .expect("the first completed tail has a valid swap plan")
            .observe(),
    );
    assert_eq!(
        first_projection,
        Some(DtmRxResultProjection::from_word(0xa500_0000))
    );

    let (owner, _) = cleaned.into_cpu_owned().into_parts();
    let second = running_owner_from_cpu(owner, 0);
    script_current_rx_tail_returned(&second, 0x3100_0001, 7);
    let (cleaned, second_projection) = commit_rx_observed(
        rx_success_recycle_prepared(second)
            .prepare_receiver_success()
            .expect("the alternate completed tail rotates back to the first slot")
            .observe(),
    );
    assert_eq!(
        second_projection,
        Some(DtmRxResultProjection::from_word(0x3100_0001))
    );

    let (owner, _) = cleaned.into_cpu_owned().into_parts();
    let _prepared = owner
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("two rotations retain a valid recurring graph");
}

#[test]
fn steady_empty_event_preserves_the_chain_for_the_next_return() {
    let first = running_owner_with_status(0x2f00_3100, 0);
    script_current_rx_tail_returned(&first, 0, 0);
    let (cleaned, _) = commit_rx_observed(
        rx_success_recycle_prepared(first)
            .prepare_receiver_success()
            .expect("the first return is valid")
            .observe(),
    );

    let (owner, _) = cleaned.into_cpu_owned().into_parts();
    let empty = running_owner_from_cpu(owner, 0);
    let (cleaned, projection) = commit_rx_observed(
        rx_success_recycle_prepared(empty)
            .prepare_receiver_success()
            .expect("an incomplete steady tail is a valid empty event")
            .observe(),
    );
    assert_eq!(projection, None);

    let (owner, _) = cleaned.into_cpu_owned().into_parts();
    let returned = running_owner_from_cpu(owner, 0);
    script_current_rx_tail_returned(&returned, 0x4200_0000, 3);
    let (cleaned, projection) = commit_rx_observed(
        rx_success_recycle_prepared(returned)
            .prepare_receiver_success()
            .expect("the tail after an empty recurrence remains returnable")
            .observe(),
    );
    assert!(projection.is_some());
    let _owner = cleaned.into_cpu_owned();
}

#[test]
fn rx_success_sentinel_rejection_is_lossless() {
    let owner = running_owner_with_status(0x2f00_3500, 0);
    script_current_rx_tail_returned(&owner, 0x00ff_ffff, 0);
    let prepared = rx_success_recycle_prepared(owner);
    let before = snapshot(prepared.completed.owner.storage.as_ref().get_ref());
    let failure = match prepared.prepare_receiver_success() {
        Ok(_) => panic!("a returned packet cannot retain the result sentinel"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        DtmMemoryGraphRxSuccessRecycleError::ReturnedResultNotProduced
    );
    let prepared = failure.into_recycle_prepared();
    assert_eq!(
        snapshot(prepared.completed.owner.storage.as_ref().get_ref()),
        before
    );
}

#[test]
fn rx_success_auxiliary_rearm_rejection_is_lossless() {
    let owner = running_owner_with_status(0x2f00_3900, 0);
    script_current_rx_tail_returned(&owner, 0, u16::MAX);
    let prepared = rx_success_recycle_prepared(owner);
    let before = snapshot(prepared.completed.owner.storage.as_ref().get_ref());
    let failure = match prepared.prepare_receiver_success() {
        Ok(_) => panic!("a returned packet cannot retain the auxiliary re-arm state"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        DtmMemoryGraphRxSuccessRecycleError::ReturnedAuxiliaryNotProduced
    );
    let prepared = failure.into_recycle_prepared();
    assert_eq!(
        snapshot(prepared.completed.owner.storage.as_ref().get_ref()),
        before
    );
}

#[test]
fn builder_failure_returns_the_byte_unchanged_reusable_owner() {
    let owner = model_owner(0x2f00_0900);
    let owner = assert_prepare_failure_unchanged(
        owner,
        |_| Err::<DtmPositionalEventWords, _>("builder rejected inputs"),
        DtmMemoryGraphPrepareError::Build("builder rejected inputs"),
    );
    let _prepared = owner
        .try_prepare_positional_event(|seed| Ok::<_, Infallible>(candidate_words(seed)))
        .expect("the returned owner remains reusable");
}

#[test]
fn reclaimed_cpu_graph_can_start_another_affine_epoch() {
    let reclaimed = model_owner(0x2f00_4100).into_reclaimed();
    let owner = reclaimed.reinitialize();

    let reclaimed = owner.into_reclaimed();
    let _owner = reclaimed.reinitialize();
}

#[test]
fn failed_binding_returns_the_same_storage_for_retry() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(DtmMemoryGraphStorage::new()));
    let original_address = core::ptr::addr_of!(*storage).addr();
    let crossing =
        DtmMemoryGraphModelAddress::new(BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH - 0x3a8 + 4)
            .expect("crossing base still has valid compressed-pointer syntax");

    let failure =
        match DtmMemoryGraphStorage::pin_static_model(storage, crossing, allocation_config()) {
            Ok(_) => panic!("a graph crossing physical SRAM must be rejected"),
            Err(failure) => failure,
        };
    let (storage, _) = failure.into_parts();
    assert_eq!(core::ptr::addr_of!(*storage).addr(), original_address);

    let valid = DtmMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("retry base has valid compressed-pointer syntax");
    let _owner = DtmMemoryGraphStorage::pin_static_model(storage, valid, allocation_config())
        .expect("returned allocation can be bound exactly once");
}

#[test]
fn every_standard_le_test_pdu_type_reaches_packet_readiness() {
    let payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
    let mut owner = model_owner(0x2f00_2100);

    for payload_type in 0..=7 {
        let prepared = owner
            .prepare_tx_packet(payload_type, 0, &payload)
            .expect("all standard LE Test PDU Types prepare");
        owner = prepared.discard_packet_readiness();
    }
}

#[test]
fn unsupported_le_test_pdu_type_returns_the_unchanged_owner() {
    let payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
    let owner = model_owner(0x2f00_2500);
    let before = snapshot(owner.storage.as_ref().get_ref());

    let failure = match owner.prepare_tx_packet(8, 3, &payload) {
        Ok(_) => panic!("an unsupported LE Test PDU Type cannot claim readiness"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        DtmTxPacketPrepareError::UnsupportedPayloadType
    );
    let (owner, error) = failure.into_parts();
    assert_eq!(error, DtmTxPacketPrepareError::UnsupportedPayloadType);
    assert_eq!(snapshot(owner.storage.as_ref().get_ref()), before);
}

fn stopped_item_for(
    address: BluetoothControllerSramAddress,
) -> oer_esp32s31_hal::bluetooth::BluetoothSchedulerStoppedItem {
    let head = BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(
        BluetoothSchedulerHardwareListIndex::ZERO,
        BluetoothSchedulerHardwareListHead::from_address(address).unwrap(),
    );
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerStoppedItem::from_head_for_validation(head)
}

#[test]
fn stopped_silent_receiver_reclaims_and_can_run_again_without_a_packet() {
    let mut cpu = model_owner(0x2f00_6500);
    for _ in 0..3 {
        let running = running_owner_from_cpu(cpu, u32::MAX);
        let address = running.scheduler_item_address();
        let before = snapshot(running.storage.as_ref().get_ref());
        let (completed, head) = match running.observe_stopped(stopped_item_for(address)) {
            Ok(value) => value,
            Err(_) => panic!("matching stopped item must bind"),
        };
        assert_eq!(
            completed.status(),
            DtmSchedulerItemCompletionStatus::Aborted
        );
        assert_eq!(snapshot(completed.owner.storage.as_ref().get_ref()), before);
        let removal = BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(head);
        let prepared = match completed.prepare_recycle_after_software_list_removal(removal) {
            Ok(value) => value,
            Err(_) => panic!("exact removal must permit recycle"),
        };
        let (owner, status) = prepared.commit().into_cpu_owned().into_parts();
        assert_eq!(status, DtmSchedulerItemCompletionStatus::Aborted);
        cpu = owner;
    }
}

#[test]
fn stop_preserves_completion_racing_the_request_and_rejects_foreign_identity() {
    for (raw, expected) in [
        (0, DtmSchedulerItemCompletionStatus::Zero),
        (
            7,
            DtmSchedulerItemCompletionStatus::NonZero(core::num::NonZeroU32::new(7).unwrap()),
        ),
    ] {
        let running = running_owner_with_status(0x2f00_6900, raw);
        let address = running.scheduler_item_address();
        let before = snapshot(running.storage.as_ref().get_ref());
        let foreign = BluetoothControllerSramAddress::new(address.address() + 4).unwrap();
        let (running, _) = match running.observe_stopped(stopped_item_for(foreign)) {
            Err(owners) => owners,
            Ok(_) => panic!("a different stopped item cannot authorize this graph"),
        };
        assert_eq!(snapshot(running.storage.as_ref().get_ref()), before);
        let (completed, _) = match running.observe_stopped(stopped_item_for(address)) {
            Ok(value) => value,
            Err(_) => panic!("the matching identity must bind"),
        };
        assert_eq!(completed.status(), expected);
    }
}
