use super::{LeRxError, NonScanningRxMemoryModelAddress, NonScanningRxMemoryStorage};

#[test]
fn published_cursor_preserves_both_receive_slots_across_rearming() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let mut owner = NonScanningRxMemoryStorage::pin_static_model(
        storage,
        NonScanningRxMemoryModelAddress::new(0x2f00_8000).unwrap(),
    )
    .unwrap();
    let initial_cursor = owner.current_cursor();
    let first = [0x02, 6, 1, 2, 3, 4, 5, 6];
    let second = [0x02, 6, 6, 5, 4, 3, 2, 1];
    for _ in 0..2 {
        assert!(owner.extract_completed_rx_batch().unwrap().is_empty());
        let cursor = owner.model_controller_receive_after_current(initial_cursor, &first);
        let batch = owner
            .extract_completed_rx_batch()
            .expect("the first RX has no completion gap");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.packet(0).unwrap().as_bytes(), &first);
        owner.model_controller_receive_after_current(cursor, &second);
        let batch = owner.extract_completed_rx_batch().unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch.packet(1).unwrap().as_bytes(), &second);
        owner.reinitialize_after_event();
        assert_eq!(owner.current_cursor(), initial_cursor);
        assert!(owner.is_initialized());
    }
}

#[test]
fn pinned_pool_forms_one_initialized_two_node_rotation() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let base = NonScanningRxMemoryModelAddress::new(0x2f00_4000)
        .expect("the model base belongs to controller SRAM");
    let owner = NonScanningRxMemoryStorage::pin_static_model(storage, base)
        .expect("the complete RX pool fits controller SRAM");

    assert!(owner.is_initialized());
}

#[test]
fn completed_batch_is_copied_before_explicit_pool_reinitialization() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let base = NonScanningRxMemoryModelAddress::new(0x2f00_5000)
        .expect("the model base belongs to controller SRAM");
    let mut owner = NonScanningRxMemoryStorage::pin_static_model(storage, base)
        .expect("the complete RX pool fits controller SRAM");
    let pdu = [0x02, 6, 1, 2, 3, 4, 5, 6];
    let pool = owner.storage.as_ref().get_ref();
    pool.nodes[0]
        .packet
        .emulate_hardware_receive(&pdu, -42, 0x1234_5678);
    pool.nodes[0].header.emulate_hardware_completion();

    let batch = owner
        .extract_completed_rx_batch()
        .expect("one completed prefix node is a valid receive batch");
    assert!(!owner.is_initialized());
    let packet = batch.packet(0).expect("the completed PDU was copied");
    assert_eq!(packet.as_bytes(), &pdu);
    assert_eq!(packet.rssi_dbm(), -42);
    assert_eq!(batch.discarded_count(), 0);

    owner.reinitialize_after_event();

    assert!(owner.is_initialized());
    assert_eq!(
        batch
            .packet(0)
            .expect("the copied batch remains owned")
            .as_bytes(),
        &pdu
    );
}

#[test]
fn hardware_discard_never_becomes_a_received_pdu() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let base = NonScanningRxMemoryModelAddress::new(0x2f00_6000)
        .expect("the model base belongs to controller SRAM");
    let owner = NonScanningRxMemoryStorage::pin_static_model(storage, base)
        .expect("the complete RX pool fits controller SRAM");
    let pool = owner.storage.as_ref().get_ref();
    pool.nodes[0].packet.emulate_hardware_discard();
    pool.nodes[0].header.emulate_hardware_completion();

    let batch = owner
        .extract_completed_rx_batch()
        .expect("a hardware-discarded observation is not malformed storage");

    assert!(batch.is_empty());
    assert!(batch.packet(0).is_none());
    assert_eq!(batch.discarded_count(), 1);
}

#[test]
fn untouched_producer_result_is_not_a_hardware_discard() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    let base = NonScanningRxMemoryModelAddress::new(0x2f00_7000)
        .expect("the model base belongs to controller SRAM");
    let owner = NonScanningRxMemoryStorage::pin_static_model(storage, base)
        .expect("the complete RX pool fits controller SRAM");
    owner.storage.as_ref().get_ref().nodes[0]
        .header
        .emulate_hardware_completion();

    assert_eq!(
        owner.extract_completed_rx_batch(),
        Err(LeRxError::ProducerSentinelRetained)
    );
}
