//! One permanent, installation-owned SRAM region for driver and IRQ registration.
//!
//! The future owns its affine pinned lease, never the backing allocation.
//! Bounded quiescence returns that lease or quarantines it permanently. Neither
//! cancellation nor forgetting the future can reclaim memory an entered actor
//! still uses. Context and the output latch are outside the registered graph.

use atomic_waker::AtomicWaker;
use core::{
    cell::{Cell, UnsafeCell},
    future::poll_fn,
    marker::PhantomPinned,
    mem::{ManuallyDrop, MaybeUninit},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use oer_esp32s31_hal::{
    owner::{ConnectedStaInterruptPrepared, RadioRuntimeOwner},
    types::{MacPowerInterruptObservation, MacPowerWakeCause},
};
use oer_esp32s31_ieee80211_mac::irq::{
    IrqSnapshot, PowerIrqSnapshot,
    consumer::{
        IrqBatch, IrqConsumer, IrqPublication, IrqPublisher, IrqStorage, PowerIrqConsumer,
        PowerIrqPublisher, PowerIrqStorage,
    },
    epoch::{ActiveEpoch, InactiveEpoch, Moderated},
};
use oer_time::Instant;

pub mod commands;
pub mod driver;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeSources {
    Commands,
    Interrupts,
    CommandsAndInterrupts,
}

/// A ready finite step, or concrete events and a future watchdog deadline.
#[must_use = "the executor must arrange every wake in this plan"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakePlan {
    Ready,
    Wait {
        sources: WakeSources,
        deadline: FutureDeadline,
    },
}

/// A deadline checked against the fresh clock sample at the end of a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FutureDeadline {
    at: Instant,
}
impl FutureDeadline {
    pub const fn at(self) -> Instant {
        self.at
    }
}
impl WakePlan {
    pub fn until(now: Instant, at: Instant, sources: WakeSources) -> Self {
        if at <= now {
            Self::Ready
        } else {
            Self::Wait {
                sources,
                deadline: FutureDeadline { at },
            }
        }
    }
}

#[must_use]
pub enum DriverStep {
    Wake(WakePlan),
    /// Retirement work is complete. The scope still drains all actors and
    /// invokes stop once before returning hardware or its backing lease.
    Retired,
}

/// Facts captured once per finite driver step. New publications are next-step work.
#[must_use = "every captured IRQ fact belongs to this step"]
pub struct DriverEvents {
    pub mac: IrqBatch,
    pub power: MacPowerInterruptObservation,
}

pub struct DriverSignals {
    retire: AtomicBool,
    abandon: AtomicBool,
}
impl DriverSignals {
    pub fn retirement_requested(&self) -> bool {
        self.retire.load(Ordering::Acquire)
    }
    pub fn abandonment_requested(&self) -> bool {
        self.abandon.load(Ordering::Acquire)
    }
}

mod sealed {
    pub trait InterruptOwner {}
}

/// Only the real moderated epoch implements this outside this module's tests.
/// The two register banks are disjoint from the task-side radio owner.
pub trait InterruptOwner: sealed::InterruptOwner + Send + 'static {
    type Detached;
    fn mac(&mut self) -> IrqSnapshot;
    fn power(&mut self) -> PowerIrqSnapshot;
    fn detach(self) -> Self::Detached;
}
impl sealed::InterruptOwner for ActiveEpoch<Moderated> {}
impl InterruptOwner for ActiveEpoch<Moderated> {
    type Detached = InactiveEpoch;
    fn mac(&mut self) -> IrqSnapshot {
        self.service_mac().1
    }
    fn power(&mut self) -> PowerIrqSnapshot {
        self.service_power().1
    }
    fn detach(self) -> InactiveEpoch {
        self.quiesce()
    }
}

/// The sole permanent ISR address and admission domain. An entrant increments
/// before testing admission. Closing admission and observing zero therefore
/// excludes every possible borrow of the old registration on every core.
pub struct ScopeSlot {
    entry: AtomicPtr<Header>,
    open: AtomicBool,
    entered: AtomicUsize,
}
impl ScopeSlot {
    pub const fn new() -> Self {
        Self {
            entry: AtomicPtr::new(ptr::null_mut()),
            open: AtomicBool::new(false),
            entered: AtomicUsize::new(0),
        }
    }

    fn enter(&self) -> Option<Entered<'_>> {
        // SeqCst closes the two-atomic admission race: an entrant not seen by
        // the zero-count observation must see the preceding closed admission.
        self.entered.fetch_add(1, Ordering::SeqCst);
        let guard = Entered(self);
        if self.open.load(Ordering::SeqCst) {
            Some(guard)
        } else {
            None
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "synchronous cancellation has a finite observation bound and cannot yield to an executor"
    )]
    fn close(&self, bound: DrainBound) -> Quiescence {
        self.open.store(false, Ordering::SeqCst);
        for _ in 0..bound.checks {
            if self.entered.load(Ordering::SeqCst) == 0 {
                return Quiescence::Drained;
            }
            #[cfg(test)]
            tests::wait_boundary();
            core::hint::spin_loop();
        }
        Quiescence::TimedOut
    }

    #[allow(
        unsafe_code,
        reason = "the owned waker refers only to the permanent address slot"
    )]
    fn driver_waker(&'static self) -> Waker {
        // SAFETY: the slot itself is static, independently of any lease. Wake
        // enters its admission domain before looking at the current binding;
        // clone/drop need no installation memory or allocation.
        unsafe { Waker::from_raw(RawWaker::new(ptr::from_ref(self).cast(), &DRIVER_WAKER)) }
    }

    #[allow(
        unsafe_code,
        reason = "the static slot admits this scheduling callback independently"
    )]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_irq")
    )]
    #[inline(never)]
    fn notify_driver(&self) {
        let Some(_entered) = self.enter() else {
            return;
        };
        let entry = self.entry.load(Ordering::Acquire);
        // SAFETY: the count protects the current IRQ region. This callback
        // only pends the executor, and cannot borrow Driver or synchronously
        // tear down the installation. Closed/stale wakes reach no region.
        let _scheduled = unsafe { ((*entry).schedule)((*entry).irq_context) };
    }

    /// Run one step on the separate driver executor, on either permitted core.
    ///
    /// # Safety
    /// The executor serializes driver dispatches (including across cores).
    /// Platform callbacks are the ones audited by DriverScope::new. Wakers
    /// only schedule: they must never poll or synchronously tear down a scope.
    #[allow(
        unsafe_code,
        reason = "the exclusive executor invokes its admitted registration"
    )]
    pub unsafe fn dispatch(&self) {
        let completion = {
            let Some(_entered) = self.enter() else {
                return;
            };
            let entry = self.entry.load(Ordering::Acquire);
            // SAFETY: admission keeps the permanent header and its exact
            // driver projection alive; the caller serializes driver steps.
            unsafe { ((*entry).driver)((*entry).driver_context) }
        };
        if let Some(waker) = completion {
            waker.wake();
        }
    }

    /// Capture facts through the IRQ region, never through a Driver borrow.
    /// Composition wakers may only pend execution; synchronous teardown or
    /// polling from clone/wake/drop would violate the installation binding.
    #[allow(
        unsafe_code,
        reason = "the hard entry and its callback graph execute from SRAM"
    )]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_irq")
    )]
    #[inline(never)]
    pub fn mac_interrupt(&self) -> IrqSnapshot {
        match self.capture_mac() {
            Some(publication) => publication.wake(),
            None => IrqSnapshot::default(),
        }
    }
    #[inline(always)]
    fn capture_mac(&self) -> Option<MacPublication> {
        let _entered = self.enter()?;
        critical_section::with(|_| {
            let entry = self.entry.load(Ordering::Acquire);
            #[allow(unsafe_code, reason = "admission retains the separate IRQ region")]
            // SAFETY: the count precedes admission. Release cannot reclaim the
            // registration before this guard exits; timeout retains it forever.
            Some(unsafe { ((*entry).mac)((*entry).irq_context) })
        })
    }

    #[allow(
        unsafe_code,
        reason = "both hard entries share the audited SRAM placement"
    )]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_irq")
    )]
    #[inline(never)]
    pub fn power_interrupt(&self) -> PowerIrqSnapshot {
        match self.capture_power() {
            Some(publication) => publication.wake(),
            None => PowerIrqSnapshot::default(),
        }
    }
    #[inline(always)]
    fn capture_power(&self) -> Option<PowerPublication> {
        let _entered = self.enter()?;
        critical_section::with(|_| {
            let entry = self.entry.load(Ordering::Acquire);
            #[allow(
                unsafe_code,
                reason = "the admission guard covers the whole IRQ transaction"
            )]
            // SAFETY: same admission and permanent-backing proof as MAC.
            Some(unsafe { ((*entry).power)((*entry).irq_context) })
        })
    }
}
impl Default for ScopeSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(
    unsafe_code,
    reason = "the driver's immutable wake table is in the IRQ SRAM closure"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".critical.rodata.open_radio_irq")
)]
static DRIVER_WAKER: RawWakerVTable = RawWakerVTable::new(
    clone_driver_waker,
    wake_driver,
    wake_driver,
    drop_driver_waker,
);

#[allow(
    unsafe_code,
    reason = "clone preserves the permanent slot address without borrowing an installation"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn clone_driver_waker(data: *const ()) -> RawWaker {
    RawWaker::new(data, &DRIVER_WAKER)
}
#[allow(
    unsafe_code,
    reason = "the raw waker was constructed solely from a static ScopeSlot"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn wake_driver(data: *const ()) {
    // SAFETY: only driver_waker/clone_driver_waker construct this vtable and
    // each retains the exact static slot pointer. No DriverScope pointer lives
    // in the waker, so late wakes remain valid after release or quarantine.
    unsafe { &*data.cast::<ScopeSlot>() }.notify_driver();
}
#[allow(
    unsafe_code,
    reason = "the permanent slot needs no wake-data reclamation"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn drop_driver_waker(_: *const ()) {}
struct Entered<'a>(&'a ScopeSlot);
impl Drop for Entered<'_> {
    fn drop(&mut self) {
        self.0.entered.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
enum Quiescence {
    Drained,
    TimedOut,
}

/// A finite number of admission-count observations during synchronous detach.
/// The composition chooses the bound; exhausting it permanently quarantines
/// the backing rather than assuming that an actor must have finished.
#[derive(Clone, Copy)]
pub struct DrainBound {
    checks: usize,
}
impl DrainBound {
    pub const fn checks<const N: usize>() -> Self {
        const {
            assert!(N > 0, "quiescence needs a positive observation bound");
        }
        Self { checks: N }
    }
}

#[macro_export]
macro_rules! declare_scope_slot {
    ($visibility:vis static $name:ident) => {
        #[cfg_attr(target_arch = "riscv32", unsafe(link_section = ".critical.data.open_radio_irq"))]
        $visibility static $name: $crate::scope::ScopeSlot = $crate::scope::ScopeSlot::new();
    };
}

/// Monomorphized scheduling supplied by a composition. It only arranges a
/// later driver step, never polls, destroys a scope, or calls an external waker.
/// A Wait plan arms its sources/deadline; it must never clear an already
/// pending driver-SWI. The executor acknowledges that SWI before dispatch.
/// IRQ publication and retire/abandon can therefore pend it on either side
/// of the driver's final Acquire check without losing a wake.
pub trait WakeScheduler<P>: Sync + 'static {
    fn schedule(&self, platform: &P, plan: WakePlan);
}
impl<P, F: Fn(&P, WakePlan) + Sync + 'static> WakeScheduler<P> for F {
    fn schedule(&self, platform: &P, plan: WakePlan) {
        self(platform, plan);
    }
}

/// By-value platform configuration. Constructing it registers nothing.
#[must_use]
pub struct ScopeRoute<P, E, S = fn(&P, WakePlan)> {
    platform: P,
    slot: &'static ScopeSlot,
    mask: fn(&P),
    acknowledge: fn(&P),
    enable: fn(&P) -> Result<(), E>,
    schedule: S,
}
impl<P, E, S> ScopeRoute<P, E, S> {
    pub const fn new(
        platform: P,
        slot: &'static ScopeSlot,
        mask: fn(&P),
        acknowledge: fn(&P),
        enable: fn(&P) -> Result<(), E>,
        schedule: S,
    ) -> Self {
        Self {
            platform,
            slot,
            mask,
            acknowledge,
            enable,
            schedule,
        }
    }
    pub fn into_platform(self) -> P {
        self.platform
    }
}

/// Caller access cannot escape its body. The publisher shares only atomic
/// memory owned by the installation's backing, never memory inside Driver.
pub struct ScopeAccess<'scope> {
    irq: &'scope IrqPublisher<'scope>,
    abandon: &'scope dyn Fn(),
}
impl ScopeAccess<'_> {
    pub fn irq(&self) -> &IrqPublisher<'_> {
        self.irq
    }
    /// Synchronously mask/close, wait within the configured bound, and stop
    /// a drained driver. A timeout permanently quarantines the installation.
    pub fn abandon(self) {
        (self.abandon)();
    }
}

#[must_use]
pub struct ReturnedOwners<H, I, P, E, S = fn(&P, WakePlan)> {
    pub hardware: H,
    pub interrupts: I,
    pub route: ScopeRoute<P, E, S>,
}
/// Temporary stand-in for the shared core's pending `Quarantined<S>`.
/// The Driver and its returned state use this same holder: dropping it never
/// drops the contents. The DriverLoan integration removes this declaration
/// and uses the core type directly, with no alias or compatibility wrapper.
#[must_use = "abandoned hardware cannot be reused without a physical reset proof"]
pub struct Quarantined<S> {
    _retained: ManuallyDrop<S>,
}
impl<S> Quarantined<S> {
    fn retain(state: S) -> Self {
        Self {
            _retained: ManuallyDrop::new(state),
        }
    }
}
#[must_use]
pub enum DetachedOwners<H> {
    Retired(H),
    Abandoned(Quarantined<H>),
}

/// Successful quiescence returns the affine lease for another installation.
#[must_use]
pub struct ReleasedScope<
    H: Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> {
    pub lease: ScopeLease<H, I, P, E, S>,
    pub owners: DetachedOwners<ReturnedOwners<H, I::Detached, P, E, S>>,
}
/// No lease, owner or HardwareStopped proof can be recovered from this receipt.
/// Hardware may still be running. Masks, closed admission and permanent DMA /
/// owner quarantine preserve memory safety. The composition must report this
/// fault and arrange a chip reset; it must not reinstall or release clocks.
#[must_use = "reset the chip before installing again after permanent quarantine"]
pub struct ScopePoisoned {
    _private: (),
}
#[must_use]
pub enum ScopeReturn<
    H: Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    Released(ReleasedScope<H, I, P, E, S>),
    Poisoned(ScopePoisoned),
}
/// An output slot, outside every registered callback's reachable memory.
/// None means no return was delivered (for example, the future was forgotten).
pub struct AbandonLatch<
    H: Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    returned: Option<ScopeReturn<H, I, P, E, S>>,
}
impl<H: Send + 'static, I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>
    AbandonLatch<H, I, P, E, S>
{
    pub const fn new() -> Self {
        Self { returned: None }
    }
    pub fn into_return(self) -> Option<ScopeReturn<H, I, P, E, S>> {
        self.returned
    }
}
impl<H: Send + 'static, I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>
    Default for AbandonLatch<H, I, P, E, S>
{
    fn default() -> Self {
        Self::new()
    }
}

/// The composition declares one of these in SRAM and transfers its one
/// &'static mut reference at boot (or from its installation Mint). There is
/// no shared take(), installed flag, allocator or fallible second acquisition.
/// Context is not stored here and has no 'static requirement.
pub struct ScopeStorage<
    H: Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    mac: UnsafeCell<IrqStorage>,
    power: UnsafeCell<PowerIrqStorage>,
    irq: BackingCell<IrqRegion<I, P, E, S>>,
    driver: BackingCell<DriverRegion<H, I, P, E, S>>,
    header: BackingCell<Header>,
    // Caller-side synchronous shutdown reply, never addressed by an ISR.
    shutdown: Cell<Option<Quiescence>>,
    _pin: PhantomPinned,
}
type BackingCell<T> = UnsafeCell<MaybeUninit<T>>;
impl<H: Send + 'static, I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>
    ScopeStorage<H, I, P, E, S>
{
    pub const fn new() -> Self {
        Self {
            mac: UnsafeCell::new(IrqStorage::new()),
            power: UnsafeCell::new(PowerIrqStorage::new()),
            irq: UnsafeCell::new(MaybeUninit::uninit()),
            driver: UnsafeCell::new(MaybeUninit::uninit()),
            header: UnsafeCell::new(MaybeUninit::uninit()),
            shutdown: Cell::new(None),
            _pin: PhantomPinned,
        }
    }
    #[allow(
        unsafe_code,
        reason = "the lease initialized this permanent IRQ projection"
    )]
    fn irq(&self) -> &IrqRegion<I, P, E, S> {
        // SAFETY: only the active scope and its matched lifecycle control call
        // this private method, before the explicit outcome leaves the install.
        unsafe { (*self.irq.get()).assume_init_ref() }
    }

    fn close_and_stop(&self, bound: DrainBound) -> Quiescence {
        if let Some(outcome) = self.shutdown.get() {
            return outcome;
        }
        let irq = self.irq();
        (irq.route.mask)(&irq.route.platform);
        let quiescence = irq.route.slot.close(bound);
        (irq.route.acknowledge)(&irq.route.platform);
        if matches!(quiescence, Quiescence::TimedOut) {
            self.shutdown.set(Some(Quiescence::TimedOut));
            return Quiescence::TimedOut;
        }
        irq.route
            .slot
            .entry
            .store(ptr::null_mut(), Ordering::Release);
        if !irq.retired.load(Ordering::Acquire) {
            irq.signals.abandon.store(true, Ordering::Release);
        }
        #[allow(
            unsafe_code,
            reason = "zero admitted actors excludes the driver's only mutable borrow"
        )]
        // SAFETY: admission is closed and every previous projection exited.
        // The stop callback cannot reenter and the permanent lease stays live.
        let driver = unsafe { (*self.driver.get()).assume_init_mut() };
        let mut control = IrqControl {
            owner: &irq.interrupts,
        };
        (driver.callbacks.stop)(&mut driver.hardware, &mut control);
        self.shutdown.set(Some(Quiescence::Drained));
        Quiescence::Drained
    }

    fn abandon(&self, bound: DrainBound) -> Quiescence {
        // Cancellation can drop Driver before the core's backend guard. Its
        // completed reply is outside the extracted regions and stays valid.
        if let Some(outcome) = self.shutdown.get() {
            return outcome;
        }
        let irq = self.irq();
        irq.signals.abandon.store(true, Ordering::Release);
        irq.route
            .schedule
            .schedule(&irq.route.platform, WakePlan::Ready);
        self.close_and_stop(bound)
    }

    #[allow(
        unsafe_code,
        reason = "consuming the unique static borrow permanently pins its backing"
    )]
    pub fn pin_static(storage: &'static mut Self) -> ScopeLease<H, I, P, E, S> {
        // SAFETY: the affine lease never exposes mutable storage or a way to
        // unpin it. Released returns that same lease; Poisoned never does.
        let memory = unsafe { Pin::new_unchecked(storage) };
        ScopeLease { memory }
    }
}
impl<H: Send + 'static, I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>
    Default for ScopeStorage<H, I, P, E, S>
{
    fn default() -> Self {
        Self::new()
    }
}
#[must_use = "retain the unique pinned SRAM lease until release or permanent quarantine"]
pub struct ScopeLease<
    H: Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    memory: Pin<&'static mut ScopeStorage<H, I, P, E, S>>,
}
impl<H: Send + 'static, I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>
    ScopeLease<H, I, P, E, S>
{
    fn storage(&self) -> &ScopeStorage<H, I, P, E, S> {
        self.memory.as_ref().get_ref()
    }
}

/// Register ownership is separate from both Driver and its &mut borrow.
pub struct IrqControl<'scope, I: InterruptOwner = ActiveEpoch<Moderated>> {
    owner: &'scope UnsafeCell<ManuallyDrop<I>>,
}
impl IrqControl<'_, ActiveEpoch<Moderated>> {
    fn with_owner<R>(&mut self, operation: impl FnOnce(&mut ActiveEpoch<Moderated>) -> R) -> R {
        critical_section::with(|_| {
            #[allow(
                unsafe_code,
                reason = "IRQ-bank projections share the bounded register transaction"
            )]
            // SAFETY: admission retains backing; the critical section excludes
            // the hard handlers. This private helper calls only these MMIO leaves.
            operation(unsafe { &mut *self.owner.get() })
        })
    }
    pub fn unmask_rx_after_drain(&mut self) {
        self.with_owner(|irq| irq.unmask_rx_after_drain());
    }
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        self.with_owner(|irq| irq.prepare_connected_sta_without_power_save(radio))
    }
    pub fn mask_and_acknowledge_power_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.with_owner(|irq| irq.mask_and_acknowledge_power_wake_cause(cause));
    }
}

// Lifetime-free wake tails cannot address installation memory after wake.
struct MacPublication {
    snapshot: IrqSnapshot,
    wake: IrqPublication,
}
struct PowerPublication {
    snapshot: PowerIrqSnapshot,
    wake: IrqPublication,
}
impl MacPublication {
    #[inline(always)]
    fn wake(self) -> IrqSnapshot {
        self.wake.wake();
        self.snapshot
    }
}
impl PowerPublication {
    #[inline(always)]
    fn wake(self) -> PowerIrqSnapshot {
        self.wake.wake();
        self.snapshot
    }
}
struct Header {
    driver: unsafe fn(*mut ()) -> Option<Waker>,
    driver_context: *mut (),
    mac: unsafe fn(*mut ()) -> MacPublication,
    power: unsafe fn(*mut ()) -> PowerPublication,
    schedule: unsafe fn(*mut ()) -> Scheduled,
    irq_context: *mut (),
}
// Distinct private result also identifies this callback family in the linked
// SRAM audit; no untyped function-pointer allowlist is needed.
struct Scheduled {
    _published: bool,
}

#[allow(
    unsafe_code,
    reason = "the admitted wake callback sees only the separate IRQ region"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn schedule_entry<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>(
    context: *mut (),
) -> Scheduled {
    // SAFETY: the same count/admission proof as the IRQ capture callbacks;
    // the exact region projection and function were published together.
    let irq = unsafe { &*context.cast::<IrqRegion<I, P, E, S>>() };
    irq.route
        .schedule
        .schedule(&irq.route.platform, WakePlan::Ready);
    Scheduled { _published: true }
}

// The hard entries know only this type. It contains no H, DriverRegion,
// pointer to DriverRegion, context, latch or future reference.
struct IrqRegion<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>> {
    interrupts: UnsafeCell<ManuallyDrop<I>>,
    mac: IrqPublisher<'static>,
    power: PowerIrqPublisher<'static>,
    route: ManuallyDrop<ScopeRoute<P, E, S>>,
    signals: DriverSignals,
    retired: AtomicBool,
    completion: AtomicWaker,
}
struct DriverRegion<
    H: Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> {
    hardware: ManuallyDrop<H>,
    mac: IrqConsumer<'static>,
    power: PowerIrqConsumer<'static>,
    irq: *const IrqRegion<I, P, E, S>,
    callbacks: DriverCallbacks<H, I>,
}

/// The bounded hardware operations bound by the installation's safety proof.
pub struct DriverCallbacks<H, I: InterruptOwner> {
    step: fn(&mut H, &mut IrqControl<'_, I>, DriverEvents, &DriverSignals) -> DriverStep,
    stop: fn(&mut H, &mut IrqControl<'_, I>),
}
impl<H, I: InterruptOwner> DriverCallbacks<H, I> {
    pub const fn new(
        step: fn(&mut H, &mut IrqControl<'_, I>, DriverEvents, &DriverSignals) -> DriverStep,
        stop: fn(&mut H, &mut IrqControl<'_, I>),
    ) -> Self {
        Self { step, stop }
    }
}

#[allow(
    unsafe_code,
    reason = "only the exclusive admitted executor projects DriverRegion"
)]
unsafe fn invoke<
    H: Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
>(
    context: *mut (),
) -> Option<Waker> {
    // SAFETY: header and exact callback are bound together; admitted steps
    // are serialized. IRQ callbacks cannot reach this DriverRegion.
    let driver = unsafe { &mut *context.cast::<DriverRegion<H, I, P, E, S>>() };
    // SAFETY: the admission count retains this separate shared IRQ region.
    let irq = unsafe { &*driver.irq };
    if irq.retired.load(Ordering::Acquire) || irq.signals.abandonment_requested() {
        return None;
    }
    // The permanent slot waker covers software and hardware publications.
    // Registration precedes each Acquire snapshot, never follows its check.
    let waker = irq.route.slot.driver_waker();
    let mut cx = Context::from_waker(&waker);
    let events = DriverEvents {
        mac: match driver.mac.poll_batch(&mut cx) {
            Poll::Ready(batch) => batch,
            Poll::Pending => IrqBatch::default(),
        },
        power: match driver.power.poll(&mut cx) {
            Poll::Ready(observation) => observation,
            Poll::Pending => MacPowerInterruptObservation::default(),
        },
    };
    let mut control = IrqControl {
        owner: &irq.interrupts,
    };
    match (driver.callbacks.step)(&mut driver.hardware, &mut control, events, &irq.signals) {
        DriverStep::Wake(plan) => {
            irq.route.schedule.schedule(&irq.route.platform, plan);
            // Arm before checking. A fact published before the Acquire is
            // visible here; a later publication pends the executor itself.
            // The captured batch stays finite: this work belongs to next step.
            if driver.mac.has_pending()
                || driver.power.has_pending()
                || irq.signals.retirement_requested()
                || irq.signals.abandonment_requested()
            {
                irq.route
                    .schedule
                    .schedule(&irq.route.platform, WakePlan::Ready);
            }
            None
        }
        DriverStep::Retired => {
            irq.retired.store(true, Ordering::Release);
            irq.completion.take()
        }
    }
}

#[allow(
    unsafe_code,
    reason = "the admitted hard callback projects only the disjoint IRQ region"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn mac_entry<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>(
    context: *mut (),
) -> MacPublication {
    // SAFETY: admission retains this exact IRQ region; the caller holds its
    // register critical section. No driver address is present in this type.
    let irq = unsafe { &*context.cast::<IrqRegion<I, P, E, S>>() };
    let measurement = irq.mac.begin_mac_entry();
    // SAFETY: every bank projection shares the same critical section.
    let snapshot = unsafe { (&mut *irq.interrupts.get()).mac() };
    let wake = irq.mac.publish_snapshot_deferred(snapshot);
    MacPublication {
        snapshot,
        wake: measurement.finish(wake),
    }
}
#[allow(
    unsafe_code,
    reason = "power shares the same admission and disjoint IRQ-bank proof"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn power_entry<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>(
    context: *mut (),
) -> PowerPublication {
    // SAFETY: same exact-type, admission and bank-serialization proof as MAC.
    let irq = unsafe { &*context.cast::<IrqRegion<I, P, E, S>>() };
    // SAFETY: the caller serializes every IRQ-bank projection.
    let snapshot = unsafe { (&mut *irq.interrupts.get()).power() };
    let wake = irq.power.publish_deferred(snapshot.observation);
    PowerPublication { snapshot, wake }
}

/// The future owns only the lease and caller data. The permanent backing owns
/// every registered byte, even after Poisoned, cancellation and owning-Pin drop.
#[must_use]
pub struct DriverScope<
    'latch,
    H: Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    lease: ManuallyDrop<ScopeLease<H, I, P, E, S>>,
    latch: &'latch mut AbandonLatch<H, I, P, E, S>,
    bound: DrainBound,
}
impl<
    'latch,
    H: Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> DriverScope<'latch, H, I, P, E, S>
{
    /// Bind one installation to its unique permanent SRAM lease.
    ///
    /// # Safety
    /// Retain the unique platform routing tokens for this slot. Mask disables
    /// MAC/POWER and driver-SWI admission at the hardware routes on all bound
    /// cores; acknowledge clears the SWI. The separate driver executor is
    /// exclusive, including across cores. IRQ-bank operations are serialized
    /// by the platform's cross-core critical-section implementation.
    /// Step/stop and routing callbacks are bounded and do not unwind,
    /// synchronously poll, reenter driver dispatch or destroy the scope.
    /// IRQ preemption is permitted. Step reports Retired only after accounting
    /// for all accepted work; stop stops hardware/DMA before returning and
    /// runs once after quiescence.
    /// It is never invoked on timeout. Schedulers
    /// and composition wakers (including clone/drop) ONLY pend execution: they
    /// never synchronously poll or detach. All reachable callback data survive
    /// forget and lie in the linked SRAM closure. No priority order is a safety
    /// premise. The lease was placed in SRAM by the composition's linker.
    #[allow(
        unsafe_code,
        reason = "one permanent registration primitive binds all external premises"
    )]
    pub unsafe fn new(
        lease: ScopeLease<H, I, P, E, S>,
        hardware: H,
        interrupts: I,
        callbacks: DriverCallbacks<H, I>,
        route: ScopeRoute<P, E, S>,
        bound: DrainBound,
        latch: &'latch mut AbandonLatch<H, I, P, E, S>,
    ) -> Self {
        let storage = lease.storage();
        storage.shutdown.set(None);
        // SAFETY: an unused/Released lease has no live handoff borrower. A new
        // installation begins with its own empty facts, diagnostics and wakers.
        unsafe {
            *storage.mac.get() = IrqStorage::new();
            *storage.power.get() = PowerIrqStorage::new();
        }
        // SAFETY: this affine, unused/Released lease has no live projection.
        // These fields physically live forever. Consumers and publishers are
        // private to this initialized region, destroyed before lease return.
        let (mac, mac_consumer) = unsafe { (&mut *storage.mac.get()).split() };
        // SAFETY: the distinct permanent power handoff has the same ownership.
        let (power, power_consumer) = unsafe { (&mut *storage.power.get()).split() };
        // SAFETY: all three destinations are uninitialized under this lease.
        let irq = unsafe {
            (*storage.irq.get()).write(IrqRegion {
                interrupts: UnsafeCell::new(ManuallyDrop::new(interrupts)),
                mac,
                power,
                route: ManuallyDrop::new(route),
                signals: DriverSignals {
                    retire: AtomicBool::new(false),
                    abandon: AtomicBool::new(false),
                },
                retired: AtomicBool::new(false),
                completion: AtomicWaker::new(),
            })
        };
        // SAFETY: independent driver storage, initialized once for this lease.
        let driver = unsafe {
            (*storage.driver.get()).write(DriverRegion {
                hardware: ManuallyDrop::new(hardware),
                mac: mac_consumer,
                power: power_consumer,
                irq,
                callbacks,
            })
        };
        // SAFETY: the exact projections and callbacks are published together.
        unsafe {
            (*storage.header.get()).write(Header {
                driver: invoke::<H, I, P, E, S>,
                driver_context: ptr::from_mut(driver).cast(),
                mac: mac_entry::<I, P, E, S>,
                power: power_entry::<I, P, E, S>,
                schedule: schedule_entry::<I, P, E, S>,
                irq_context: ptr::from_mut(irq).cast(),
            });
        }
        Self {
            lease: ManuallyDrop::new(lease),
            latch,
            bound,
        }
    }

    #[allow(unsafe_code, reason = "the live lease owns one initialized IRQ region")]
    fn irq(&self) -> &IrqRegion<I, P, E, S> {
        // SAFETY: new initializes it; Drop extracts only after quiescence.
        unsafe { (*self.lease.storage().irq.get()).assume_init_ref() }
    }

    async fn run_body<C, R>(
        &self,
        context: C,
        body: impl for<'scope> AsyncFnOnce(C, ScopeAccess<'scope>) -> R,
    ) -> Result<R, E> {
        let irq = self.irq();
        // run_port polls its body before drive. fail_stop may therefore have
        // closed this installation before the driver's first poll.
        if !irq.signals.abandonment_requested() {
            (irq.route.mask)(&irq.route.platform);
            (irq.route.acknowledge)(&irq.route.platform);
            let header = self.lease.storage().header.get().cast::<Header>();
            irq.route.slot.entry.store(header, Ordering::Release);
            irq.route.slot.open.store(true, Ordering::SeqCst);
            (irq.route.enable)(&irq.route.platform)?;
            irq.route
                .schedule
                .schedule(&irq.route.platform, WakePlan::Ready);
        }
        let abandon = || self.abandon();
        let result = body(
            context,
            ScopeAccess {
                irq: &irq.mac,
                abandon: &abandon,
            },
        )
        .await;
        if irq.signals.abandonment_requested() {
            return Ok(result);
        }
        irq.signals.retire.store(true, Ordering::Release);
        irq.route
            .schedule
            .schedule(&irq.route.platform, WakePlan::Ready);
        poll_fn(|cx| {
            irq.completion.register(cx.waker());
            if irq.retired.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        Ok(result)
    }

    fn abandon(&self) {
        self.lease.storage().abandon(self.bound);
    }

    fn close_and_stop(&self) -> Quiescence {
        self.lease.storage().close_and_stop(self.bound)
    }

    /// Run a body, publishing the returned lease or quarantine into its latch.
    pub async fn run<C, R>(
        self,
        context: C,
        body: impl for<'scope> AsyncFnOnce(C, ScopeAccess<'scope>) -> R,
    ) -> Result<R, E> {
        let result = self.run_body(context, body).await;
        let mut scope = ManuallyDrop::new(self);
        let returned = scope.complete();
        scope.latch.returned = Some(returned);
        result
    }

    /// Consume the scope through ordinary explicit teardown. No outcome relies
    /// on Drop; a timed-out actor keeps the permanent backing in quarantine.
    pub fn finish(self) -> ScopeReturn<H, I, P, E, S> {
        ManuallyDrop::new(self).complete()
    }

    fn complete(&mut self) -> ScopeReturn<H, I, P, E, S> {
        let quiescence = self.close_and_stop();
        if matches!(quiescence, Quiescence::TimedOut) {
            // Do not inspect, move or drop anything an admitted actor can use.
            // Losing this affine lease leaves its static backing quarantined.
            return ScopeReturn::Poisoned(ScopePoisoned { _private: () });
        }
        #[allow(
            unsafe_code,
            reason = "closed admission and zero actors permit unique owner recovery"
        )]
        // SAFETY: all publishers/consumers/owners are initialized and no actor
        // can project them. Moving these values destroys the private borrows
        // before the affine lease returns; its backing remains pinned forever.
        let (mut driver, mut irq, lease) = unsafe {
            let storage = self.lease.storage();
            (
                (*storage.driver.get()).assume_init_read(),
                (*storage.irq.get()).assume_init_read(),
                ManuallyDrop::take(&mut self.lease),
            )
        };
        #[allow(
            unsafe_code,
            reason = "the drained initialized owners move out exactly once"
        )]
        // SAFETY: the sole extraction above detached every actor; the local
        // ManuallyDrop wrappers prevent a second destruction of each owner.
        let owners = unsafe {
            ReturnedOwners {
                hardware: ManuallyDrop::take(&mut driver.hardware),
                interrupts: ManuallyDrop::take(irq.interrupts.get_mut()).detach(),
                route: ManuallyDrop::take(&mut irq.route),
            }
        };
        let owners = if *irq.retired.get_mut() && !*irq.signals.abandon.get_mut() {
            DetachedOwners::Retired(owners)
        } else {
            DetachedOwners::Abandoned(Quarantined::retain(owners))
        };
        // The non-Drop driver projection ends with this function, before the
        // returned lease can be used. IRQ's owned waker is destroyed here.
        drop(irq);
        ScopeReturn::Released(ReleasedScope { lease, owners })
    }
}
impl<H: Send + 'static, I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>> Drop
    for DriverScope<'_, H, I, P, E, S>
{
    fn drop(&mut self) {
        self.latch.returned = Some(self.complete());
    }
}

#[cfg(test)]
mod tests;
