extern crate std;

mod forget;
mod lifecycle;
mod work;

use super::*;
use core::{cell::RefCell, convert::Infallible, future::Future, task::Context};
use std::{
    boxed::Box,
    sync::{Arc, Barrier},
    task::Wake,
    thread,
};

#[derive(Clone, Default)]
struct Count(Arc<AtomicUsize>);
impl Count {
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
    fn increment(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Routing {
    masked: AtomicBool,
    pends: Count,
    acks: Count,
    last_ready: AtomicBool,
}
type Platform = Arc<Routing>;
type Schedule = fn(&Platform, WakePlan);
type Storage<E = Infallible> = ScopeStorage<Hardware, Interrupts, Platform, E, Schedule>;
type Lease<E = Infallible> = ScopeLease<Hardware, Interrupts, Platform, E, Schedule>;
type Latch<E = Infallible> = AbandonLatch<Hardware, Interrupts, Platform, E, Schedule>;
type Return<E = Infallible> = ScopeReturn<Hardware, Interrupts, Platform, E, Schedule>;
fn mask(p: &Platform) {
    p.masked.store(true, Ordering::SeqCst);
}
fn acknowledge(p: &Platform) {
    p.acks.increment();
}
fn enable(p: &Platform) -> Result<(), Infallible> {
    p.masked.store(false, Ordering::SeqCst);
    Ok(())
}
fn schedule(p: &Platform, plan: WakePlan) {
    p.pends.increment();
    p.last_ready
        .store(matches!(plan, WakePlan::Ready), Ordering::Release);
    if matches!(plan, WakePlan::Wait { .. }) {
        let hook = WAIT_ARM.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}
fn make_route() -> (
    ScopeRoute<Platform, Infallible>,
    &'static ScopeSlot,
    Platform,
) {
    let slot = Box::leak(Box::new(ScopeSlot::new()));
    let platform = Arc::new(Routing::default());
    (
        ScopeRoute::new(
            platform.clone(),
            slot,
            mask,
            acknowledge,
            enable,
            schedule as Schedule,
        ),
        slot,
        platform,
    )
}
fn lease<E: 'static>() -> Lease<E> {
    Storage::pin_static(Box::leak(Box::new(Storage::new())))
}

// A deterministic scheduler point inside the bounded wait. The hook can let
// another core finish before the next observation; no wall-clock sleeps.
std::thread_local! {
    static WAIT_BOUNDARY: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static WAIT_ARM: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}
pub(super) fn wait_boundary() {
    let hook = WAIT_BOUNDARY.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

struct Block {
    entered: Barrier,
    resume: Barrier,
}
impl Block {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Barrier::new(2),
            resume: Barrier::new(2),
        })
    }
    fn actor(&self) {
        self.entered.wait();
        self.resume.wait();
    }
}
struct Hardware {
    work: work::DmaQueue,
    steps: Count,
    drops: Count,
    stops: Count,
    facts: Count,
    capacity: Count,
    accesses_after_pause: Count,
    block: Option<Arc<Block>>,
    irq_during_step: Option<&'static ScopeSlot>,
    platform: Platform,
}
impl Drop for Hardware {
    fn drop(&mut self) {
        self.drops.increment();
    }
}
fn hardware(platform: &Platform) -> Hardware {
    Hardware {
        work: work::DmaQueue::default(),
        steps: Count::default(),
        drops: Count::default(),
        stops: Count::default(),
        facts: Count::default(),
        capacity: Count::default(),
        accesses_after_pause: Count::default(),
        block: None,
        irq_during_step: None,
        platform: platform.clone(),
    }
}
fn step(
    h: &mut Hardware,
    _: &mut IrqControl<Interrupts>,
    events: DriverEvents,
    signals: &DriverSignals,
) -> DriverStep {
    assert_eq!(h.stops.get(), 0);
    h.work.assert_owned();
    if events.mac.rx_capacity_returned() {
        h.capacity.increment();
    }
    h.facts.0.fetch_add(events.mac.count(), Ordering::SeqCst);
    let _handled = events.power;
    h.steps.increment();
    if let Some(block) = &h.block {
        block.actor();
        h.accesses_after_pause.increment();
    }
    if let Some(slot) = h.irq_during_step {
        assert!(slot.mac_interrupt().had_status);
    }
    if signals.retirement_requested() {
        h.work.drain();
        DriverStep::Retired
    } else {
        DriverStep::Wake(WakePlan::until(
            Instant::from_micros(0),
            Instant::from_micros(100),
            WakeSources::CommandsAndInterrupts,
        ))
    }
}
fn stop(h: &mut Hardware, _: &mut IrqControl<Interrupts>) {
    assert!(h.platform.masked.load(Ordering::SeqCst));
    assert_eq!(
        h.stops.get(),
        0,
        "stop runs exactly once after all actors exit"
    );
    h.stops.increment();
    h.work.drain();
}
#[derive(Default)]
struct Interrupts {
    mac_reads: Count,
    power_reads: Count,
    detached: Count,
    drops: Count,
    accesses_after_pause: Count,
    block: Option<Arc<Block>>,
    driver_during_irq: Option<&'static ScopeSlot>,
}
impl Drop for Interrupts {
    fn drop(&mut self) {
        self.drops.increment();
    }
}
impl sealed::InterruptOwner for Interrupts {}
impl InterruptOwner for Interrupts {
    type Detached = ();
    fn mac(&mut self) -> IrqSnapshot {
        self.mac_reads.increment();
        if let Some(slot) = self.driver_during_irq {
            dispatch(slot);
        }
        if let Some(block) = &self.block {
            block.actor();
            self.accesses_after_pause.increment();
        }
        IrqSnapshot {
            had_status: true,
            posted_events: oer_esp32s31_ieee80211_mac::irq::EVENT_RX_SUCCESS,
            ..IrqSnapshot::default()
        }
    }
    fn power(&mut self) -> PowerIrqSnapshot {
        self.power_reads.increment();
        if let Some(block) = &self.block {
            block.actor();
            self.accesses_after_pause.increment();
        }
        PowerIrqSnapshot {
            observation: MacPowerInterruptObservation::from_semantic_events(
                false, false, false, false, true, false,
            ),
        }
    }
    fn detach(self) {
        self.detached.increment();
    }
}

#[allow(
    unsafe_code,
    reason = "the harness binds its unique slot, permanent lease and exclusive executor"
)]
fn new_scope<'a, E: 'static>(
    lease: Lease<E>,
    h: Hardware,
    irq: Interrupts,
    route: ScopeRoute<Platform, E>,
    latch: &'a mut Latch<E>,
) -> DriverScope<'a, Hardware, Interrupts, Platform, E> {
    // SAFETY: each test owns one route and one permanent backing; the only
    // dispatch is serialized. IRQ-bank access uses the host cross-core CS.
    // Callbacks do not poll, destroy or reenter the scope. Barriers model a
    // delayed actor; timeout must retain its backing without calling stop.
    unsafe {
        DriverScope::new(
            lease,
            h,
            irq,
            DriverCallbacks::new(step, stop),
            route,
            DrainBound::checks::<4>(),
            latch,
        )
    }
}
fn dispatch(slot: &ScopeSlot) {
    #[allow(unsafe_code, reason = "tests never overlap two driver dispatches")]
    // SAFETY: tests permit one driver actor, with no external hardware borrow.
    unsafe {
        slot.dispatch()
    };
}
fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}
fn released(
    result: Option<Return>,
) -> ReleasedScope<Hardware, Interrupts, Platform, Infallible, Schedule> {
    match result {
        Some(ScopeReturn::Released(released)) => released,
        _ => panic!("expected Released"),
    }
}

#[test]
fn completion_keeps_result_until_separate_retirement_and_stops_once() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let (steps, stops, drops) = (h.steps.clone(), h.stops.clone(), h.drops.clone());
    let mut latch = Latch::new();
    let mut context = 5;
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run(&mut context, async |context, _| {
        *context += 1;
        *context
    }));
    assert!(poll(future.as_mut()).is_pending());
    assert_eq!(steps.get(), 0);
    assert_eq!(
        platform.pends.get(),
        2,
        "start and retire both wake the separate driver"
    );
    dispatch(slot);
    assert_eq!(steps.get(), 1);
    assert_eq!(
        stops.get(),
        0,
        "retirement does not bypass the quiescence boundary"
    );
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(6)));
    drop(future);
    assert_eq!(context, 6);
    assert_eq!(stops.get(), 1);
    assert!(platform.masked.load(Ordering::SeqCst));
    assert!(slot.entry.load(Ordering::Acquire).is_null());
    let returned = released(latch.into_return());
    assert!(matches!(returned.owners, DetachedOwners::Retired(_)));
    assert_eq!(drops.get(), 0);
    drop(returned);
    assert_eq!(drops.get(), 1);
}

#[test]
fn cancellation_before_and_during_poll_returns_lease_and_quarantines_owners() {
    for started in [false, true] {
        let (route, slot, platform) = make_route();
        let h = hardware(&platform);
        let (stops, drops) = (h.stops.clone(), h.drops.clone());
        let mut latch = Latch::new();
        let mut context = 0;
        let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
        let mut future = Box::pin(scope.run(&mut context, async |context, _| {
            *context += 1;
            core::future::pending::<()>().await;
        }));
        if started {
            assert!(poll(future.as_mut()).is_pending());
        }
        drop(future);
        assert_eq!(context, usize::from(started));
        assert_eq!(stops.get(), 1);
        assert!(platform.masked.load(Ordering::SeqCst));
        assert!(!slot.mac_interrupt().had_status);
        assert!(slot.power_interrupt().observation.is_empty());
        let returned = released(latch.into_return());
        assert!(matches!(returned.owners, DetachedOwners::Abandoned(_)));
        drop(returned);
        assert_eq!(drops.get(), 0, "cancelled owners are not silently recycled");
    }
}

#[test]
fn fail_stop_is_synchronous_and_drop_does_not_stop_twice() {
    for body_waits in [false, true] {
        let (route, slot, platform) = make_route();
        let h = hardware(&platform);
        let stops = h.stops.clone();
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), access| {
            access.abandon();
            assert_eq!(stops.get(), 1);
            assert!(platform.masked.load(Ordering::SeqCst));
            assert!(!slot.open.load(Ordering::SeqCst));
            if body_waits {
                core::future::pending::<()>().await;
            }
            17
        }));
        assert_eq!(poll(future.as_mut()).is_pending(), body_waits);
        dispatch(slot);
        drop(future);
        assert_eq!(stops.get(), 1);
        assert!(matches!(
            released(latch.into_return()).owners,
            DetachedOwners::Abandoned(_)
        ));
    }
}

#[test]
fn irq_and_driver_backing_survive_owning_pin_forget_without_static_context() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let (steps, stops, drops) = (h.steps.clone(), h.stops.clone(), h.drops.clone());
    let irq = Interrupts::default();
    let detached = irq.detached.clone();
    let mut latch = Latch::new();
    {
        let mut context = 5;
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run(&mut context, async |context, _| {
            *context += 1;
            core::future::pending::<()>().await;
        }));
        assert!(poll(future.as_mut()).is_pending());
        core::mem::forget(future);
    }
    assert!(slot.mac_interrupt().had_status);
    assert!(slot.power_interrupt().observation.sta_tbtt());
    dispatch(slot);
    assert_eq!(steps.get(), 1);
    assert_eq!(stops.get(), 0);
    assert_eq!(drops.get(), 0);
    assert_eq!(detached.get(), 0);
    assert!(
        latch.into_return().is_none(),
        "forget fabricates neither release nor poison"
    );
}

#[test]
#[allow(
    clippy::forget_non_drop,
    reason = "regression distinguishes borrowed Pin from ownership of its future"
)]
fn forgetting_borrowed_pin_still_cancels_when_the_future_drops() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let mut latch = Latch::new();
    {
        let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
        let mut future =
            core::pin::pin!(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        core::mem::forget(future);
        assert!(slot.mac_interrupt().had_status);
    }
    assert_eq!(stops.get(), 1);
    assert!(!slot.mac_interrupt().had_status);
    assert!(matches!(
        latch.into_return(),
        Some(ScopeReturn::Released(_))
    ));
}

#[test]
fn cancellation_after_retirement_keeps_owners_and_drops_the_result() {
    struct Output(Count);
    impl Drop for Output {
        fn drop(&mut self) {
            self.0.increment();
        }
    }
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let (drops, stops) = (h.drops.clone(), h.stops.clone());
    let result_drops = Count::default();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run(Output(result_drops.clone()), async |output, _| output));
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot);
    assert_eq!(result_drops.get(), 0);
    drop(future);
    assert_eq!(result_drops.get(), 1);
    assert_eq!(stops.get(), 1);
    drop(released(latch.into_return()));
    assert_eq!(drops.get(), 1);
}

#[test]
fn wrong_core_does_not_start_body_and_detaches_both_entries() {
    #[derive(Debug, Eq, PartialEq)]
    struct WrongCore;
    fn wrong_core(_: &Platform) -> Result<(), WrongCore> {
        Err(WrongCore)
    }
    let (route, slot, platform) = make_route();
    let route = ScopeRoute::new(
        route.into_platform(),
        slot,
        mask,
        acknowledge,
        wrong_core,
        schedule as Schedule,
    );
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let irq = Interrupts::default();
    let detached = irq.detached.clone();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, irq, route, &mut latch);
    let mut future =
        Box::pin(scope.run((), async |(), _| panic!("body cannot start on wrong core")));
    assert_eq!(poll(future.as_mut()), Poll::Ready(Err(WrongCore)));
    drop(future);
    assert_eq!(stops.get(), 1);
    assert_eq!(detached.get(), 1);
    assert!(platform.masked.load(Ordering::SeqCst));
    assert!(!slot.mac_interrupt().had_status);
    assert!(slot.power_interrupt().observation.is_empty());
    assert!(matches!(
        latch.into_return(),
        Some(ScopeReturn::Released(_))
    ));
}

#[derive(Clone, Copy, Debug)]
enum Actor {
    Driver,
    Mac,
    Power,
}
fn actor(slot: &'static ScopeSlot, which: Actor) {
    match which {
        Actor::Driver => dispatch(slot),
        Actor::Mac => {
            slot.mac_interrupt();
        }
        Actor::Power => {
            slot.power_interrupt();
        }
    }
}

// Regression for target/agent-a/reclamation-counterexample.rs: an inline
// pinned future freed the registration after timeout while an actor held it.
// Here dropping that same owning Pin cannot release the permanent allocation.
#[test]
fn timeout_quarantines_live_driver_and_irq_backing_without_calling_stop() {
    for which in [Actor::Driver, Actor::Mac, Actor::Power] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let (stops, drops) = (h.stops.clone(), h.drops.clone());
        let mut irq = Interrupts::default();
        let (irq_drops, detached) = (irq.drops.clone(), irq.detached.clone());
        let resumed = match which {
            Actor::Driver => {
                h.block = Some(block.clone());
                h.accesses_after_pause.clone()
            }
            _ => {
                irq.block = Some(block.clone());
                irq.accesses_after_pause.clone()
            }
        };
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let worker = thread::spawn(move || actor(slot, which));
        block.entered.wait();
        assert_eq!(slot.entered.load(Ordering::SeqCst), 1);
        drop(future);
        assert!(
            matches!(latch.into_return(), Some(ScopeReturn::Poisoned(_))),
            "{which:?}"
        );
        assert!(platform.masked.load(Ordering::SeqCst));
        assert!(!slot.open.load(Ordering::SeqCst));
        assert_eq!(stops.get(), 0, "timeout cannot project a borrowed Driver");
        assert_eq!(drops.get(), 0);
        assert_eq!(irq_drops.get(), 0);
        assert_eq!(detached.get(), 0);
        assert!(
            !slot.mac_interrupt().had_status,
            "closed admission rejects without taking the bank lock"
        );
        block.resume.wait();
        worker.join().unwrap();
        assert_eq!(
            resumed.get(),
            1,
            "the admitted actor still owns live permanent bytes"
        );
        assert_eq!(slot.entered.load(Ordering::SeqCst), 0);
        assert_eq!(stops.get(), 0, "eventual exit cannot undo permanent poison");
        assert_eq!(drops.get(), 0);
        assert_eq!(irq_drops.get(), 0);
    }
}

#[test]
fn unconfirmed_can_free_driver_container_without_drop_or_reclaiming_backing() {
    for which in [Actor::Driver, Actor::Mac, Actor::Power] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let (stops, drops) = (h.stops.clone(), h.drops.clone());
        let mut irq = Interrupts::default();
        let (irq_drops, detached) = (irq.drops.clone(), irq.detached.clone());
        let resumed = match which {
            Actor::Driver => {
                h.block = Some(block.clone());
                h.accesses_after_pause.clone()
            }
            _ => {
                irq.block = Some(block.clone());
                irq.accesses_after_pause.clone()
            }
        };
        let context = std::string::String::from("caller-owned context");
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        // The same borrowed wait as Driver::drive; the core explicitly abandons.
        let mut future = Box::pin(scope.run_body(&context, async |context, _| {
            assert_eq!(context, "caller-owned context");
            core::future::pending::<()>().await;
        }));
        assert!(poll(future.as_mut()).is_pending());
        let registered = slot.entry.load(Ordering::Acquire);
        let worker = thread::spawn(move || actor(slot, which));
        block.entered.wait();
        scope.abandon();
        assert!(matches!(
            scope.lease.storage().shutdown.get(),
            Some(Quiescence::TimedOut)
        ));
        assert!(platform.masked.load(Ordering::Acquire));
        assert!(!slot.open.load(Ordering::Acquire));
        drop(future);
        drop(context);
        // Model core quarantine, then free the outer allocation without Drop.
        let quarantine = Box::new(Quarantined::retain(driver::Driver::new(scope)));
        drop(quarantine);
        assert!(latch.into_return().is_none(), "Driver::drop must not run");
        assert_eq!(slot.entry.load(Ordering::Acquire), registered);
        assert_eq!(
            (stops.get(), drops.get(), irq_drops.get(), detached.get()),
            (0, 0, 0, 0)
        );
        assert!(!slot.mac_interrupt().had_status);
        block.resume.wait();
        worker.join().unwrap();
        assert_eq!(resumed.get(), 1, "the actor resumes in permanent backing");
        assert_eq!(slot.entered.load(Ordering::Acquire), 0);
        assert_eq!(
            (stops.get(), drops.get(), irq_drops.get(), detached.get()),
            (0, 0, 0, 0)
        );
    }
}

#[test]
fn bounded_wait_that_drains_another_core_stops_once_and_returns_the_lease() {
    for which in [Actor::Driver, Actor::Mac, Actor::Power] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let stops = h.stops.clone();
        let mut irq = Interrupts::default();
        let detached = irq.detached.clone();
        match which {
            Actor::Driver => h.block = Some(block.clone()),
            _ => irq.block = Some(block.clone()),
        }
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let worker = thread::spawn(move || actor(slot, which));
        block.entered.wait();
        WAIT_BOUNDARY.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                block.resume.wait();
                worker.join().unwrap();
            }))
        });
        drop(future);
        assert_eq!(stops.get(), 1);
        assert_eq!(detached.get(), 1);
        assert!(slot.entry.load(Ordering::Acquire).is_null());
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }
}

#[test]
fn irq_can_preempt_or_overlap_a_mutable_driver_step_on_either_core() {
    // Both same-core orders: hard IRQ preempts a live Driver borrow, or a
    // higher driver-SWI preempts an IRQ-bank borrow. Neither projects the other.
    for mac_first in [false, true] {
        let (route, slot, platform) = make_route();
        let mut h = hardware(&platform);
        let mut irq = Interrupts::default();
        if mac_first {
            irq.driver_during_irq = Some(slot);
        } else {
            h.irq_during_step = Some(slot);
        }
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        if mac_first {
            assert!(slot.mac_interrupt().had_status);
        } else {
            dispatch(slot);
        }
        drop(future);
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }

    // Either priority on another core: the other actor remains entered.
    for driver_first in [false, true] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let mut irq = Interrupts::default();
        if driver_first {
            h.block = Some(block.clone());
        } else {
            irq.block = Some(block.clone());
        }
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let worker = thread::spawn(move || {
            actor(
                slot,
                if driver_first {
                    Actor::Driver
                } else {
                    Actor::Mac
                },
            )
        });
        block.entered.wait();
        if driver_first {
            assert!(slot.mac_interrupt().had_status);
        } else {
            dispatch(slot);
        }
        block.resume.wait();
        worker.join().unwrap();
        drop(future);
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }
}

#[test]
fn a_released_lease_can_be_installed_again_with_clean_facts() {
    let (route, slot, platform) = make_route();
    let mut latch = Latch::new();
    let scope = new_scope(
        lease(),
        hardware(&platform),
        Interrupts::default(),
        route,
        &mut latch,
    );
    let mut future = Box::pin(scope.run((), async |(), _| ()));
    assert!(poll(future.as_mut()).is_pending());
    assert!(slot.mac_interrupt().had_status);
    dispatch(slot);
    assert!(slot.mac_interrupt().had_status);
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
    drop(future);
    let first = released(latch.into_return());
    let DetachedOwners::Retired(owners) = first.owners else {
        panic!("normal return");
    };
    let h = hardware(&platform);
    let facts = h.facts.clone();
    drop(owners.hardware);
    let mut latch = Latch::new();
    let scope = new_scope(
        first.lease,
        h,
        Interrupts::default(),
        owners.route,
        &mut latch,
    );
    let mut future = Box::pin(scope.run((), async |(), _| ()));
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot);
    assert_eq!(
        facts.get(),
        0,
        "previous installation facts cannot cross lease reuse"
    );
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
    drop(future);
    assert!(matches!(
        latch.into_return(),
        Some(ScopeReturn::Released(_))
    ));
}

struct Wakes {
    count: Count,
    slot: &'static ScopeSlot,
}
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        assert_eq!(
            self.slot.entered.load(Ordering::SeqCst),
            0,
            "all installation borrows end before wake"
        );
        self.count.increment(); // Composition wakers only schedule, never detach.
    }
}
#[test]
fn both_irq_wake_tails_own_values_and_run_after_admission_guard_exits() {
    for mac in [false, true] {
        let (route, slot, platform) = make_route();
        let mut latch = Latch::new();
        let scope = new_scope(
            lease(),
            hardware(&platform),
            Interrupts::default(),
            route,
            &mut latch,
        );
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let count = Count::default();
        let waker = Waker::from(Arc::new(Wakes {
            count: count.clone(),
            slot,
        }));
        #[allow(
            unsafe_code,
            reason = "the test exclusively registers a waker before starting any actor"
        )]
        // SAFETY: this test binds exactly these types and has no concurrent
        // driver/IRQ actors; the exclusive test projection ends before entry.
        let driver = unsafe {
            &mut *(*slot.entry.load(Ordering::Acquire))
                .driver_context
                .cast::<DriverRegion<Hardware, Interrupts, Platform, Infallible, Schedule>>()
        };
        if mac {
            assert!(
                driver
                    .mac
                    .poll_batch(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        } else {
            assert!(
                driver
                    .power
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        if mac {
            assert!(slot.mac_interrupt().had_status);
        } else {
            assert!(slot.power_interrupt().observation.sta_tbtt());
        }
        assert_eq!(count.get(), 1);
        drop(future);
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }
}

#[test]
fn due_deadlines_are_ready_and_future_deadlines_name_sources() {
    let now = Instant::from_micros(100);
    assert_eq!(
        WakePlan::until(now, now, WakeSources::Commands),
        WakePlan::Ready
    );
    assert_eq!(
        WakePlan::until(now, Instant::from_micros(99), WakeSources::Interrupts),
        WakePlan::Ready
    );
    let WakePlan::Wait { sources, deadline } = WakePlan::until(
        now,
        Instant::from_micros(101),
        WakeSources::CommandsAndInterrupts,
    ) else {
        panic!("future deadline");
    };
    assert_eq!(sources, WakeSources::CommandsAndInterrupts);
    assert_eq!(deadline.at(), Instant::from_micros(101));
}

#[test]
fn software_capacity_publication_wakes_the_registered_driver() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let capacity = h.capacity.clone();
    let publish = Arc::new(AtomicBool::new(false));
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run(publish.clone(), async |publish, access| {
        poll_fn(|_| {
            if publish.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        access.irq().notify_rx_capacity();
        core::future::pending::<()>().await;
    }));
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot); // Registers the owned static-slot waker before checking facts.
    let before = platform.pends.get();
    publish.store(true, Ordering::Release);
    assert!(poll(future.as_mut()).is_pending());
    assert!(platform.pends.get() > before);
    assert!(platform.last_ready.load(Ordering::Acquire));
    dispatch(slot);
    assert_eq!(capacity.get(), 1);
    drop(future);
}

#[test]
fn irq_while_arming_wait_leaves_ready_work_for_a_finite_next_step() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let facts = h.facts.clone();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
    assert!(poll(future.as_mut()).is_pending());
    WAIT_ARM.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(slot.mac_interrupt().had_status);
        }))
    });
    dispatch(slot);
    assert_eq!(facts.get(), 0, "new facts cannot extend the captured batch");
    assert!(platform.last_ready.load(Ordering::Acquire));
    dispatch(slot);
    assert_eq!(facts.get(), 1);
    drop(future);
}

#[test]
fn late_owned_driver_waker_does_not_touch_detached_installation_memory() {
    let (route, slot, platform) = make_route();
    let mut latch = Latch::new();
    let scope = new_scope(
        lease(),
        hardware(&platform),
        Interrupts::default(),
        route,
        &mut latch,
    );
    let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
    assert!(poll(future.as_mut()).is_pending());
    let waker = slot.driver_waker();
    drop(future);
    let before = platform.pends.get();
    let cloned = waker.clone();
    drop(waker);
    cloned.wake_by_ref();
    cloned.wake();
    assert_eq!(platform.pends.get(), before);
    assert_eq!(slot.entered.load(Ordering::Acquire), 0);
}
