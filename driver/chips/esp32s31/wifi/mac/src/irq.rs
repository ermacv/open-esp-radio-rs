//! Allocation-free MAC ISR to Embassy-task event handoff.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use open_esp_radio_esp32s31_hal::{
    MacInterruptEvents, MacInterruptMask, MacInterruptObservation, MacInterruptRegisters,
    MacInterruptSnapshot, MacPowerInterruptObservation, MacPowerInterruptRegisters,
    MacPowerInterruptSnapshot,
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

/// Finite MAC interrupt capability used by the hard ISR.
///
/// Production delegates both operations to generated PAC registers. Its
/// associated snapshot is opaque and can only be acknowledged once. Tests
/// model the same ordering with semantic observations and no MMIO identity.
pub trait MacInterruptStatusSnapshot {
    fn observation(&self) -> MacInterruptObservation;
}

impl MacInterruptStatusSnapshot for MacInterruptSnapshot {
    fn observation(&self) -> MacInterruptObservation {
        MacInterruptSnapshot::observation(self)
    }
}

impl MacInterruptStatusSnapshot for MacInterruptObservation {
    fn observation(&self) -> MacInterruptObservation {
        *self
    }
}

pub trait MacPowerInterruptStatusSnapshot {
    fn observation(&self) -> MacPowerInterruptObservation;
}

impl MacPowerInterruptStatusSnapshot for MacPowerInterruptSnapshot {
    fn observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptSnapshot::observation(self)
    }
}

impl MacPowerInterruptStatusSnapshot for MacPowerInterruptObservation {
    fn observation(&self) -> MacPowerInterruptObservation {
        *self
    }
}

pub trait MacInterrupt {
    type Snapshot: MacInterruptStatusSnapshot;

    fn status(&mut self) -> Self::Snapshot;
    fn mask_rx_delivery(&mut self) {}
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
}

impl MacInterrupt for MacInterruptRegisters {
    type Snapshot = MacInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.mac_interrupt_status()
    }

    fn mask_rx_delivery(&mut self) {
        self.mask_rx_delivery_interrupts();
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_mac_interrupts(snapshot);
    }
}

/// Finite WDEVPWR interrupt capability used by the hard ISR.
///
/// The PAC snapshot exposes the reviewed TSF-timer causes as named fields and
/// groups every still-unknown cause as unhandled. Policy code never receives a
/// register image or an unqualified hardware bit number.
pub trait MacPowerInterrupt {
    type Snapshot: MacPowerInterruptStatusSnapshot;

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
    /// Driver-local task event mask, never a hardware register image.
    pub events: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrqSnapshot {
    /// Whether the generated PAC observed at least one asserted STATUS field.
    pub had_status: bool,
    /// Driver-local task event mask posted by this handler invocation.
    pub posted_events: u32,
    /// Known STATUS fields acknowledged without an independent work item.
    pub had_auxiliary_event: bool,
    /// At least one non-dispatched STATUS field was asserted.
    pub had_unhandled_event: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqDisposition {
    Posted,
    Spurious,
    /// A nonzero image was acknowledged but contained no task-side work.
    AcknowledgedOnly,
}

/// Duplicate interrupts coalesce by driver-local event kind, matching the C worker latch.
pub trait IrqSink {
    fn post(&self, events: u32);
    fn record_unhandled_event(&self);

    /// Whether this connected epoch owns task-side RX source moderation.
    fn moderate_rx_success(&self) -> bool {
        false
    }
}

/// Sink for acknowledged WDEVPWR causes classified by the PAC.
pub trait PowerIrqSink {
    fn post_power(&self, observation: MacPowerInterruptObservation);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerIrqSnapshot {
    pub observation: MacPowerInterruptObservation,
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
    pending_events: AtomicU32,
    observed_unhandled: AtomicBool,
}

impl IrqState {
    pub const fn new() -> Self {
        Self {
            pending_events: AtomicU32::new(0),
            observed_unhandled: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn post(&self, events: u32) {
        self.pending_events.fetch_or(events, Ordering::Release);
    }

    #[inline]
    pub fn record_unhandled_event(&self) {
        self.observed_unhandled.store(true, Ordering::Relaxed);
    }

    pub fn observed_unhandled(&self) -> bool {
        self.observed_unhandled.load(Ordering::Relaxed)
    }

    /// Take one pending action in the instruction-exact vendor ISR order.
    ///
    /// Duplicate edges still coalesce by task event kind, matching `pp_post` for
    /// these event kinds. Unlike [`try_take`](Self::try_take), this method
    /// preserves the ordering between different kinds and is suitable for a
    /// run-to-completion Embassy dispatcher.
    pub fn try_take_next(&self) -> Option<IrqWork> {
        let mut pending = self.pending_events.load(Ordering::Acquire);
        loop {
            let work = next_irq_work(pending)?;
            match self.pending_events.compare_exchange_weak(
                pending,
                pending & !work.event_bit(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(work),
                Err(current) => pending = current,
            }
        }
    }

    pub fn try_take(&self) -> Option<IrqEvent> {
        let events = self.pending_events.swap(0, Ordering::AcqRel);
        (events != 0).then_some(IrqEvent { events })
    }
}

/// Select one action from a driver-local task event set in recovered vendor order.
///
/// Keeping selection pure lets validation probes exercise the same production
/// decision without depending on the executor's atomic-instruction model.
pub const fn next_irq_work(pending: u32) -> Option<IrqWork> {
    if pending & EVENT_RX_SUCCESS != 0 {
        Some(IrqWork::RxSuccess)
    } else if pending & EVENT_TX_COMPLETE != 0 {
        Some(IrqWork::TxComplete)
    } else if pending & EVENT_TX_TIMEOUT != 0 {
        Some(IrqWork::TxTimeout)
    } else if pending & EVENT_COLLISION != 0 {
        Some(IrqWork::Collision)
    } else {
        None
    }
}

impl IrqSink for IrqState {
    fn post(&self, events: u32) {
        IrqState::post(self, events);
    }

    fn record_unhandled_event(&self) {
        IrqState::record_unhandled_event(self);
    }
}

impl Default for IrqState {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn event_mask(causes: MacInterruptEvents) -> u32 {
    let mut mask = 0;
    if causes.tx_complete() {
        mask |= EVENT_TX_COMPLETE;
    }
    if causes.tx_timeout() {
        mask |= EVENT_TX_TIMEOUT;
    }
    if causes.collision() {
        mask |= EVENT_COLLISION;
    }
    if causes.rx_success() {
        mask |= EVENT_RX_SUCCESS;
    }
    mask
}

/// Handles exactly one MAC status snapshot and posts only the four recovered
/// task-side events. The complete status snapshot is acknowledged before the
/// task can run, including unsupported bits, matching the pinned common ISR.
pub fn handle_mac_irq<M: MacInterrupt, S: IrqSink>(
    interrupt: &mut M,
    sink: &S,
) -> (IrqDisposition, IrqSnapshot) {
    let status_snapshot = interrupt.status();
    let observation = status_snapshot.observation();
    let work_events = observation.work_events();
    let posted_events = event_mask(work_events);
    let snapshot = IrqSnapshot {
        had_status: !observation.is_empty(),
        posted_events,
        had_auxiliary_event: observation.has_auxiliary_event(),
        had_unhandled_event: observation.has_unhandled_event(),
    };

    if observation.is_empty() {
        return (IrqDisposition::Spurious, snapshot);
    }

    if work_events.contains(MacInterruptEvents::RX_SUCCESS) && sink.moderate_rx_success() {
        // Mask before W1C. A completion racing this edge remains latched and
        // becomes visible when the bottom half restores RX delivery.
        interrupt.mask_rx_delivery();
    }
    interrupt.acknowledge(status_snapshot);
    // Avoid an atomic RMW on every qualified RX interrupt. The two observed
    // auxiliary bits are acknowledged above but neither they nor a wholly
    // known work image need to mutate unknown-event telemetry.
    if observation.has_unhandled_event() {
        sink.record_unhandled_event();
    }
    if work_events.is_empty() {
        (IrqDisposition::AcknowledgedOnly, snapshot)
    } else {
        sink.post(posted_events);
        (IrqDisposition::Posted, snapshot)
    }
}

/// Acknowledge one complete masked WDEVPWR snapshot before publishing its causes.
///
/// This intentionally stops at the ISR/executor boundary. Decoding beacon
/// miss, sleep-limit, TSF or other causes requires separate vendor evidence
/// and is not implied by an otherwise unhandled cause.
pub fn handle_power_irq<P: MacPowerInterrupt, S: PowerIrqSink>(
    interrupt: &mut P,
    sink: &S,
) -> (PowerIrqDisposition, PowerIrqSnapshot) {
    let status_snapshot = interrupt.status();
    let observation = status_snapshot.observation();
    let snapshot = PowerIrqSnapshot { observation };
    if observation.is_empty() {
        return (PowerIrqDisposition::Spurious, snapshot);
    }

    interrupt.acknowledge(status_snapshot);
    sink.post_power(observation);
    (PowerIrqDisposition::Posted, snapshot)
}
