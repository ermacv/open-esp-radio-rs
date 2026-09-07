use crate::{support::*, *};

#[test]
fn tx_slot_rejects_stale_cookie_and_completes_one_generation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    slot.as_mut().buffer_mut().unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    assert_eq!(size(slot.descriptor_word0()), 512);
    assert_eq!(length(slot.descriptor_word0()), 100);
    assert_eq!(slot.state(), TxSlotState::Reserved);
    assert_eq!(slot.as_mut().mark_hardware_owned(cookie), Ok(()));
    assert_eq!(
        slot.as_mut().mark_hardware_owned(cookie),
        Err(TxError::Stale)
    );

    let mut mmio = MockMmio::default();
    mmio.set_tx_completion(
        0,
        MacTxCompletionObservation::new_model(3, 0).with_trigger_flow_model(true),
    );

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(completion.cookie(), cookie);
    assert_eq!(completion.status(), 3);
    assert!(completion.is_trigger_flow());
    assert!(!completion.used_alternate_record());
    assert_eq!(slot.state(), TxSlotState::Completed);

    mmio.set_tx_queue_attached(0, true);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
    assert_eq!(slot.state(), TxSlotState::Free);
}

#[test]
fn tx_slot_cancels_only_an_unpublished_reservation() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();

    assert_eq!(slot.as_mut().cancel_reservation(cookie), Ok(()));
    assert_eq!(slot.state(), TxSlotState::Free);
    assert_eq!(slot.descriptor_word0(), 0);
    assert!(slot.as_mut().buffer_mut().is_ok());
    assert_eq!(
        slot.as_mut().cancel_reservation(cookie),
        Err(TxError::Stale)
    );
}

#[test]
fn executor_deadline_quarantines_hardware_owned_tx_storage_without_drop_panic() {
    let mut slot = std::boxed::Box::pin(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    assert_eq!(slot.as_mut().require_reset(cookie), Ok(()));
    assert_eq!(slot.state(), TxSlotState::ResetRequired);
    assert!(matches!(slot.as_mut().buffer_mut(), Err(TxError::Busy)));
    assert_eq!(slot.as_mut().require_reset(cookie), Err(TxError::Stale));
    drop(slot);
}

#[test]
fn tx_completion_decodes_the_blob_ack_snr_byte() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    // Encoded 0x8b plus the pinned 0x60 offset narrows to signed -21.
    mmio.set_tx_completion(
        0,
        MacTxCompletionObservation::new_model(0, 0).with_ack_snr_encoded_model(0x8b),
    );

    let completion = slot
        .as_mut()
        .acknowledge_q0_completion(&mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(completion.status(), 0);
    assert_eq!(completion.ack_snr_sample(), Some(-21));

    let failed = TxCompletion::new_model(cookie, 5, 0).with_ack_snr_encoded_model(0x8b);
    assert_eq!(failed.ack_snr_sample(), None);

    mmio.set_tx_queue_attached(0, true);
    slot.as_mut().detach_completed(&mut mmio, cookie).unwrap();
}

#[test]
fn tx_slot_preserves_the_semantic_timeout_abort_order() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    mmio.set_tx_timeout_pending(0, true);
    mmio.set_tx_queue_attached(0, true);

    assert_eq!(
        slot.as_mut().begin_timeout_abort(&mut mmio, cookie),
        Ok(true)
    );
    slot.as_mut()
        .finish_timeout_abort(&mut mmio, cookie)
        .unwrap();

    assert_eq!(slot.state(), TxSlotState::Free);
    assert!(!mmio.tx_queue_attached[0]);
    assert!(!mmio.tx_timeout_pending[0]);

    let invalidation = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::DisableTxQueue(0))
        .unwrap();
    let cca_release = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::ReleaseTxCca)
        .unwrap();
    let timeout_clear = mmio
        .operations()
        .iter()
        .position(|operation| {
            *operation == Operation::AcknowledgeTxEvent(0, MacTxDetachReason::Timeout)
        })
        .unwrap();
    assert!(invalidation < cca_release);
    assert!(cca_release < timeout_clear);
}

#[test]
fn tx_slot_disables_before_acknowledging_one_collision_queue() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let cookie = slot.as_mut().reserve(512, 100).unwrap();
    slot.as_mut().mark_hardware_owned(cookie).unwrap();

    let mut mmio = MockMmio::default();
    mmio.set_tx_collision_pending(0, true);
    mmio.set_tx_queue_attached(0, true);

    assert_eq!(slot.as_mut().abort_collision(&mut mmio, cookie), Ok(true));
    assert_eq!(slot.state(), TxSlotState::Free);
    assert!(!mmio.tx_queue_attached[0]);
    assert!(!mmio.tx_collision_pending[0]);

    let disable = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::DisableTxQueue(0))
        .unwrap();
    let acknowledge = mmio
        .operations()
        .iter()
        .position(|operation| {
            *operation == Operation::AcknowledgeTxEvent(0, MacTxDetachReason::Collision)
        })
        .unwrap();
    assert!(disable < acknowledge);
}
