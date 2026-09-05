use super::{
    RadioPhyRestoreSlot, RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetPrepareError,
    TxDcPwdetRestoreError, TxDcPwdetRestoreFields, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError, decode_noise_floor_quarter_db, quarter_db_to_dbm,
};
use std::{cell::RefCell, vec::Vec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreEvent {
    Capture,
    Prepare,
    Restore,
}

#[test]
fn txdc_restore_slot_rejects_interlopers_and_preserves_operation_order() {
    let mut slot = RadioPhyRestoreSlot::new();
    let events = RefCell::new(Vec::new());
    slot.prepare_txdc_with(
        || {
            events.borrow_mut().push(RestoreEvent::Capture);
            (TxDcPwdetRestoreFields::default(), ())
        },
        |()| events.borrow_mut().push(RestoreEvent::Prepare),
    )
    .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [RestoreEvent::Capture, RestoreEvent::Prepare]
    );

    let rejected = slot.prepare_txdc_with(
        || panic!("occupied restore slot must not capture registers"),
        |_: ()| panic!("occupied restore slot must not prepare registers"),
    );
    assert_eq!(rejected, Err(TxDcPwdetPrepareError::RestorePending));
    assert_eq!(
        events.borrow().as_slice(),
        [RestoreEvent::Capture, RestoreEvent::Prepare]
    );
    let rejected = slot.prepare_txiq_with(|| panic!("TX-DC owner must exclude TX-IQ capture"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));

    slot.restore_txdc_with(|_| {
        events.borrow_mut().push(RestoreEvent::Restore);
    })
    .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [
            RestoreEvent::Capture,
            RestoreEvent::Prepare,
            RestoreEvent::Restore
        ]
    );

    let rejected =
        slot.restore_txdc_with(|_| panic!("empty restore slot must not touch registers"));
    assert_eq!(rejected, Err(TxDcPwdetRestoreError::RestoreNotPending));
}

#[test]
fn txiq_restore_slot_rejects_interlopers_and_consumes_authority_after_restore() {
    let mut slot = RadioPhyRestoreSlot::new();
    let events = RefCell::new(Vec::new());
    slot.prepare_txiq_with(|| {
        events.borrow_mut().push(RestoreEvent::Capture);
        super::TxIqToneControlFields::default()
    })
    .unwrap();

    let rejected = slot.prepare_txiq_with(|| panic!("occupied slot must not sample the register"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));
    assert_eq!(events.borrow().as_slice(), [RestoreEvent::Capture]);
    let rejected = slot.prepare_txdc_with(
        || panic!("TX-IQ owner must exclude TX-DC capture"),
        |_: ()| panic!("TX-IQ owner must exclude TX-DC preparation"),
    );
    assert_eq!(rejected, Err(TxDcPwdetPrepareError::RestorePending));

    slot.restore_txiq_with(|_| events.borrow_mut().push(RestoreEvent::Restore))
        .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [RestoreEvent::Capture, RestoreEvent::Restore]
    );

    let rejected = slot.restore_txiq_with(|_| panic!("empty slot must not write the register"));
    assert_eq!(
        rejected,
        Err(TxIqToneControlRestoreError::RestoreNotPending)
    );
}

#[test]
fn rx_dco_restore_slot_is_a_bounded_lifo_and_excludes_other_calibrations() {
    let mut slot = RadioPhyRestoreSlot::new();
    slot.prepare_rx_dco_with(|| 1).unwrap();
    slot.prepare_rx_dco_with(|| 2).unwrap();

    let rejected = slot.prepare_rx_dco_with(|| panic!("full stack must not capture"));
    assert_eq!(rejected, Err(RxDcoControlPrepareError::RestoreStackFull));
    let rejected = slot.prepare_txiq_with(|| panic!("RX-DCO must exclude TX-IQ capture"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));

    let restored = RefCell::new(Vec::new());
    slot.restore_rx_dco_with(|field| restored.borrow_mut().push(field))
        .unwrap();
    assert!(slot.rx_dco_pending());
    slot.restore_rx_dco_with(|field| restored.borrow_mut().push(field))
        .unwrap();
    assert_eq!(restored.borrow().as_slice(), [2, 1]);
    assert!(!slot.rx_dco_pending());

    let rejected = slot.restore_rx_dco_with(|_| panic!("empty stack must not write the register"));
    assert_eq!(rejected, Err(RxDcoControlRestoreError::RestoreNotPending));
}

#[test]
fn noise_floor_decode_reproduces_both_complete_arithmetic_shifts() {
    // -96 dBm is encoded as -1536 sixteenth-dB, or low twelve bits 0xa00.
    assert_eq!(decode_noise_floor_quarter_db(0x0a00), -384);
    assert_eq!(decode_noise_floor_quarter_db(0x0fff), -1);
    assert_eq!(decode_noise_floor_quarter_db(0x0000), -1024);
    assert_eq!(quarter_db_to_dbm(-384), -96);
    assert_eq!(quarter_db_to_dbm(-1), 0);
    assert_eq!(quarter_db_to_dbm(-1024), 0);
}
