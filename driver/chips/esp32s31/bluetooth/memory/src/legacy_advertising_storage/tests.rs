use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerSoftwareListRemovalReady,
};

use super::{
    BluetoothLegacyAdvertisingMemoryGraphModelAddress, BluetoothLegacyAdvertisingMemoryGraphStorage,
};

fn owner() -> super::BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
        .expect("the complete graph fits physical controller SRAM")
}

fn first_event() -> super::BluetoothLegacyAdvertisingMemoryGraphEventPrepared {
    event(
        crate::BluetoothLegacyAdvertisingPrimaryChannelPlan::new(true, false, false)
            .expect("one channel is non-empty"),
    )
}

fn event(
    channels: crate::BluetoothLegacyAdvertisingPrimaryChannelPlan,
) -> super::BluetoothLegacyAdvertisingMemoryGraphEventPrepared {
    owner()
        .prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6])
        .expect("the encoded advertising PDU fits")
        .reset_link_state(0)
        .expect("the PDU selects the restricted reset")
        .prepare_event(channels, 1_000, 128)
        .expect("the bound event graph is intact")
}

#[test]
fn bound_graph_prepares_and_cancels_one_complete_advertising_pdu() {
    let owner = owner();
    assert!(owner.retains_reviewed_graph());
    let identity = owner.binding().identity();
    let range = owner.binding().range();
    let prepared = owner
        .prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6])
        .expect("the complete legacy advertising PDU fits");

    assert_eq!(prepared.pdu(), &[0x02, 6, 1, 2, 3, 4, 5, 6]);
    assert_eq!(prepared.binding().identity(), identity);

    let owner = prepared.cancel();
    assert!(owner.retains_reviewed_graph());
    assert_eq!(owner.binding().identity(), identity);
    assert_eq!(owner.binding().range(), range);
}

#[test]
fn malformed_packet_returns_the_same_graph_for_retry() {
    let owner = owner();
    let identity = owner.binding().identity();
    let failure = match owner.prepare_packet(&[0x02, 7, 1, 2, 3]) {
        Ok(_) => panic!("a mismatched encoded length must fail closed"),
        Err(failure) => failure,
    };
    let (owner, _) = failure.into_parts();
    assert_eq!(owner.binding().identity(), identity);
    assert!(owner.prepare_packet(&[0x02, 3, 1, 2, 3]).is_ok());
}

#[test]
fn reset_rejects_a_non_advertising_packet_without_losing_the_prepared_graph() {
    let prepared = owner()
        .prepare_packet(&[0x00, 6, 1, 2, 3, 4, 5, 6])
        .expect("the common packet allocation validates only the LE length");
    let failure = match prepared.reset_link_state(0) {
        Ok(_) => panic!("a non-advertising PDU must not select the advertising reset"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        super::BluetoothLegacyAdvertisingPduError::UnsupportedPduType
    );
    let (prepared, _) = failure.into_parts();
    assert_eq!(prepared.pdu(), &[0x00, 6, 1, 2, 3, 4, 5, 6]);
    assert!(prepared.cancel().retains_reviewed_graph());
}

#[test]
fn random_address_packet_reaches_the_same_cancellable_reset_state() {
    let reset = owner()
        .prepare_packet(&[0x42, 6, 1, 2, 3, 4, 5, 0xc6])
        .expect("the encoded random-address PDU fits")
        .reset_link_state(7)
        .expect("TxAdd selects the reviewed static-random reset branch");
    assert_eq!(reset.pdu(), &[0x42, 6, 1, 2, 3, 4, 5, 0xc6]);
    assert!(reset.cancel().retains_reviewed_graph());
}

#[test]
fn first_event_preparation_is_affine_and_cancellation_restores_the_graph() {
    let prepared = first_event();

    assert_eq!(prepared.pdu(), &[0x02, 6, 1, 2, 3, 4, 5, 6]);
    assert!(prepared.cancel().retains_reviewed_graph());
}

#[test]
fn scheduler_prefix_and_empty_list_links_cancel_back_to_the_event() {
    let empty = first_event()
        .prepare_scheduler_bookkeeping()
        .prepare_empty_list_link();
    let prepared = empty.cancel().cancel();

    assert_eq!(prepared.pdu(), &[0x02, 6, 1, 2, 3, 4, 5, 6]);
    assert!(prepared.cancel().retains_reviewed_graph());
}

#[test]
fn three_channel_event_waits_for_and_reports_every_hardware_item() {
    let prepared = event(
        crate::BluetoothLegacyAdvertisingPrimaryChannelPlan::new(true, true, true)
            .expect("all primary channels form one event"),
    );
    assert_eq!(prepared.scheduler_item_count(), 3);
    assert!(!prepared.storage.as_ref().get_ref().scheduler_items[0].is_terminal());
    assert!(!prepared.storage.as_ref().get_ref().scheduler_items[1].is_terminal());
    assert!(prepared.storage.as_ref().get_ref().scheduler_items[2].is_terminal());

    let prepared = prepared.prepare_scheduler_bookkeeping();
    prepared.storage.as_ref().get_ref().scheduler_items[0].words
        [super::SCHEDULER_ITEM_WORD_38_OFFSET]
        .set(0);
    prepared.storage.as_ref().get_ref().scheduler_items[1].words
        [super::SCHEDULER_ITEM_WORD_38_OFFSET]
        .set(7);
    let running = super::BluetoothLegacyAdvertisingMemoryGraphRunning {
        storage: prepared.storage,
        binding: prepared.binding,
        _packet_length: prepared.packet_length,
        item_count: prepared.item_count,
    };
    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };
    let super::BluetoothLegacyAdvertisingMemoryGraphCompletionObservation::StillInFlight(running) =
        running.observe_completion(observed)
    else {
        panic!("the last channel still has its in-flight sentinel")
    };
    running.storage.as_ref().get_ref().scheduler_items[2].words
        [super::SCHEDULER_ITEM_WORD_38_OFFSET]
        .set(0);

    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };
    let super::BluetoothLegacyAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
        completed,
    ) = running.observe_completion(observed)
    else {
        panic!("every active item has a non-sentinel result")
    };
    let statuses = completed.statuses();
    assert_eq!(statuses.item_count(), 3);
    assert_eq!(
        statuses.status(0),
        Some(super::BluetoothLegacyAdvertisingSchedulerItemCompletionStatus::Zero)
    );
    assert!(matches!(
        statuses.status(1),
        Some(super::BluetoothLegacyAdvertisingSchedulerItemCompletionStatus::NonZero(status))
            if status.get() == 7
    ));
    assert_eq!(
        statuses.status(2),
        Some(super::BluetoothLegacyAdvertisingSchedulerItemCompletionStatus::Zero)
    );
}

#[test]
fn fenced_list_zero_observation_classifies_a_completed_event_once() {
    let prepared = first_event().prepare_scheduler_bookkeeping();
    prepared.storage.as_ref().get_ref().scheduler_items[0].words
        [super::SCHEDULER_ITEM_WORD_38_OFFSET]
        .set(0);
    let running = super::BluetoothLegacyAdvertisingMemoryGraphRunning {
        storage: prepared.storage,
        binding: prepared.binding,
        _packet_length: prepared.packet_length,
        item_count: prepared.item_count,
    };
    let scheduler_item_address = running.scheduler_item_address();
    let observation = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0])
        .expect("list zero is representable");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = observation.pop_lowest() else {
        panic!("the semantic observation contains list zero")
    };

    let super::BluetoothLegacyAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
        completed,
    ) = running.observe_completion(observed)
    else {
        panic!("status zero is a completed advertising item")
    };
    assert_eq!(
        completed.statuses().status(0),
        Some(super::BluetoothLegacyAdvertisingSchedulerItemCompletionStatus::Zero)
    );
    let head = BluetoothSchedulerHardwareListHead::from_address(scheduler_item_address)
        .expect("the retained graph has a nonempty scheduler-head identity");
    let empty = BluetoothSchedulerHardwareListHeadEmptyObserved::from_identity_for_validation(
        BluetoothSchedulerHardwareListIndex::ZERO,
        head,
    );
    let removal = BluetoothSchedulerSoftwareListRemovalReady::from_head_for_validation(empty);
    let recycled = match completed.prepare_recycle_after_software_list_removal(removal) {
        Ok(prepared) => prepared.commit(),
        Err(_) => panic!("the matching removal proof must release the advertising graph"),
    };
    let (owner, status) = recycled.into_parts();
    assert_eq!(
        status.status(0),
        Some(super::BluetoothLegacyAdvertisingSchedulerItemCompletionStatus::Zero)
    );
    assert!(owner.prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6]).is_ok());
}
