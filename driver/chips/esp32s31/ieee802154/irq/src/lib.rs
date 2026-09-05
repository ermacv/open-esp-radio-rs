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
mod tests;
