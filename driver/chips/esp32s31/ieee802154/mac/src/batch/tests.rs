use super::*;
use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154Event;

#[test]
fn clock_count_is_rejected_before_a_batch_exists() {
    assert_eq!(
        MacEventBatch::new(Ieee802154Event::ClockCountMatch.mask(), None, None, None),
        Err(MacBatchConstructionError::UnsupportedEvents(
            Ieee802154Event::ClockCountMatch.mask()
        ))
    );
}

#[test]
fn abort_reasons_are_present_exactly_with_their_events() {
    let rx = Ieee802154Event::RxAbort.mask();
    let tx = Ieee802154Event::TxAbort.mask();
    assert_eq!(
        MacEventBatch::new(rx, None, None, None),
        Err(MacBatchConstructionError::MissingRxAbortReason)
    );
    assert_eq!(
        MacEventBatch::new(
            Ieee802154EventMask::NONE,
            Some(Ieee802154RxAbortReason::CrcError),
            None,
            None
        ),
        Err(MacBatchConstructionError::UnexpectedRxAbortReason)
    );
    assert_eq!(
        MacEventBatch::new(tx, None, None, None),
        Err(MacBatchConstructionError::MissingTxAbortReason)
    );
    assert_eq!(
        MacEventBatch::new(
            Ieee802154EventMask::NONE,
            None,
            Some(Ieee802154TxAbortReason::CcaBusy),
            None
        ),
        Err(MacBatchConstructionError::UnexpectedTxAbortReason)
    );
}

#[test]
fn ed_done_and_measurement_are_bijective() {
    let sample = MacMeasurementSample::ClearChannel(MacCcaSample::Clear);
    assert_eq!(
        MacEventBatch::new(Ieee802154Event::EdDone.mask(), None, None, None),
        Err(MacBatchConstructionError::MissingMeasurementSample)
    );
    assert_eq!(
        MacEventBatch::new(Ieee802154EventMask::NONE, None, None, Some(sample)),
        Err(MacBatchConstructionError::UnexpectedMeasurementSample)
    );
    assert!(MacEventBatch::new(Ieee802154Event::EdDone.mask(), None, None, Some(sample)).is_ok());
}
