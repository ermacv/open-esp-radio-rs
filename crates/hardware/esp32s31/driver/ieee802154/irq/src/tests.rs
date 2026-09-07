extern crate std;

use std::{cell::RefCell, rc::Rc, vec::Vec};

use super::*;

#[derive(Default)]
struct RecordingSink {
    events: Vec<Ieee802154DispatchedEvent>,
}

impl Ieee802154EventSink for RecordingSink {
    fn on_event(&mut self, event: Ieee802154DispatchedEvent) {
        self.events.push(event);
    }
}

#[test]
fn full_batch_dispatches_in_exact_vendor_order() {
    let mut sink = RecordingSink::default();
    dispatch_event_batch(Ieee802154EventMask::VENDOR_HANDLED, &mut sink)
        .expect("all events have reviewed handlers");

    assert_eq!(
        sink.events,
        [
            Ieee802154DispatchedEvent::RxAbortPhase1,
            Ieee802154DispatchedEvent::RxSfdDone,
            Ieee802154DispatchedEvent::TxSfdDone,
            Ieee802154DispatchedEvent::TxDone,
            Ieee802154DispatchedEvent::RxDone,
            Ieee802154DispatchedEvent::AckTxDone,
            Ieee802154DispatchedEvent::AckRxDone,
            Ieee802154DispatchedEvent::RxAbortPhase2,
            Ieee802154DispatchedEvent::TxAbort,
            Ieee802154DispatchedEvent::EdDone,
            Ieee802154DispatchedEvent::Timer0Overflow,
            Ieee802154DispatchedEvent::Timer1Overflow,
        ]
    );
}

#[test]
fn receive_abort_dispatches_both_phases() {
    let mut sink = RecordingSink::default();
    dispatch_event_batch(Ieee802154Event::RxAbort.mask(), &mut sink).expect("RX abort is handled");
    assert_eq!(
        sink.events,
        [
            Ieee802154DispatchedEvent::RxAbortPhase1,
            Ieee802154DispatchedEvent::RxAbortPhase2,
        ]
    );
}

#[test]
fn unsupported_batch_is_rejected_before_any_callback() {
    let mut sink = RecordingSink::default();
    let error = dispatch_event_batch(Ieee802154Event::ClockCountMatch.mask(), &mut sink)
        .expect_err("clock-count event is named but has no reviewed handler");
    assert_eq!(
        error.unsupported_events(),
        Ieee802154Event::ClockCountMatch.mask()
    );
    assert!(sink.events.is_empty());
}

#[test]
fn empty_batch_is_a_noop() {
    let mut sink = RecordingSink::default();
    dispatch_event_batch(Ieee802154EventMask::NONE, &mut sink).expect("empty batch is supported");
    assert!(sink.events.is_empty());
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HardIrqOperation {
    Status,
    ReadEventClassification,
    ReadRxAbortReason,
    ReadTxAbortReason,
    ReadEdRss,
    ReadCcaBusy,
    Acknowledge(u8),
    Post,
}

struct ModelInterruptSnapshot {
    identity: u8,
    event_classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
    rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
    tx_abort_reason: Option<Ieee802154TxAbortReasonObservation>,
    ed_rss_code: i8,
    cca_busy: bool,
    operations: Rc<RefCell<Vec<HardIrqOperation>>>,
}

impl InterruptSnapshot for ModelInterruptSnapshot {
    fn event_classification(&self) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        self.operations
            .borrow_mut()
            .push(HardIrqOperation::ReadEventClassification);
        self.event_classification
    }

    fn rx_abort_reason(&self) -> Option<Ieee802154RxAbortReasonObservation> {
        self.operations
            .borrow_mut()
            .push(HardIrqOperation::ReadRxAbortReason);
        self.rx_abort_reason
    }

    fn tx_abort_reason(&self) -> Option<Ieee802154TxAbortReasonObservation> {
        self.operations
            .borrow_mut()
            .push(HardIrqOperation::ReadTxAbortReason);
        self.tx_abort_reason
    }

    fn ed_rss_code(&self) -> i8 {
        self.operations
            .borrow_mut()
            .push(HardIrqOperation::ReadEdRss);
        self.ed_rss_code
    }

    fn cca_busy(&self) -> bool {
        self.operations
            .borrow_mut()
            .push(HardIrqOperation::ReadCcaBusy);
        self.cca_busy
    }
}

struct ModelInterruptPort {
    snapshot: Option<ModelInterruptSnapshot>,
    operations: Rc<RefCell<Vec<HardIrqOperation>>>,
}

impl InterruptPort for ModelInterruptPort {
    type Snapshot = ModelInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.operations.borrow_mut().push(HardIrqOperation::Status);
        self.snapshot
            .take()
            .expect("one model status snapshot is available")
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.operations
            .borrow_mut()
            .push(HardIrqOperation::Acknowledge(snapshot.identity));
    }
}

struct ModelAcknowledgedSink {
    operations: Rc<RefCell<Vec<HardIrqOperation>>>,
    posted: RefCell<Option<Ieee802154AcknowledgedInterrupt>>,
}

impl Ieee802154AcknowledgedInterruptSink for ModelAcknowledgedSink {
    fn post(
        &self,
        acknowledged: Ieee802154AcknowledgedInterrupt,
    ) -> Result<(), Ieee802154AcknowledgedInterrupt> {
        self.operations.borrow_mut().push(HardIrqOperation::Post);
        assert!(self.posted.replace(Some(acknowledged)).is_none());
        Ok(())
    }
}

struct RejectingAcknowledgedSink {
    operations: Rc<RefCell<Vec<HardIrqOperation>>>,
}

impl Ieee802154AcknowledgedInterruptSink for RejectingAcknowledgedSink {
    fn post(
        &self,
        acknowledged: Ieee802154AcknowledgedInterrupt,
    ) -> Result<(), Ieee802154AcknowledgedInterrupt> {
        self.operations.borrow_mut().push(HardIrqOperation::Post);
        Err(acknowledged)
    }
}

fn hard_irq_fixture(
    identity: u8,
    event_classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
    rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
    tx_abort_reason: Option<Ieee802154TxAbortReasonObservation>,
    ed_rss_code: i8,
    cca_busy: bool,
) -> (ModelInterruptPort, ModelAcknowledgedSink) {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let snapshot = ModelInterruptSnapshot {
        identity,
        event_classification,
        rx_abort_reason,
        tx_abort_reason,
        ed_rss_code,
        cca_busy,
        operations: Rc::clone(&operations),
    };
    (
        ModelInterruptPort {
            snapshot: Some(snapshot),
            operations: Rc::clone(&operations),
        },
        ModelAcknowledgedSink {
            operations,
            posted: RefCell::new(None),
        },
    )
}

#[test]
fn hard_irq_acknowledges_the_exact_snapshot_before_posting() {
    let (mut port, sink) = hard_irq_fixture(
        0xa5,
        Ok(Ieee802154Event::RxAbort
            .mask()
            .union(Ieee802154Event::EdDone.mask())),
        Some(Ieee802154RxAbortReason::CrcError.into()),
        Some(Ieee802154TxAbortReason::CcaBusy.into()),
        -73,
        true,
    );

    assert_eq!(
        handle_interrupt(&mut port, &sink),
        Ieee802154InterruptDisposition::Posted
    );
    assert_eq!(
        *sink.operations.borrow(),
        [
            HardIrqOperation::Status,
            HardIrqOperation::ReadEventClassification,
            HardIrqOperation::ReadRxAbortReason,
            HardIrqOperation::ReadTxAbortReason,
            HardIrqOperation::ReadEdRss,
            HardIrqOperation::ReadCcaBusy,
            HardIrqOperation::Acknowledge(0xa5),
            HardIrqOperation::Post,
        ]
    );
}

#[test]
fn hard_irq_returns_the_exact_acknowledged_token_rejected_by_the_sink() {
    let events = Ieee802154Event::TxDone
        .mask()
        .union(Ieee802154Event::TxSfdDone.mask());
    let (mut port, accepting_sink) = hard_irq_fixture(0x5a, Ok(events), None, None, -37, false);
    let rejecting_sink = RejectingAcknowledgedSink {
        operations: Rc::clone(&accepting_sink.operations),
    };

    let rejected = match handle_interrupt(&mut port, &rejecting_sink) {
        Ieee802154InterruptDisposition::HandoffRejected(rejected) => rejected,
        disposition => panic!("expected rejected handoff, got {disposition:?}"),
    };

    assert_eq!(rejected.event_classification(), Ok(events));
    assert_eq!(rejected.ed_rss_code(), -37);
    assert_eq!(
        *accepting_sink.operations.borrow(),
        [
            HardIrqOperation::Status,
            HardIrqOperation::ReadEventClassification,
            HardIrqOperation::ReadRxAbortReason,
            HardIrqOperation::ReadTxAbortReason,
            HardIrqOperation::ReadEdRss,
            HardIrqOperation::ReadCcaBusy,
            HardIrqOperation::Acknowledge(0x5a),
            HardIrqOperation::Post,
        ]
    );
}

#[test]
fn hard_irq_posts_semantic_events_and_named_reason_observations() {
    let events = Ieee802154Event::RxAbort
        .mask()
        .union(Ieee802154Event::TxAbort.mask())
        .union(Ieee802154Event::EdDone.mask());
    let (mut port, sink) = hard_irq_fixture(
        7,
        Ok(events),
        Some(Ieee802154RxAbortReason::CoexistenceBreak.into()),
        Some(Ieee802154TxAbortReason::TxSecurityError.into()),
        -101,
        false,
    );

    assert_eq!(
        handle_interrupt(&mut port, &sink),
        Ieee802154InterruptDisposition::Posted
    );
    let acknowledged = sink
        .posted
        .borrow_mut()
        .take()
        .expect("the acknowledged observation was posted");
    assert_eq!(acknowledged.event_classification(), Ok(events));
    assert_eq!(
        acknowledged.rx_abort_reason(),
        Some(Ieee802154RxAbortReasonObservation::Named(
            Ieee802154RxAbortReason::CoexistenceBreak
        ))
    );
    assert_eq!(
        acknowledged.tx_abort_reason(),
        Some(Ieee802154TxAbortReasonObservation::Named(
            Ieee802154TxAbortReason::TxSecurityError
        ))
    );
    assert_eq!(acknowledged.ed_rss_code(), -101);
    assert!(!acknowledged.cca_busy());
}

#[test]
fn zero_event_snapshot_is_spurious_without_acknowledgement_or_post() {
    let (mut port, sink) =
        hard_irq_fixture(3, Ok(Ieee802154EventMask::NONE), None, None, i8::MIN, true);

    assert_eq!(
        handle_interrupt(&mut port, &sink),
        Ieee802154InterruptDisposition::Spurious
    );
    assert_eq!(
        *sink.operations.borrow(),
        [
            HardIrqOperation::Status,
            HardIrqOperation::ReadEventClassification
        ]
    );
    assert!(sink.posted.borrow().is_none());
}

#[test]
fn unclassified_semantic_evidence_is_acknowledged_before_handoff() {
    let (mut port, sink) = hard_irq_fixture(
        9,
        Err(Ieee802154EventObservationError),
        Some(Ieee802154RxAbortReasonObservation::Unclassified),
        Some(Ieee802154TxAbortReasonObservation::Unclassified),
        -1,
        true,
    );

    assert_eq!(
        handle_interrupt(&mut port, &sink),
        Ieee802154InterruptDisposition::Posted
    );
    let acknowledged = sink
        .posted
        .borrow_mut()
        .take()
        .expect("unclassified semantic evidence is posted for fail-closed handling");
    assert_eq!(
        acknowledged.event_classification(),
        Err(Ieee802154EventObservationError)
    );
    assert_eq!(
        acknowledged.rx_abort_reason(),
        Some(Ieee802154RxAbortReasonObservation::Unclassified)
    );
    assert_eq!(
        acknowledged.tx_abort_reason(),
        Some(Ieee802154TxAbortReasonObservation::Unclassified)
    );
    assert_eq!(acknowledged.ed_rss_code(), -1);
    assert!(acknowledged.cca_busy());
}
