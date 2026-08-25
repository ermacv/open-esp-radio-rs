#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Embassy wake and bottom-half composition for the ESP32-S31 IEEE 802.15.4
//! MAC.
//!
//! The hard interrupt samples all event sidebands and acknowledges the exact
//! hardware snapshot before publishing a value here. The Embassy task owns
//! command execution and affine DMA resources; neither PAC handles nor raw
//! addresses cross the async handoff.

#[cfg(test)]
extern crate std;

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, TrySendError},
};
use open_esp_radio_esp32s31_ieee802154_irq::{
    Ieee802154AcknowledgedInterrupt, Ieee802154AcknowledgedInterruptSink,
};
use open_esp_radio_esp32s31_ieee802154_runtime::{
    AcknowledgedMacEventBatch, MacCommandExecutor, MacInterruptBatchError, MacRuntimeActive,
    MacRuntimeBatchOutcome, MacRuntimeBatchRejected, MacRuntimeCompletion,
};

mod owner;

pub use owner::{
    EmbassyIeee802154Active, EmbassyIeee802154DmaResolved, EmbassyIeee802154DmaRunToReadyError,
    EmbassyIeee802154NoDmaResolved, EmbassyIeee802154Ready,
};

/// The hard IRQ acknowledged more snapshots than the bounded queue retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154IrqOverflow;

/// Complete stale IRQ handoff state removed at an epoch boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154IrqDrain {
    /// Number of acknowledged values discarded from the bounded queue.
    pub acknowledged_events: usize,
    /// Whether at least one already-acknowledged value was rejected by the
    /// bounded queue.
    pub overflowed: bool,
}

/// Bounded ISR-to-Embassy handoff for one IEEE 802.15.4 interrupt epoch.
///
/// Unlike the Wi-Fi wake signal, this queue preserves every acknowledged MAC
/// value: IEEE 802.15.4 event status is no longer durable after the hard ISR
/// clears it. Queue overflow returns the exact rejected token to the hard IRQ
/// and marks the async operation failed; it is never treated as a coalesced
/// wake.
pub struct EmbassyIeee802154IrqRuntime<M: RawMutex, const DEPTH: usize> {
    acknowledged: Channel<M, Ieee802154AcknowledgedInterrupt, DEPTH>,
    overflowed: AtomicBool,
}

impl<M: RawMutex, const DEPTH: usize> EmbassyIeee802154IrqRuntime<M, DEPTH> {
    /// Construct empty static IRQ handoff state.
    pub const fn new() -> Self {
        Self {
            acknowledged: Channel::new(),
            overflowed: AtomicBool::new(false),
        }
    }

    /// Wait for the next acknowledged hard-IRQ value.
    ///
    /// If any publication overflowed, the queued value used to wake this task
    /// is discarded and the complete operation must fail closed.
    pub async fn wait(&self) -> Result<Ieee802154AcknowledgedInterrupt, Ieee802154IrqOverflow> {
        let acknowledged = self.acknowledged.receive().await;
        if self.overflowed.swap(false, Ordering::AcqRel) {
            Err(Ieee802154IrqOverflow)
        } else {
            Ok(acknowledged)
        }
    }

    /// Take one already queued IRQ value without waiting.
    pub fn try_take(
        &self,
    ) -> Option<Result<Ieee802154AcknowledgedInterrupt, Ieee802154IrqOverflow>> {
        let acknowledged = self.acknowledged.try_receive().ok()?;
        if self.overflowed.swap(false, Ordering::AcqRel) {
            Some(Err(Ieee802154IrqOverflow))
        } else {
            Some(Ok(acknowledged))
        }
    }

    /// Discard all publications after the hardware route is quiesced.
    pub fn drain(&self) -> Ieee802154IrqDrain {
        let mut acknowledged_events = 0;
        while self.acknowledged.try_receive().is_ok() {
            acknowledged_events += 1;
        }
        Ieee802154IrqDrain {
            acknowledged_events,
            overflowed: self.overflowed.swap(false, Ordering::AcqRel),
        }
    }
}

impl<M: RawMutex, const DEPTH: usize> Ieee802154AcknowledgedInterruptSink
    for EmbassyIeee802154IrqRuntime<M, DEPTH>
{
    fn post(
        &self,
        acknowledged: Ieee802154AcknowledgedInterrupt,
    ) -> Result<(), Ieee802154AcknowledgedInterrupt> {
        match self.acknowledged.try_send(acknowledged) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(rejected)) => {
                self.overflowed.store(true, Ordering::Release);
                Err(rejected)
            }
        }
    }
}

impl<M: RawMutex, const DEPTH: usize> Default for EmbassyIeee802154IrqRuntime<M, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// Fail-closed reason returned by one async MAC operation step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyIeee802154OperationError {
    /// The bounded ISR handoff lost at least one acknowledged snapshot.
    IrqOverflow,
    /// An acknowledged snapshot could not form a valid event batch.
    Interrupt(MacInterruptBatchError),
    /// The pure MAC actor rejected the event batch in its active phase.
    Rejected(
        /// Exact pure actor rejection reason.
        open_esp_radio_esp32s31_ieee802154_mac::MacBatchRejectReason,
    ),
}

/// Result of one cancellation-safe async bottom-half step.
pub enum EmbassyIeee802154OperationProgress<R, E: MacCommandExecutor> {
    /// The operation remains active and is retained by its owner.
    Pending,
    /// A terminal MAC event completed the operation.
    Completed(MacRuntimeCompletion<R, E>),
}

/// Cancellation-safe owner of one IRQ-driven MAC operation.
///
/// The active affine owner remains in this object while [`Self::advance`]
/// awaits an IRQ. Cancelling that future therefore drops only a borrow, not
/// command-register or DMA ownership. After the await, decoding and actor
/// advancement are synchronous and contain no cancellation point.
pub struct EmbassyIeee802154Operation<R, E: MacCommandExecutor> {
    active: Option<MacRuntimeActive<R, E>>,
    quarantine: Option<EmbassyIeee802154OperationQuarantine<R, E>>,
}

#[allow(
    dead_code,
    reason = "quarantined affine owners are deliberately retained without a recovery API"
)]
enum EmbassyIeee802154OperationQuarantine<R, E: MacCommandExecutor> {
    LostOrUndecodable(MacRuntimeActive<R, E>),
    Rejected(MacRuntimeBatchRejected<R, E>),
}

impl<R, E: MacCommandExecutor> EmbassyIeee802154Operation<R, E> {
    /// Bind an already started task-side runtime to the Embassy bottom half.
    pub const fn new(active: MacRuntimeActive<R, E>) -> Self {
        Self {
            active: Some(active),
            quarantine: None,
        }
    }

    /// Return whether this owner still retains an active MAC operation.
    pub const fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Return whether an acknowledged, lost, or rejected IRQ irreversibly
    /// quarantined the exact runtime and resources.
    pub const fn is_quarantined(&self) -> bool {
        self.quarantine.is_some()
    }

    /// Recover the active owner only when an IRQ await was cancelled before it
    /// consumed an acknowledged value.
    ///
    /// After an IRQ overflow, decode failure, or actor rejection this returns
    /// `None`; the exact owner remains irreversibly quarantined.
    pub fn into_active(self) -> Option<MacRuntimeActive<R, E>> {
        self.active
    }

    /// Wait for and apply exactly one acknowledged interrupt value.
    pub async fn advance<M: RawMutex, const DEPTH: usize>(
        &mut self,
        irq: &EmbassyIeee802154IrqRuntime<M, DEPTH>,
    ) -> Result<EmbassyIeee802154OperationProgress<R, E>, EmbassyIeee802154OperationError> {
        assert!(
            self.active.is_some(),
            "a completed or quarantined operation cannot consume another IRQ"
        );
        let interrupt = irq.wait().await;
        // No cancellation point exists after the await: if it was cancelled
        // while pending, `active` stayed in `self`; once the await returned an
        // acknowledged/lost epoch, this synchronous section owns the outcome.
        let active = self
            .active
            .take()
            .expect("a completed or quarantined operation cannot be advanced");
        let interrupt = match interrupt {
            Ok(interrupt) => interrupt,
            Err(_) => {
                self.quarantine = Some(EmbassyIeee802154OperationQuarantine::LostOrUndecodable(
                    active,
                ));
                return Err(EmbassyIeee802154OperationError::IrqOverflow);
            }
        };
        let phase = active.phase();
        let batch = match AcknowledgedMacEventBatch::from_interrupt(interrupt, phase) {
            Ok(batch) => batch,
            Err(error) => {
                self.quarantine = Some(EmbassyIeee802154OperationQuarantine::LostOrUndecodable(
                    active,
                ));
                return Err(EmbassyIeee802154OperationError::Interrupt(error));
            }
        };
        match active.process_batch(batch) {
            Ok(MacRuntimeBatchOutcome::Pending(active)) => {
                self.active = Some(active);
                Ok(EmbassyIeee802154OperationProgress::Pending)
            }
            Ok(MacRuntimeBatchOutcome::Completed(completed)) => {
                Ok(EmbassyIeee802154OperationProgress::Completed(completed))
            }
            Err(rejected) => {
                let reason = rejected.reason();
                self.quarantine = Some(EmbassyIeee802154OperationQuarantine::Rejected(rejected));
                Err(EmbassyIeee802154OperationError::Rejected(reason))
            }
        }
    }

    /// Drive acknowledged IRQ values until the operation completes.
    ///
    /// Cancelling this borrowed future preserves the active owner in `self`.
    pub async fn run<M: RawMutex, const DEPTH: usize>(
        &mut self,
        irq: &EmbassyIeee802154IrqRuntime<M, DEPTH>,
    ) -> Result<MacRuntimeCompletion<R, E>, EmbassyIeee802154OperationError> {
        loop {
            match self.advance(irq).await? {
                EmbassyIeee802154OperationProgress::Pending => {}
                EmbassyIeee802154OperationProgress::Completed(completed) => {
                    return Ok(completed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_esp32s31_ieee802154_irq::{
        Ieee802154Event, acknowledged_interrupt_for_validation,
    };
    use open_esp_radio_esp32s31_ieee802154_mac::{MacNoDmaResources, MacReady};
    use open_esp_radio_esp32s31_ieee802154_runtime::{MacRuntime, ValidationMacCommandExecutor};

    use super::*;

    fn publish<M: RawMutex, const DEPTH: usize>(
        irq: &EmbassyIeee802154IrqRuntime<M, DEPTH>,
        events: u16,
        ed_rss: i8,
    ) -> Result<(), Ieee802154AcknowledgedInterrupt> {
        irq.post(acknowledged_interrupt_for_validation(
            events, 0, 0, ed_rss, false,
        ))
    }

    fn active_cca() -> MacRuntimeActive<MacNoDmaResources, ValidationMacCommandExecutor> {
        MacRuntime::for_validation()
            .start(MacReady::new().request_clear_channel_assessment())
            .unwrap()
    }

    #[test]
    fn acknowledged_values_cross_the_async_handoff_in_order() {
        let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 2>::new();
        publish(&irq, Ieee802154Event::TxSfdDone.bit(), -20).unwrap();
        publish(&irq, Ieee802154Event::TxDone.bit(), -21).unwrap();

        let first = block_on(irq.wait()).unwrap();
        let second = block_on(irq.wait()).unwrap();
        assert_eq!(first.raw_event_bits(), Ieee802154Event::TxSfdDone.bit());
        assert_eq!(first.ed_rss_code(), -20);
        assert_eq!(second.raw_event_bits(), Ieee802154Event::TxDone.bit());
        assert_eq!(second.ed_rss_code(), -21);
    }

    #[test]
    fn overflow_is_a_fail_closed_operation_error() {
        let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
        publish(&irq, Ieee802154Event::TxSfdDone.bit(), 0).unwrap();
        let rejected = publish(&irq, Ieee802154Event::TxDone.bit(), 0)
            .expect_err("the full handoff returns the exact rejected token");
        assert_eq!(rejected.raw_event_bits(), Ieee802154Event::TxDone.bit());

        assert!(matches!(block_on(irq.wait()), Err(Ieee802154IrqOverflow)));
        assert_eq!(irq.drain(), Ieee802154IrqDrain::default());
    }

    #[test]
    fn cancellation_before_an_irq_keeps_the_exact_active_owner_recoverable() {
        let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
        let mut operation = EmbassyIeee802154Operation::new(active_cca());
        let mut future = std::boxed::Box::pin(operation.advance(&irq));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        drop(future);

        assert!(operation.is_active());
        assert!(!operation.is_quarantined());
        assert!(operation.into_active().is_some());
    }

    #[test]
    fn consumed_overflow_decode_and_rejection_errors_quarantine_the_owner() {
        let overflow_irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
        publish(&overflow_irq, Ieee802154Event::TxSfdDone.bit(), 0).unwrap();
        let rejected = publish(&overflow_irq, Ieee802154Event::TxDone.bit(), 0)
            .expect_err("the full handoff returns the exact rejected token");
        assert_eq!(rejected.raw_event_bits(), Ieee802154Event::TxDone.bit());
        let mut overflow = EmbassyIeee802154Operation::new(active_cca());
        assert!(matches!(
            block_on(overflow.advance(&overflow_irq)),
            Err(EmbassyIeee802154OperationError::IrqOverflow)
        ));
        assert!(!overflow.is_active());
        assert!(overflow.is_quarantined());
        assert!(overflow.into_active().is_none());

        let decode_irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
        publish(&decode_irq, 1 << 7, 0).unwrap();
        let mut decode = EmbassyIeee802154Operation::new(active_cca());
        assert!(matches!(
            block_on(decode.advance(&decode_irq)),
            Err(EmbassyIeee802154OperationError::Interrupt(
                MacInterruptBatchError::UnsupportedEventBits { .. }
            ))
        ));
        assert!(!decode.is_active());
        assert!(decode.is_quarantined());
        assert!(decode.into_active().is_none());

        let rejected_irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
        publish(&rejected_irq, Ieee802154Event::TxDone.bit(), 0).unwrap();
        let mut rejected = EmbassyIeee802154Operation::new(active_cca());
        assert!(matches!(
            block_on(rejected.advance(&rejected_irq)),
            Err(EmbassyIeee802154OperationError::Rejected(_))
        ));
        assert!(!rejected.is_active());
        assert!(rejected.is_quarantined());
        assert!(rejected.into_active().is_none());
    }

    #[test]
    fn quiesced_epoch_drain_reports_every_stale_value() {
        let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 2>::new();
        publish(&irq, Ieee802154Event::RxSfdDone.bit(), 0).unwrap();
        publish(&irq, Ieee802154Event::RxDone.bit(), 0).unwrap();

        assert_eq!(
            irq.drain(),
            Ieee802154IrqDrain {
                acknowledged_events: 2,
                overflowed: false,
            }
        );
        assert!(irq.try_take().is_none());
    }
}
