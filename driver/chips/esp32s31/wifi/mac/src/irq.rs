//! Allocation-free MAC ISR to Embassy-task event handoff.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_esp32s31_hal::{
    MacInterruptEvents, MacInterruptMask, MacInterruptRegisters, MacInterruptSnapshot,
    MacPowerInterruptRegisters, MacPowerInterruptSnapshot,
};

/// Platform route which lends both interrupt-register capabilities to hard
/// handlers for one finite connected epoch.
///
/// Implementations own CPU routing and stable ISR storage. They must disable
/// the routes and recover both HAL capabilities before `quiesce` returns. Their
/// active representation must also be safe to retain indefinitely: a lost
/// higher-level epoch forgets the installed route rather than dropping ISR
/// storage which hardware can still reach, and role control then requires
/// reset before another epoch can be constructed.
pub trait MacInterruptRoute {
    type Platform: ?Sized;
    type Setup;
    type Error;

    fn activate(
        &mut self,
        platform: &Self::Platform,
        setup: Self::Setup,
        event_mask: MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)>;

    fn quiesce(&mut self, platform: &Self::Platform) -> Result<Self::Setup, Self::Error>;
}

pub const MAC_INT_TX_COMPLETE: u32 = 0x0000_0080;
pub const MAC_INT_COLLISION: u32 = 0x0000_0100;
pub const MAC_INT_WATCHDOG: u32 = 0x0000_0800;
pub const MAC_INT_RX_SUCCESS: u32 = 0x0000_4000;
pub const MAC_INT_TX_TIMEOUT: u32 = 0x0008_0000;

/// Status bits observed alongside [`MAC_INT_RX_SUCCESS`] during sustained
/// HE20 receive traffic. The vendor FIQ acknowledges both in its full-image
/// W1C but does not dispatch either as an independent work item. Their
/// hardware semantics remain unknown, so they intentionally stay outside
/// [`HANDLED_MAC_MASK`].
pub const MAC_INT_RX_ASSOCIATED_AUXILIARY_5: u32 =
    MacInterruptEvents::RX_ASSOCIATED_AUXILIARY_5.bits();
pub const MAC_INT_RX_ASSOCIATED_AUXILIARY_24: u32 =
    MacInterruptEvents::RX_ASSOCIATED_AUXILIARY_24.bits();
pub const MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK: u32 =
    MAC_INT_RX_ASSOCIATED_AUXILIARY_5 | MAC_INT_RX_ASSOCIATED_AUXILIARY_24;

/// Finite MAC interrupt capability used by the hard ISR.
///
/// Production delegates both operations to generated PAC registers. Its
/// associated snapshot is opaque and can only be acknowledged once. Tests
/// can model the same ordering with a plain value and no MMIO identity.
pub trait InterruptStatusSnapshot {
    fn bits(&self) -> u32;
}

impl InterruptStatusSnapshot for u32 {
    fn bits(&self) -> u32 {
        *self
    }
}

impl InterruptStatusSnapshot for MacInterruptSnapshot {
    fn bits(&self) -> u32 {
        MacInterruptSnapshot::bits(self)
    }
}

impl InterruptStatusSnapshot for MacPowerInterruptSnapshot {
    fn bits(&self) -> u32 {
        MacPowerInterruptSnapshot::bits(self)
    }
}

pub trait MacInterrupt {
    type Snapshot: InterruptStatusSnapshot;

    fn status(&mut self) -> Self::Snapshot;
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
}

impl MacInterrupt for MacInterruptRegisters {
    type Snapshot = MacInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.mac_interrupt_status()
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_mac_interrupts(snapshot);
    }
}

/// Finite opaque WDEVPWR interrupt capability used by the hard ISR.
///
/// Cause meanings deliberately do not appear in this trait. The current
/// qualification proves only the complete masked STATUS image and exact W1C
/// acknowledgement; policy code must not infer sleep semantics from an
/// unqualified bit number.
pub trait MacPowerInterrupt {
    type Snapshot: InterruptStatusSnapshot;

    fn status(&mut self) -> Self::Snapshot;
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
}

impl MacPowerInterrupt for MacPowerInterruptRegisters {
    type Snapshot = MacPowerInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.power_interrupt_status()
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_power_interrupts(snapshot);
    }
}

pub const EVENT_TX_COMPLETE: u32 = 0x01;
pub const EVENT_TX_TIMEOUT: u32 = 0x02;
pub const EVENT_COLLISION: u32 = 0x04;
pub const EVENT_RX_SUCCESS: u32 = 0x08;
pub const EVENT_KNOWN_MASK: u32 = 0x0f;

pub const HANDLED_MAC_MASK: u32 =
    MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION | MAC_INT_RX_SUCCESS;

/// One run-to-completion MAC action in the order used by the vendor ISR.
///
/// This is deliberately not an arbitrary bit mask. Complete
/// `libpp.a[wdev.o]::wDev_ProcessFiq` handles RX success before TX
/// completion, TX timeout and collision when one interrupt snapshot contains
/// several causes. Those leaves publish separate `ppTask` queue entries, and
/// complete `libpp.a[pp.o]::ppTask` consumes one entry before
/// receiving the next. An executor port must therefore be able to retain that
/// ordering instead of merging RX and TX into one indistinguishable wakeup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqWork {
    RxSuccess,
    TxComplete,
    TxTimeout,
    Collision,
}

impl IrqWork {
    pub const fn mac_bit(self) -> u32 {
        match self {
            Self::RxSuccess => MAC_INT_RX_SUCCESS,
            Self::TxComplete => MAC_INT_TX_COMPLETE,
            Self::TxTimeout => MAC_INT_TX_TIMEOUT,
            Self::Collision => MAC_INT_COLLISION,
        }
    }

    pub const fn event_bit(self) -> u32 {
        match self {
            Self::RxSuccess => EVENT_RX_SUCCESS,
            Self::TxComplete => EVENT_TX_COMPLETE,
            Self::TxTimeout => EVENT_TX_TIMEOUT,
            Self::Collision => EVENT_COLLISION,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrqEvent {
    pub mac_pending: u32,
    pub event_mask: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrqSnapshot {
    pub status: u32,
    pub pending: u32,
    pub handled: u32,
    /// Known W1C status which does not create an independent work item.
    pub auxiliary: u32,
    /// Bits which have neither a qualified work mapping nor an observed
    /// acknowledgement-only classification.
    pub unhandled: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqDisposition {
    Posted,
    Spurious,
    /// A nonzero image was acknowledged but contained no task-side work.
    AcknowledgedOnly,
}

/// Duplicate interrupts coalesce by raw MAC bit, matching the C worker latch.
pub trait IrqSink {
    fn post(&self, mac_pending: u32);
    fn record_unhandled(&self, bits: u32);
}

/// Sink for an acknowledged but otherwise opaque WDEVPWR event image.
pub trait PowerIrqSink {
    fn post_power(&self, pending: u32);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerIrqSnapshot {
    pub status: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerIrqDisposition {
    Posted,
    Spurious,
}

/// Executor-neutral coalescing state shared by the ISR and its task.
///
/// The executor integration decides how a posted edge wakes the task. This
/// keeps the MAC crate independent of Embassy while retaining the exact
/// interrupt/event mapping.
pub struct IrqState {
    pending_mac: AtomicU32,
    observed_unhandled: AtomicU32,
}

impl IrqState {
    pub const fn new() -> Self {
        Self {
            pending_mac: AtomicU32::new(0),
            observed_unhandled: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn post(&self, mac_pending: u32) {
        self.pending_mac.fetch_or(mac_pending, Ordering::Release);
    }

    #[inline]
    pub fn record_unhandled(&self, bits: u32) {
        self.observed_unhandled.fetch_or(bits, Ordering::Relaxed);
    }

    pub fn observed_unhandled(&self) -> u32 {
        self.observed_unhandled.load(Ordering::Relaxed)
    }

    /// Take one pending action in the instruction-exact vendor ISR order.
    ///
    /// Duplicate edges still coalesce by raw MAC bit, matching `pp_post` for
    /// these event kinds. Unlike [`try_take`](Self::try_take), this method
    /// preserves the ordering between different kinds and is suitable for a
    /// run-to-completion Embassy dispatcher.
    pub fn try_take_next(&self) -> Option<IrqWork> {
        let mut pending = self.pending_mac.load(Ordering::Acquire);
        loop {
            let work = next_irq_work(pending)?;
            match self.pending_mac.compare_exchange_weak(
                pending,
                pending & !work.mac_bit(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(work),
                Err(current) => pending = current,
            }
        }
    }

    pub fn try_take(&self) -> Option<IrqEvent> {
        let mac_pending = self.pending_mac.swap(0, Ordering::AcqRel);
        (mac_pending != 0).then(|| IrqEvent {
            mac_pending,
            event_mask: event_mask(mac_pending),
        })
    }
}

/// Select one action from a raw pending image in recovered vendor order.
///
/// Keeping selection pure lets validation probes exercise the same production
/// decision without depending on the executor's atomic-instruction model.
pub const fn next_irq_work(pending: u32) -> Option<IrqWork> {
    if pending & MAC_INT_RX_SUCCESS != 0 {
        Some(IrqWork::RxSuccess)
    } else if pending & MAC_INT_TX_COMPLETE != 0 {
        Some(IrqWork::TxComplete)
    } else if pending & MAC_INT_TX_TIMEOUT != 0 {
        Some(IrqWork::TxTimeout)
    } else if pending & MAC_INT_COLLISION != 0 {
        Some(IrqWork::Collision)
    } else {
        None
    }
}

impl IrqSink for IrqState {
    fn post(&self, mac_pending: u32) {
        self.post(mac_pending);
    }

    fn record_unhandled(&self, bits: u32) {
        self.record_unhandled(bits);
    }
}

impl Default for IrqState {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn event_mask(mac_pending: u32) -> u32 {
    let mut events = 0;
    if mac_pending & MAC_INT_TX_COMPLETE != 0 {
        events |= EVENT_TX_COMPLETE;
    }
    if mac_pending & MAC_INT_TX_TIMEOUT != 0 {
        events |= EVENT_TX_TIMEOUT;
    }
    if mac_pending & MAC_INT_COLLISION != 0 {
        events |= EVENT_COLLISION;
    }
    if mac_pending & MAC_INT_RX_SUCCESS != 0 {
        events |= EVENT_RX_SUCCESS;
    }
    events
}

/// Handles exactly one MAC status snapshot and posts only the four recovered
/// task-side events. The complete status snapshot is acknowledged before the
/// task can run, including unsupported bits, matching the pinned common ISR.
pub fn handle_mac_irq<M: MacInterrupt, S: IrqSink>(
    interrupt: &mut M,
    sink: &S,
) -> (IrqDisposition, IrqSnapshot) {
    let status_snapshot = interrupt.status();
    let status = status_snapshot.bits();
    let handled = status & HANDLED_MAC_MASK;
    let auxiliary = status & MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK;
    let unhandled = status & !(HANDLED_MAC_MASK | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK);
    let snapshot = IrqSnapshot {
        status,
        pending: status,
        handled,
        auxiliary,
        unhandled,
    };

    if status == 0 {
        return (IrqDisposition::Spurious, snapshot);
    }

    interrupt.acknowledge(status_snapshot);
    // Avoid an atomic RMW on every qualified RX interrupt. The two observed
    // auxiliary bits are acknowledged above but neither they nor a wholly
    // known work image need to mutate unknown-event telemetry.
    if unhandled != 0 {
        sink.record_unhandled(unhandled);
    }
    if handled == 0 {
        (IrqDisposition::AcknowledgedOnly, snapshot)
    } else {
        sink.post(handled);
        (IrqDisposition::Posted, snapshot)
    }
}

/// Acknowledge one complete masked WDEVPWR snapshot before publishing it.
///
/// This intentionally stops at the ISR/executor boundary. Decoding beacon
/// miss, sleep-limit, TSF or other causes requires separate vendor evidence
/// and is not implied by a nonzero raw bit.
pub fn handle_power_irq<P: MacPowerInterrupt, S: PowerIrqSink>(
    interrupt: &mut P,
    sink: &S,
) -> (PowerIrqDisposition, PowerIrqSnapshot) {
    let status_snapshot = interrupt.status();
    let status = status_snapshot.bits();
    let snapshot = PowerIrqSnapshot { status };
    if status == 0 {
        return (PowerIrqDisposition::Spurious, snapshot);
    }

    interrupt.acknowledge(status_snapshot);
    sink.post_power(status);
    (PowerIrqDisposition::Posted, snapshot)
}
