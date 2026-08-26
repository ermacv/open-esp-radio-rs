//! Interrupt vocabulary and executor-neutral hard-IRQ handoff for the
//! ESP32-S31 IEEE 802.15.4 MAC.
//!
//! This crate defines the finite hard-IRQ handoff: sample one opaque status
//! snapshot and its sidebands, acknowledge that exact snapshot, then move a
//! non-replayable value to the executor-side sink. Its restricted PAC adapter
//! supplies the production event/status MMIO port. CPU interrupt binding,
//! route enable, and Embassy integration remain platform concerns. The PAC
//! classifies the complete event field before it crosses this boundary;
//! unclassified observations remain opaque and fail closed without leaking
//! register positions.
//!
//! The production port receives an affine PAC snapshot of the complete
//! fourteen-bit `EVENT_STATUS` field. Acknowledgement consumes that exact
//! snapshot, so an ISR cannot manufacture, clone, or replay a W1C image.
//!
//! Register identities and values below are audited against ESP-IDF commit
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`:
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/include/soc/interrupts.h#L139-L153>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/interrupt_core0_reg.h#L2786-L2805>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/interrupt_core1_reg.h#L2786-L2805>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/reg_base.h#L137-L138>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L43-L122>,
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/include/esp_intr_alloc.h#L135-L169>,
//! and
//! <https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/ieee802154/driver/esp_ieee802154_dev.c#L782-L938>.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod pac_port;

/// The only interrupt source identity represented by this crate.
///
/// No constructor from an integer is provided, so an arbitrary peripheral
/// source cannot be confused with the IEEE 802.15.4 MAC source.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
pub enum Ieee802154InterruptSource {
    /// `ETS_MODEM_ZB_MAC_INTR_SOURCE` in the pinned ESP32-S31 source table.
    ModemZbMac = 132,
}

impl Ieee802154InterruptSource {
    /// Return the audited peripheral interrupt source number.
    pub const fn number(self) -> u16 {
        self as u16
    }
}

/// The audited ESP32-S31 IEEE 802.15.4 MAC interrupt source.
pub const IEEE802154_MAC_INTERRUPT_SOURCE: Ieee802154InterruptSource =
    Ieee802154InterruptSource::ModemZbMac;

pub use open_esp_radio_esp32s31_pac::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservationError, Ieee802154RxAbortReason,
    Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReason,
    Ieee802154TxAbortReasonObservation,
};

trait InterruptSnapshot {
    fn event_classification(&self) -> Result<Ieee802154EventMask, Ieee802154EventObservationError>;
    fn rx_abort_reason(&self) -> Option<Ieee802154RxAbortReasonObservation>;
    fn tx_abort_reason(&self) -> Option<Ieee802154TxAbortReasonObservation>;
    fn ed_rss_code(&self) -> i8;
    fn cca_busy(&self) -> bool;
}

trait InterruptPort {
    type Snapshot: InterruptSnapshot;
    fn status(&mut self) -> Self::Snapshot;
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
}

/// One non-replayable interrupt observation after its exact snapshot was
/// acknowledged.
///
/// The constructor is private and this type is neither [`Clone`] nor [`Copy`].
/// Moving it into [`Ieee802154AcknowledgedInterruptSink::post`] is the only
/// public way to cross the hard-IRQ boundary with acknowledgement evidence.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154AcknowledgedInterrupt;
///
/// let fabricated = Ieee802154AcknowledgedInterrupt::new();
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154AcknowledgedInterrupt;
///
/// fn require_replayable<T: Clone>() {}
/// require_replayable::<Ieee802154AcknowledgedInterrupt>();
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct Ieee802154AcknowledgedInterrupt {
    event_classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
    rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
    tx_abort_reason: Option<Ieee802154TxAbortReasonObservation>,
    ed_rss_code: i8,
    cca_busy: bool,
}

impl Ieee802154AcknowledgedInterrupt {
    const fn new(
        event_classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
        rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
        tx_abort_reason: Option<Ieee802154TxAbortReasonObservation>,
        ed_rss_code: i8,
        cca_busy: bool,
    ) -> Self {
        Self {
            event_classification,
            rx_abort_reason,
            tx_abort_reason,
            ed_rss_code,
            cca_busy,
        }
    }

    /// Return the PAC classification of the complete sampled event field.
    pub const fn event_classification(
        &self,
    ) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        self.event_classification
    }

    /// Return typed RX-abort evidence captured before acknowledgement.
    pub const fn rx_abort_reason(&self) -> Option<Ieee802154RxAbortReasonObservation> {
        self.rx_abort_reason
    }

    /// Return typed TX-abort evidence captured before acknowledgement.
    pub const fn tx_abort_reason(&self) -> Option<Ieee802154TxAbortReasonObservation> {
        self.tx_abort_reason
    }

    /// Return the uncalibrated signed energy-detection RSS code.
    pub const fn ed_rss_code(&self) -> i8 {
        self.ed_rss_code
    }

    /// Return the sampled clear-channel busy indication.
    pub const fn cca_busy(&self) -> bool {
        self.cca_busy
    }
}

/// Executor-side receiver of one acknowledged interrupt observation.
pub trait Ieee802154AcknowledgedInterruptSink {
    /// Post a non-replayable observation after hard-IRQ acknowledgement.
    ///
    /// A saturated sink must return the exact rejected value. A hard IRQ can
    /// therefore quarantine acknowledged evidence instead of silently losing
    /// the affine token after hardware status has already been cleared.
    fn post(
        &self,
        acknowledged: Ieee802154AcknowledgedInterrupt,
    ) -> Result<(), Ieee802154AcknowledgedInterrupt>;
}

/// Construct acknowledged evidence for explicit non-target validation probes.
#[cfg(all(feature = "validation-probes", not(target_arch = "riscv32")))]
#[doc(hidden)]
pub const fn acknowledged_interrupt_for_validation(
    event_classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
    rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
    tx_abort_reason: Option<Ieee802154TxAbortReasonObservation>,
    ed_rss_code: i8,
    cca_busy: bool,
) -> Ieee802154AcknowledgedInterrupt {
    Ieee802154AcknowledgedInterrupt::new(
        event_classification,
        rx_abort_reason,
        tx_abort_reason,
        ed_rss_code,
        cca_busy,
    )
}

/// Outcome of one finite hard-IRQ invocation.
#[derive(Debug, Eq, PartialEq)]
pub enum Ieee802154InterruptDisposition {
    /// A nonzero event snapshot was acknowledged and posted.
    Posted,
    /// Hardware status was acknowledged, but the sink returned the exact
    /// value it could not retain.
    HandoffRejected(Ieee802154AcknowledgedInterrupt),
    /// The sampled semantic event set was empty, so nothing was acknowledged
    /// or posted.
    Spurious,
}

/// Handle one status epoch from the restricted ESP32-S31 PAC owner.
///
/// An empty classified event set is treated as spurious. For every nonempty or
/// unclassified observation, all semantic sidebands are copied before the
/// exact opaque snapshot is acknowledged. The acknowledged value is posted
/// only afterwards, so bottom-half validation can fail closed without leaving
/// hardware status latched.
///
/// A downstream fake port with a no-op acknowledgement cannot mint evidence:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::{
///     Ieee802154AcknowledgedInterrupt, Ieee802154AcknowledgedInterruptSink,
///     Ieee802154InterruptPort, Ieee802154InterruptSnapshot,
///     handle_ieee802154_interrupt,
/// };
///
/// struct FakePort;
/// impl Ieee802154InterruptPort for FakePort {
/// }
///
/// struct Sink;
/// impl Ieee802154AcknowledgedInterruptSink for Sink {
///     fn post(
///         &self,
///         acknowledged: Ieee802154AcknowledgedInterrupt,
///     ) -> Result<(), Ieee802154AcknowledgedInterrupt> {
///         Ok(())
///     }
/// }
///
/// let _ = handle_ieee802154_interrupt(&mut FakePort, &Sink);
/// ```
pub fn handle_ieee802154_interrupt<Sink: Ieee802154AcknowledgedInterruptSink + ?Sized>(
    port: &mut open_esp_radio_esp32s31_pac::Ieee802154InterruptRegisters,
    sink: &Sink,
) -> Ieee802154InterruptDisposition {
    handle_interrupt(port, sink)
}

fn handle_interrupt<Port: InterruptPort, Sink: Ieee802154AcknowledgedInterruptSink + ?Sized>(
    port: &mut Port,
    sink: &Sink,
) -> Ieee802154InterruptDisposition {
    let snapshot = port.status();
    let event_classification = snapshot.event_classification();
    if matches!(event_classification, Ok(Ieee802154EventMask::NONE)) {
        return Ieee802154InterruptDisposition::Spurious;
    }

    let rx_abort_reason = snapshot.rx_abort_reason();
    let tx_abort_reason = snapshot.tx_abort_reason();
    let ed_rss_code = snapshot.ed_rss_code();
    let cca_busy = snapshot.cca_busy();

    port.acknowledge(snapshot);
    let acknowledged = Ieee802154AcknowledgedInterrupt::new(
        event_classification,
        rx_abort_reason,
        tx_abort_reason,
        ed_rss_code,
        cca_busy,
    );
    match sink.post(acknowledged) {
        Ok(()) => Ieee802154InterruptDisposition::Posted,
        Err(rejected) => Ieee802154InterruptDisposition::HandoffRejected(rejected),
    }
}

/// One callback position in the reviewed vendor event-dispatch order.
///
/// The closed enum makes the two receive-abort phases explicit and makes no
/// claim about status acknowledgement or next-operation policy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154DispatchedEvent {
    /// Receive-abort processing before SFD and completion events.
    RxAbortPhase1,
    /// RX SFD completion.
    RxSfdDone,
    /// TX SFD completion.
    TxSfdDone,
    /// TX completion.
    TxDone,
    /// RX completion.
    RxDone,
    /// ACK TX completion.
    AckTxDone,
    /// ACK RX completion.
    AckRxDone,
    /// Receive-abort processing after ACK completion events.
    RxAbortPhase2,
    /// TX abort processing.
    TxAbort,
    /// Energy-detection completion.
    EdDone,
    /// Timer-zero overflow.
    Timer0Overflow,
    /// Timer-one overflow.
    Timer1Overflow,
}

/// Consumer of the closed, pure event-dispatch sequence.
pub trait Ieee802154EventSink {
    /// Observe one source-confirmed callback position.
    fn on_event(&mut self, event: Ieee802154DispatchedEvent);
}

/// Failure to dispatch a batch containing a named but unsupported event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154DispatchError {
    unsupported_events: Ieee802154EventMask,
}

impl Ieee802154DispatchError {
    /// Return every semantic event rejected before dispatch began.
    pub const fn unsupported_events(self) -> Ieee802154EventMask {
        self.unsupported_events
    }
}

/// Dispatch one already sampled event batch in reviewed vendor order.
///
/// Validation is transactional: if any named event lacks a reviewed handler,
/// the function returns before invoking the sink. This pure function neither
/// reads nor acknowledges `EVENT_STATUS` and does not run a next-operation
/// policy.
pub fn dispatch_event_batch<S: Ieee802154EventSink + ?Sized>(
    batch: Ieee802154EventMask,
    sink: &mut S,
) -> Result<(), Ieee802154DispatchError> {
    let unsupported_events = batch.difference(Ieee802154EventMask::VENDOR_HANDLED);
    if !unsupported_events.is_empty() {
        return Err(Ieee802154DispatchError { unsupported_events });
    }

    if batch.contains(Ieee802154Event::RxAbort) {
        sink.on_event(Ieee802154DispatchedEvent::RxAbortPhase1);
    }
    if batch.contains(Ieee802154Event::RxSfdDone) {
        sink.on_event(Ieee802154DispatchedEvent::RxSfdDone);
    }
    if batch.contains(Ieee802154Event::TxSfdDone) {
        sink.on_event(Ieee802154DispatchedEvent::TxSfdDone);
    }
    if batch.contains(Ieee802154Event::TxDone) {
        sink.on_event(Ieee802154DispatchedEvent::TxDone);
    }
    if batch.contains(Ieee802154Event::RxDone) {
        sink.on_event(Ieee802154DispatchedEvent::RxDone);
    }
    if batch.contains(Ieee802154Event::AckTxDone) {
        sink.on_event(Ieee802154DispatchedEvent::AckTxDone);
    }
    if batch.contains(Ieee802154Event::AckRxDone) {
        sink.on_event(Ieee802154DispatchedEvent::AckRxDone);
    }
    if batch.contains(Ieee802154Event::RxAbort) {
        sink.on_event(Ieee802154DispatchedEvent::RxAbortPhase2);
    }
    if batch.contains(Ieee802154Event::TxAbort) {
        sink.on_event(Ieee802154DispatchedEvent::TxAbort);
    }
    if batch.contains(Ieee802154Event::EdDone) {
        sink.on_event(Ieee802154DispatchedEvent::EdDone);
    }
    if batch.contains(Ieee802154Event::Timer0Overflow) {
        sink.on_event(Ieee802154DispatchedEvent::Timer0Overflow);
    }
    if batch.contains(Ieee802154Event::Timer1Overflow) {
        sink.on_event(Ieee802154DispatchedEvent::Timer1Overflow);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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
        dispatch_event_batch(Ieee802154Event::RxAbort.mask(), &mut sink)
            .expect("RX abort is handled");
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
        dispatch_event_batch(Ieee802154EventMask::NONE, &mut sink)
            .expect("empty batch is supported");
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
        fn event_classification(
            &self,
        ) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
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
}
