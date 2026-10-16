//! Executor-independent, coalesced IRQ facts for the S31 driver.
//!
//! The hard handler acknowledges the hardware snapshot before publishing here.
//! The driver takes a finite batch and visits RX success before TX completion;
//! descriptor and completion rings retain multiplicity. No work is executed in
//! the ISR. A waiting lane has one driver waiter; register its waker before
//! checking facts. Drain only after the route has stopped publishing, or retain
//! the drained facts when suspending the same epoch.
//!
//! Publication functions are placed in `.rwtext.open_radio_irq` on RV32.
//! The composition must also place this storage and its registered waker's
//! callback/data in ISR-accessible memory. An indirect executor callback is
//! outside this component's placement proof.

use atomic_waker::AtomicWaker;
use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::Poll,
};
use oer_esp32s31_hal::types::MacPowerInterruptObservation;

use super::{
    EVENT_COLLISION, EVENT_KNOWN_MASK, EVENT_RX_SUCCESS, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT,
    IrqSink, IrqWork, PowerIrqSink, next_irq_work,
};

const TX_EVENTS: u32 = EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT | EVENT_COLLISION;
const RX_CAPACITY: u32 = 1 << 4;

mod sealed {
    pub trait Sealed {}
}

/// RX delivery policy retained by the interrupt owner.
pub trait RxDelivery: sealed::Sealed {
    fn active(&self) -> bool;
    fn end(&self);
    fn unmask_after_drain(&self) -> bool;
}

/// A route without the capability to restore moderated RX delivery.
///
/// ```compile_fail,E0599
/// use oer_esp32s31_ieee80211_mac::irq::consumer::{IrqConsumer, Unmoderated};
/// let irq = IrqConsumer::new(Unmoderated);
/// irq.begin_rx_moderation();
/// ```
/// A restore capability makes the transition available:
/// ```
/// use oer_esp32s31_ieee80211_mac::irq::consumer::{IrqConsumer, Moderated};
/// let irq = IrqConsumer::new(Moderated::new(|| {}));
/// irq.begin_rx_moderation();
/// assert!(irq.unmask_rx_after_drain());
/// ```
pub struct Unmoderated;

impl sealed::Sealed for Unmoderated {}
impl RxDelivery for Unmoderated {
    #[inline(always)]
    fn active(&self) -> bool {
        false
    }
    fn end(&self) {}
    fn unmask_after_drain(&self) -> bool {
        false
    }
}

/// A route whose platform can restore RX delivery after the durable drain.
pub struct Moderated {
    active: AtomicBool,
    unmask: fn(),
}

impl Moderated {
    pub const fn new(unmask: fn()) -> Self {
        Self {
            active: AtomicBool::new(false),
            unmask,
        }
    }
}

impl sealed::Sealed for Moderated {}
impl RxDelivery for Moderated {
    #[inline(always)]
    fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    fn end(&self) {
        self.active.store(false, Ordering::Release);
    }
    fn unmask_after_drain(&self) -> bool {
        if !self.active() {
            return false;
        }
        (self.unmask)();
        true
    }
}

/// Only a policy with a concrete restore capability can enable moderation.
pub trait CanModerate: RxDelivery {
    fn begin(&self);
}

impl CanModerate for Moderated {
    fn begin(&self) {
        self.active.store(true, Ordering::Release);
    }
}

/// One bounded snapshot of coalesced MAC facts.
///
/// Iteration always yields RX success, TX completion, timeout, then collision.
/// New IRQs remain pending for the next batch; continuous RX cannot extend this
/// iterator or postpone the driver's command/watchdog/retire step.
pub struct IrqBatch {
    pending: u32,
}

impl IrqBatch {
    pub const fn rx_capacity_returned(&self) -> bool {
        self.pending & RX_CAPACITY != 0
    }
    pub const fn is_empty(&self) -> bool {
        self.pending == 0
    }
}

impl Iterator for IrqBatch {
    type Item = IrqWork;
    fn next(&mut self) -> Option<Self::Item> {
        let work = next_irq_work(self.pending)?;
        self.pending &= !work.event_bit();
        Some(work)
    }
}

/// Facts removed at a route boundary, retained for same-epoch suspension.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IrqDrain {
    pub rx: bool,
    pub rx_capacity: bool,
    pub tx_events: u32,
}

/// Driver-side MAC IRQ handoff, also borrowed by the hard handler as a sink.
///
/// A combined hardware snapshot is published with one Release operation. The
/// driver observes it with one Acquire batch, preserving RX-before-TX even if
/// the ISR preempts a poll. Lane waits support the separate finite RX/TX
/// services; a lane has only one waiter and is not consumed concurrently with
/// batch processing. Registering or polling performs no hardware operation.
pub struct IrqConsumer<M: RxDelivery = Unmoderated> {
    pending: AtomicU32,
    rx: AtomicWaker,
    capacity: AtomicWaker,
    tx: AtomicWaker,
    unhandled: AtomicBool,
    moderation: M,
    #[cfg(any(feature = "irq-counters", test))]
    rx_posts: AtomicU32,
}

impl<M: RxDelivery> IrqConsumer<M> {
    pub const fn new(moderation: M) -> Self {
        Self {
            pending: AtomicU32::new(0),
            rx: AtomicWaker::new(),
            capacity: AtomicWaker::new(),
            tx: AtomicWaker::new(),
            unhandled: AtomicBool::new(false),
            moderation,
            #[cfg(any(feature = "irq-counters", test))]
            rx_posts: AtomicU32::new(0),
        }
    }

    oer_esp32s31_ieee80211_dma::place_irq_path! {
    /// Publish acknowledged facts; duplicate work coalesces, IRQ counts do not.
    #[inline(never)]
    pub fn publish(&self, events: u32) {
        let events = events & EVENT_KNOWN_MASK;
        #[cfg(any(feature = "irq-counters", test))]
        if events & EVENT_RX_SUCCESS != 0 {
            self.rx_posts.fetch_add(1, Ordering::Relaxed);
        }
        self.post(events);
    }
    }

    #[inline(always)]
    fn post(&self, events: u32) {
        let previous = self.pending.fetch_or(events, Ordering::Release);
        let added = events & !previous;
        if added & EVENT_RX_SUCCESS != 0 {
            self.rx.wake();
        }
        if added & RX_CAPACITY != 0 {
            self.capacity.wake();
        }
        if added & TX_EVENTS != 0 {
            self.tx.wake();
        }
    }

    fn take(&self, mask: u32) -> u32 {
        self.pending.fetch_and(!mask, Ordering::AcqRel) & mask
    }

    /// Take one finite batch. Work arriving afterwards belongs to the next step.
    pub fn take_batch(&self) -> IrqBatch {
        IrqBatch {
            pending: self.pending.swap(0, Ordering::AcqRel),
        }
    }

    pub async fn wait_batch(&self) -> IrqBatch {
        poll_fn(|cx| {
            self.rx.register(cx.waker());
            self.capacity.register(cx.waker());
            self.tx.register(cx.waker());
            let batch = self.take_batch();
            if batch.is_empty() {
                Poll::Pending
            } else {
                Poll::Ready(batch)
            }
        })
        .await
    }

    async fn wait_lane(&self, mask: u32, waker: &AtomicWaker) -> u32 {
        poll_fn(|cx| {
            waker.register(cx.waker());
            let events = self.take(mask);
            if events == 0 {
                Poll::Pending
            } else {
                Poll::Ready(events)
            }
        })
        .await
    }

    pub async fn wait_rx(&self) {
        self.wait_lane(EVENT_RX_SUCCESS, &self.rx).await;
    }
    pub async fn wait_rx_capacity(&self) {
        self.wait_lane(RX_CAPACITY, &self.capacity).await;
    }
    pub async fn wait_tx(&self) -> u32 {
        self.wait_lane(TX_EVENTS, &self.tx).await
    }

    /// Probe the durable RX frontier after handoff; this is not IRQ evidence.
    pub fn notify_rx_handoff(&self) {
        self.post(EVENT_RX_SUCCESS);
    }
    /// A returned staging credit wakes the driver independently of RX IRQs.
    pub fn notify_rx_capacity(&self) {
        self.post(RX_CAPACITY);
    }
    #[inline(always)]
    pub fn rx_signaled(&self) -> bool {
        self.pending.load(Ordering::Acquire) & EVENT_RX_SUCCESS != 0
    }
    pub fn rx_capacity_signaled(&self) -> bool {
        self.pending.load(Ordering::Acquire) & RX_CAPACITY != 0
    }
    #[inline(always)]
    pub fn tx_signaled(&self) -> bool {
        self.pending.load(Ordering::Acquire) & TX_EVENTS != 0
    }
    pub fn try_take_tx(&self) -> Option<u32> {
        let events = self.take(TX_EVENTS);
        (events != 0).then_some(events)
    }

    pub fn drain_pending(&self) -> IrqDrain {
        let batch = self.take_batch();
        IrqDrain {
            rx: batch.pending & EVENT_RX_SUCCESS != 0,
            rx_capacity: batch.rx_capacity_returned(),
            tx_events: batch.pending & TX_EVENTS,
        }
    }

    /// Wrapping count of hardware RX publications; zero without irq-counters.
    pub fn rx_post_count(&self) -> u32 {
        #[cfg(any(feature = "irq-counters", test))]
        {
            self.rx_posts.load(Ordering::Relaxed)
        }
        #[cfg(not(any(feature = "irq-counters", test)))]
        {
            0
        }
    }

    pub fn observed_unhandled(&self) -> bool {
        self.unhandled.load(Ordering::Relaxed)
    }
    pub fn is_rx_moderation_active(&self) -> bool {
        self.moderation.active()
    }
    pub fn end_rx_moderation(&self) {
        self.moderation.end();
    }
    pub fn unmask_rx_after_drain(&self) -> bool {
        self.moderation.unmask_after_drain()
    }
}

impl<M: CanModerate> IrqConsumer<M> {
    pub fn begin_rx_moderation(&self) {
        self.moderation.begin();
    }
}

impl Default for IrqConsumer {
    fn default() -> Self {
        Self::new(Unmoderated)
    }
}

impl<M: RxDelivery> IrqSink for IrqConsumer<M> {
    #[inline(always)]
    fn post(&self, events: u32) {
        self.publish(events);
    }
    #[inline(always)]
    fn record_unhandled_event(&self) {
        self.unhandled.store(true, Ordering::Relaxed);
    }
    #[inline(always)]
    fn moderate_rx_success(&self) -> bool {
        self.moderation.active()
    }
}

/// Coalesced semantic power facts; local bits never encode a register image.
pub struct PowerIrqConsumer {
    pending: AtomicU32,
    waker: AtomicWaker,
}

impl PowerIrqConsumer {
    const TSF0: u32 = 1;
    const TSF1: u32 = 2;
    const TSF2: u32 = 4;
    const TSF3: u32 = 8;
    const TBTT: u32 = 16;
    const UNHANDLED: u32 = 32;

    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            waker: AtomicWaker::new(),
        }
    }

    oer_esp32s31_ieee80211_dma::place_irq_path! {
    #[inline(never)]
    pub fn publish(&self, observation: MacPowerInterruptObservation) {
        let events = (u32::from(observation.tsf_timer_0()) * Self::TSF0)
            | (u32::from(observation.tsf_timer_1()) * Self::TSF1)
            | (u32::from(observation.tsf_timer_2()) * Self::TSF2)
            | (u32::from(observation.tsf_timer_3()) * Self::TSF3)
            | (u32::from(observation.sta_tbtt()) * Self::TBTT)
            | (u32::from(observation.has_unhandled_event()) * Self::UNHANDLED);
        let previous = self.pending.fetch_or(events, Ordering::Release);
        if events & !previous != 0 {
            self.waker.wake();
        }
    }

    }

    fn observation(events: u32) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            events & Self::TSF0 != 0,
            events & Self::TSF1 != 0,
            events & Self::TSF2 != 0,
            events & Self::TSF3 != 0,
            events & Self::TBTT != 0,
            events & Self::UNHANDLED != 0,
        )
    }

    pub fn try_take(&self) -> Option<MacPowerInterruptObservation> {
        let events = self.pending.swap(0, Ordering::AcqRel);
        (events != 0).then(|| Self::observation(events))
    }

    pub async fn wait(&self) -> MacPowerInterruptObservation {
        poll_fn(|cx| {
            self.waker.register(cx.waker());
            self.try_take().map_or(Poll::Pending, Poll::Ready)
        })
        .await
    }

    pub fn drain_pending(&self) -> MacPowerInterruptObservation {
        Self::observation(self.pending.swap(0, Ordering::AcqRel))
    }
}

impl Default for PowerIrqConsumer {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerIrqSink for PowerIrqConsumer {
    #[inline(always)]
    fn post_power(&self, observation: MacPowerInterruptObservation) {
        self.publish(observation);
    }
}

#[cfg(test)]
mod tests;
