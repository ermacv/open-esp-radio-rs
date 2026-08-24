//! Transactional construction of one already sampled interrupt batch.

use open_esp_radio_esp32s31_ieee802154_irq::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventSink, Ieee802154RxAbortReason,
    Ieee802154TxAbortReason, dispatch_event_batch,
};

/// One separately sampled clear-channel result accompanying `ED_DONE`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacCcaSample {
    /// The separately sampled CCA status reports an available channel.
    Clear,
    /// The separately sampled CCA status reports a busy channel.
    Busy,
}

/// One raw signed energy-detection sample.
///
/// The value deliberately carries no dBm claim. The reviewed vendor path adds
/// a compensation constant whose complete open calibration contract has not
/// been established.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacEnergySample(i8);

impl MacEnergySample {
    /// Preserve one already sampled signed hardware code.
    pub const fn from_raw_code(code: i8) -> Self {
        Self(code)
    }

    /// Return the uncalibrated signed hardware code.
    pub const fn raw_code(self) -> i8 {
        self.0
    }
}

/// Typed sideband required by an `ED_DONE` event.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacMeasurementSample {
    /// A clear-channel status sampled for a standalone CCA request.
    ClearChannel(MacCcaSample),
    /// An uncalibrated energy code sampled for an ED request.
    Energy(MacEnergySample),
}

/// Failure to construct a self-consistent sampled event batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacBatchConstructionError {
    /// The named event mask contains a bit absent from the reviewed ISR.
    UnsupportedEventBits {
        /// Complete rejected event-bit subset.
        bits: u16,
    },
    /// `RX_ABORT` was present without its sampled reason.
    MissingRxAbortReason,
    /// A receive-abort reason was supplied without `RX_ABORT`.
    UnexpectedRxAbortReason,
    /// `TX_ABORT` was present without its sampled reason.
    MissingTxAbortReason,
    /// A transmit-abort reason was supplied without `TX_ABORT`.
    UnexpectedTxAbortReason,
    /// `ED_DONE` was present without its separately sampled result.
    MissingMeasurementSample,
    /// A measurement was supplied without `ED_DONE`.
    UnexpectedMeasurementSample,
}

/// One immutable, internally consistent interrupt snapshot plus sidebands.
///
/// Construction is transactional. In particular, the named-but-unhandled
/// clock-count event is rejected by the IRQ crate before any sink callback or
/// MAC actor transition can run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacEventBatch {
    events: Ieee802154EventMask,
    rx_abort_reason: Option<Ieee802154RxAbortReason>,
    tx_abort_reason: Option<Ieee802154TxAbortReason>,
    measurement: Option<MacMeasurementSample>,
}

impl MacEventBatch {
    /// Validate one complete set of already sampled values.
    pub fn new(
        events: Ieee802154EventMask,
        rx_abort_reason: Option<Ieee802154RxAbortReason>,
        tx_abort_reason: Option<Ieee802154TxAbortReason>,
        measurement: Option<MacMeasurementSample>,
    ) -> Result<Self, MacBatchConstructionError> {
        let mut sink = ValidationSink;
        if let Err(error) = dispatch_event_batch(events, &mut sink) {
            return Err(MacBatchConstructionError::UnsupportedEventBits {
                bits: error.unsupported_event_bits(),
            });
        }

        let has_rx_abort = events.contains(Ieee802154Event::RxAbort);
        match (has_rx_abort, rx_abort_reason.is_some()) {
            (true, false) => return Err(MacBatchConstructionError::MissingRxAbortReason),
            (false, true) => return Err(MacBatchConstructionError::UnexpectedRxAbortReason),
            _ => {}
        }

        let has_tx_abort = events.contains(Ieee802154Event::TxAbort);
        match (has_tx_abort, tx_abort_reason.is_some()) {
            (true, false) => return Err(MacBatchConstructionError::MissingTxAbortReason),
            (false, true) => return Err(MacBatchConstructionError::UnexpectedTxAbortReason),
            _ => {}
        }

        let has_measurement = events.contains(Ieee802154Event::EdDone);
        match (has_measurement, measurement.is_some()) {
            (true, false) => return Err(MacBatchConstructionError::MissingMeasurementSample),
            (false, true) => return Err(MacBatchConstructionError::UnexpectedMeasurementSample),
            _ => {}
        }

        Ok(Self {
            events,
            rx_abort_reason,
            tx_abort_reason,
            measurement,
        })
    }

    /// Return the reviewed event mask.
    pub const fn events(self) -> Ieee802154EventMask {
        self.events
    }

    /// Return the receive-abort reason paired with `RX_ABORT`, if present.
    pub const fn rx_abort_reason(self) -> Option<Ieee802154RxAbortReason> {
        self.rx_abort_reason
    }

    /// Return the transmit-abort reason paired with `TX_ABORT`, if present.
    pub const fn tx_abort_reason(self) -> Option<Ieee802154TxAbortReason> {
        self.tx_abort_reason
    }

    /// Return the sideband paired with `ED_DONE`, if present.
    pub const fn measurement(self) -> Option<MacMeasurementSample> {
        self.measurement
    }
}

struct ValidationSink;

impl Ieee802154EventSink for ValidationSink {
    fn on_event(
        &mut self,
        _event: open_esp_radio_esp32s31_ieee802154_irq::Ieee802154DispatchedEvent,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154Event;

    #[test]
    fn clock_count_is_rejected_before_a_batch_exists() {
        assert_eq!(
            MacEventBatch::new(Ieee802154Event::ClockCountMatch.mask(), None, None, None),
            Err(MacBatchConstructionError::UnsupportedEventBits {
                bits: Ieee802154Event::ClockCountMatch.bit(),
            })
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
        assert!(
            MacEventBatch::new(Ieee802154Event::EdDone.mask(), None, None, Some(sample)).is_ok()
        );
    }

    #[test]
    fn all_handled_event_subsets_have_one_deterministic_construction_result() {
        for raw in 0_u16..=0x3fff {
            let Ok(events) = Ieee802154EventMask::from_named_bits(raw) else {
                continue;
            };
            if events.contains(Ieee802154Event::ClockCountMatch) {
                assert!(matches!(
                    MacEventBatch::new(events, None, None, None),
                    Err(MacBatchConstructionError::UnsupportedEventBits { .. })
                ));
                continue;
            }

            let rx_reason = events
                .contains(Ieee802154Event::RxAbort)
                .then_some(Ieee802154RxAbortReason::CrcError);
            let tx_reason = events
                .contains(Ieee802154Event::TxAbort)
                .then_some(Ieee802154TxAbortReason::TxSecurityError);
            let measurement =
                events
                    .contains(Ieee802154Event::EdDone)
                    .then_some(MacMeasurementSample::Energy(
                        MacEnergySample::from_raw_code(-42),
                    ));
            assert!(
                MacEventBatch::new(events, rx_reason, tx_reason, measurement).is_ok(),
                "handled mask {raw:#06x} must form exactly one batch"
            );
        }
    }
}
