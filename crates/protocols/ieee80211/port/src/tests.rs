extern crate std;

use super::*;
use oer_radio_port::{Mint, Slot};
use oer_time::RadioInstant;

/// The test port's clock domain.
enum Port {}

/// A backend in one cell: its admitted work by slot and client, its
/// queued events, and what its next enable does.
#[derive(Default)]
struct Model {
    running: core::cell::RefCell<std::vec::Vec<(u32, u8)>>,
    events: core::cell::RefCell<std::collections::VecDeque<u32>>,
    next_slot: core::cell::Cell<u32>,
    refuse_enable: core::cell::Cell<bool>,
    poisoned: core::cell::Cell<bool>,
    /// `disable` answers `Pending` once before it ends.
    slow_disable: core::cell::Cell<bool>,
    lifecycle: core::cell::RefCell<std::vec::Vec<&'static str>>,
    /// The phases the handle counted in its own state, at the uninstall.
    lent: core::cell::Cell<u32>,
    /// The install's held part as the last phase or disable left it.
    held: core::cell::Cell<(u32, u32)>,
    /// How often `run_port` polled the driver.
    driver_polls: core::cell::Cell<u32>,
    /// Work posted to the driver (an interrupt status), and how much of it
    /// the driver served.
    posted: core::cell::Cell<u32>,
    served: core::cell::Cell<u32>,
    /// The driver ignores the retire request.
    stubborn_driver: core::cell::Cell<bool>,
    /// The backend's retire bound has passed.
    bound_passed: core::cell::Cell<bool>,
}

/// The model's capabilities marker.
struct ModelCaps;

/// The model's backend; only `run_port` wraps it.
struct Backend<'m>(&'m Model);

/// The model's running phase: the body's handle, which reaches the model
/// as a phase reaches its queues.
struct Phase<'m>(&'m Model);
struct Setup(());

impl Phase<'_> {
    /// Admit one operation of `interface`; its terminal is queued when it
    /// ends.
    fn admit(&mut self, interface: u8) -> u32 {
        let slot = self.0.next_slot.get();
        self.0.next_slot.set(slot + 1);
        self.0.running.borrow_mut().push((slot, interface));
        slot
    }

    /// End one admitted operation by its slot.
    fn end(&mut self, slot: u32) {
        end(self.0, slot);
    }
}

/// End one admitted operation by its slot; its terminal is queued.
fn end(model: &Model, slot: u32) {
    let mut running = model.running.borrow_mut();
    if let Some(index) = running.iter().position(|(running, _)| *running == slot) {
        running.remove(index);
        model.events.borrow_mut().push_back(slot);
    }
}

impl<'m> PortBackend for Backend<'m> {
    type Event<'id> = u32;
    type Terminal<'id> = u32;
    type Domain = Port;
    type Fault = ();
    type Refusal = &'static str;
    type Setup<'a, 'id>
        = Setup
    where
        Self: 'a;
    type Phase<'a, 'id>
        = Phase<'m>
    where
        Self: 'a,
        'id: 'a;
    type Pools = oer_radio_port::Slots<ModelCaps, 2>;
    type Resources<'id> = [Slot<'id, ModelCaps>; 2];
    /// The phases lent and the disables run over the install's held part.
    type Held<'id> = core::cell::Cell<(u32, u32)>;
    /// The phases the handle lent, its own count.
    type Local<'id> = u32;
    /// The parts reach the state through the model, so the driver owns
    /// none.
    type State = ();
    type Uninstall = ();
    type Uninstalled = &'m Model;

    fn enable(&mut self) -> impl Future<Output = EnableResult<&'static str, ()>> + '_ {
        let model = self.0;
        core::future::ready(if model.poisoned.get() {
            Err(Poisoned { cause: () })
        } else if model.refuse_enable.get() {
            Ok(Err("the radio refused the channel"))
        } else {
            model.lifecycle.borrow_mut().push("enable");
            Ok(Ok(()))
        })
    }

    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
        _: &'a mut u32,
        sink: &'a mut dyn FnMut(u32) -> core::ops::ControlFlow<u32>,
    ) -> impl Future<Output = Result<(), Poisoned<()>>> + 'a
    where
        'id: 'a,
    {
        let model = self.0;
        async move {
            let (phases, disables) = held.get();
            held.set((phases, disables + 1));
            model.held.set(held.get());
            if model.slow_disable.get() {
                let mut waited = false;
                core::future::poll_fn(|_| {
                    if waited {
                        core::task::Poll::Ready(())
                    } else {
                        waited = true;
                        core::task::Poll::Pending
                    }
                })
                .await;
            }
            if model.poisoned.get() {
                Err(Poisoned { cause: () })
            } else {
                model.lifecycle.borrow_mut().push("disable");
                // Disable ends every admitted work; its terminal goes to the
                // sink before the port is disabled.
                let slots: std::vec::Vec<u32> = model
                    .running
                    .borrow()
                    .iter()
                    .map(|(slot, _)| *slot)
                    .collect();
                for slot in slots {
                    end(model, slot);
                }
                loop {
                    let event = model.events.borrow_mut().pop_front();
                    let Some(event) = event else { break };
                    if let core::ops::ControlFlow::Break(event) = sink(event) {
                        model.events.borrow_mut().push_front(event);
                        model.poisoned.set(true);
                        return Err(Poisoned { cause: () });
                    }
                }
                Ok(())
            }
        }
    }

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn resources<'id>(
        &mut self,
        mint: Mint<'id, Self::Pools>,
    ) -> (core::cell::Cell<(u32, u32)>, u32, [Slot<'id, ModelCaps>; 2]) {
        (core::cell::Cell::new((0, 0)), 0, mint.into_pools())
    }

    fn setup<'a, 'id>(&'a mut self, _: &'a mut u32) -> Self::Setup<'a, 'id> {
        Setup(())
    }

    fn phase<'a, 'id>(&'a mut self, held: &'a Self::Held<'id>, lent: &'a mut u32) -> Phase<'m>
    where
        'id: 'a,
    {
        *lent += 1;
        let model = self.0;
        let (phases, disables) = held.get();
        held.set((phases + 1, disables));
        model.held.set(held.get());
        Phase(model)
    }

    fn drive<'p, 'id>(
        &mut self,
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, 'm> {
        let model = self.0;
        // It serves what was posted at every poll, and retires at the first
        // poll after the request unless it is stubborn.
        core::future::poll_fn(move |_| {
            model.driver_polls.set(model.driver_polls.get() + 1);
            // A driver that sees the latch publishes nothing and stops.
            if retire.is_abandoned() {
                return core::task::Poll::Ready(());
            }
            model.served.set(model.posted.get());
            if retire.is_requested() && !model.stubborn_driver.get() {
                model.lifecycle.borrow_mut().push("retire");
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        let model = self.0;
        core::future::poll_fn(move |_| {
            if model.bound_passed.get() {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
    }

    fn detach<'id>(&mut self, _: &Self::Held<'id>) {
        self.0.lifecycle.borrow_mut().push("detach");
    }

    fn uninstall<'id>(self, _: Self::Held<'id>, lent: u32, (): (), (): ()) -> &'m Model {
        self.0.lent.set(lent);
        self.0.lifecycle.borrow_mut().push("uninstall");
        self.0
    }

    fn abandon(&mut self) {
        self.0.lifecycle.borrow_mut().push("abandon");
    }
}

impl PortPhase for Phase<'_> {
    type Event = u32;
    type Fault = ();

    fn next_event(&mut self) -> impl Future<Output = PortResult<u32, EventsLost, ()>> + '_ {
        core::future::ready(match self.0.events.borrow_mut().pop_front() {
            Some(event) => Ok(Ok(event)),
            None => Ok(Err(EventsLost)),
        })
    }
}

/// The value of a future that is ready at its first poll.
fn now_ready<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    match future.as_mut().poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    )) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("the model answers at once"),
    }
}

/// A body that runs one phase with work left in flight, then disables and
/// uninstalls, recording the leftover terminals.
struct OnePhase<'t> {
    leftovers: &'t core::cell::RefCell<std::vec::Vec<u32>>,
    cancelled: &'t core::cell::Cell<Option<u32>>,
}

impl<'m> PortBody<Backend<'m>> for OnePhase<'_> {
    async fn run<'id>(
        self,
        mut owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let _ = owner.setup();
        let mut enabled = match owner.enable().await {
            Ok(enabled) => enabled,
            Err(failed) => return failed.fail_stop(),
        };
        {
            // The phase's operations and its event stream are one object.
            let mut phase = enabled.phase();
            let joined = phase.admit(0);
            let _served = phase.admit(1);
            phase.end(joined);
            assert_eq!(phase.next_event().await, Ok(Ok(joined)));
            self.cancelled.set(Some(joined));
        }
        // The access point's work is left in flight: `disable` ends it and
        // hands its terminal to the sink.
        let leftovers = self.leftovers;
        match enabled
            .disable(&mut |event| {
                leftovers.borrow_mut().push(event);
                core::ops::ControlFlow::Continue(())
            })
            .await
        {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_port_runs_a_phase_between_its_lifecycle_commands_and_ends_through_its_uninstall() {
    let model = Model::default();
    let leftovers = core::cell::RefCell::default();
    let cancelled = core::cell::Cell::default();
    let ended = now_ready(run_port(
        Backend(&model),
        OnePhase {
            leftovers: &leftovers,
            cancelled: &cancelled,
        },
    ));
    assert_eq!(cancelled.get(), Some(0));
    assert_eq!(*leftovers.borrow(), [1]);
    assert!(core::ptr::eq(ended.unwrap(), &model));
    assert_eq!(
        *model.lifecycle.borrow(),
        ["enable", "disable", "retire", "detach", "uninstall"]
    );
}

/// A body that runs two phases of one install.
struct TwoPhases;

impl<'m> PortBody<Backend<'m>> for TwoPhases {
    async fn run<'id>(
        self,
        mut owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        for _ in 0..2 {
            let Ok(mut enabled) = owner.enable().await else {
                panic!("the model enables");
            };
            let _ = enabled.phase();
            owner = match enabled
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
            {
                Ok(disabled) => disabled,
                Err(poisoned) => return poisoned.fail_stop(),
            };
        }
        owner.uninstall(())
    }
}

#[test]
fn the_backends_held_part_of_the_install_reaches_every_phase_and_disable() {
    let model = Model::default();
    assert!(now_ready(run_port(Backend(&model), TwoPhases)).is_some());
    // One held part travelled through both transitions of both phases.
    assert_eq!(model.held.get(), (2, 2));
    // The handle's own state travelled with the owner through every
    // transition to the uninstall.
    assert_eq!(model.lent.get(), 2);
}

struct RejectFirstTerminal;

impl<'m> PortBody<Backend<'m>> for RejectFirstTerminal {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let mut owner = owner.enable().await.ok().unwrap();
        {
            let mut phase = owner.phase();
            phase.admit(0);
            phase.admit(0);
        }
        let mut offered = 0;
        let owner = owner
            .disable(&mut |terminal| {
                offered += 1;
                core::ops::ControlFlow::Break(terminal)
            })
            .await
            .err()
            .expect("a full sink poisons delivery");
        assert_eq!(offered, 1, "no later terminal enters the full sink");
        owner.uninstall(())
    }
}

#[test]
fn a_rejected_disable_terminal_and_its_successors_remain_owned_until_uninstall() {
    let model = Model::default();
    assert!(now_ready(run_port(Backend(&model), RejectFirstTerminal)).is_some());
    assert_eq!(
        model
            .events
            .borrow()
            .iter()
            .copied()
            .collect::<std::vec::Vec<_>>(),
        [0, 1]
    );
    assert!(model.poisoned.get());
}

/// A body that enables once and reports the refusal it met.
struct EnableOnce<'t> {
    refusal: &'t core::cell::Cell<Option<&'static str>>,
}

impl<'m> PortBody<Backend<'m>> for EnableOnce<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        match owner.enable().await {
            Ok(enabled) => match enabled
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
            {
                Ok(disabled) => disabled.uninstall(()),
                Err(poisoned) => poisoned.fail_stop(),
            },
            // The radio refused: the owner is disabled and uninstalls.
            Err(Failed::Refused(owner, refusal)) => {
                self.refusal.set(Some(refusal));
                owner.uninstall(())
            }
            Err(Failed::Poisoned(poisoned)) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_refused_enable_returns_the_disabled_owner_with_the_worlds_cause() {
    let model = Model::default();
    model.refuse_enable.set(true);
    let refusal = core::cell::Cell::default();
    let ended = now_ready(run_port(Backend(&model), EnableOnce { refusal: &refusal }));
    assert_eq!(refusal.get(), Some("the radio refused the channel"));
    assert!(ended.is_some());
}

#[test]
fn a_poisoned_backend_ends_through_a_fail_stop_without_its_uninstall() {
    let model = Model::default();
    model.poisoned.set(true);
    let refusal = core::cell::Cell::default();
    let ended = now_ready(run_port(Backend(&model), EnableOnce { refusal: &refusal }));
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// A body whose `enable` future is dropped before it runs: the owner inside
/// it is dropped too.
struct DropTheEnable;

impl<'m> PortBody<Backend<'m>> for DropTheEnable {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        drop(owner.enable());
        core::future::pending().await
    }
}

#[test]
fn a_dropped_enable_ends_the_port_abandoned() {
    let model = Model::default();
    let ended = now_ready(run_port(Backend(&model), DropTheEnable));
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
    assert_eq!(model.driver_polls.get(), 0);
}

/// A body that drops its `disable` future after its first poll, while the
/// backend still waits: the owner inside it is dropped mid-wait.
struct DropTheDisable;

impl<'m> PortBody<Backend<'m>> for DropTheDisable {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let Ok(enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let mut sink = |_| core::ops::ControlFlow::Continue(());
            let mut disabling = core::pin::pin!(enabled.disable(&mut sink));
            let first = disabling
                .as_mut()
                .poll(&mut core::task::Context::from_waker(
                    core::task::Waker::noop(),
                ));
            assert!(first.is_pending());
        }
        core::future::pending().await
    }
}

#[test]
fn an_owner_dropped_while_its_disable_waits_ends_the_port_abandoned() {
    let model = Model::default();
    model.slow_disable.set(true);
    let ended = now_ready(run_port(Backend(&model), DropTheDisable));
    // The disable's future held the owner across the wait: its sentry set
    // the latch.
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["enable", "abandon"]);
}

/// A body whose backend is poisoned between `enable` and `disable`: the
/// disable meets the poison, and the poisoned owner uninstalls.
struct PoisonBetween<'t>(&'t Model);

impl<'m> PortBody<Backend<'m>> for PoisonBetween<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let Ok(enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        // The world poisons the backend while the port runs.
        self.0.poisoned.set(true);
        let Err(poisoned) = enabled
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("a poisoned model fails its disable");
        };
        poisoned.cause();
        poisoned.uninstall(())
    }
}

#[test]
fn a_poisoned_disable_leaves_through_the_poisoned_owners_uninstall() {
    let model = Model::default();
    let ended = now_ready(run_port(Backend(&model), PoisonBetween(&model)));
    assert!(core::ptr::eq(ended.unwrap(), &model));
    assert_eq!(
        *model.lifecycle.borrow(),
        ["enable", "retire", "detach", "uninstall"]
    );
}

/// A body that fail-stops after the radio refused its `Enable`.
struct FailStopRefused;

impl<'m> PortBody<Backend<'m>> for FailStopRefused {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        match owner.enable().await {
            Ok(_) => panic!("the model refuses"),
            Err(failed) => failed.fail_stop(),
        }
    }
}

#[test]
fn a_refused_enable_can_fail_stop_without_an_uninstall() {
    let model = Model::default();
    model.refuse_enable.set(true);
    let ended = now_ready(run_port(Backend(&model), FailStopRefused));
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> core::task::Poll<F::Output> {
    future.poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    ))
}

/// A future of the world that the test makes ready: it stands for a
/// foreign future (another task's reply, a permit) the body awaits.
fn foreign(ready: &core::cell::Cell<bool>) -> impl Future<Output = ()> + '_ {
    core::future::poll_fn(|_| {
        if ready.get() {
            core::task::Poll::Ready(())
        } else {
            core::task::Poll::Pending
        }
    })
}

/// A body that awaits a foreign future while disabled, then uninstalls.
struct Stalled<'t>(&'t core::cell::Cell<bool>);

impl<'m> PortBody<Backend<'m>> for Stalled<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        foreign(self.0).await;
        owner.uninstall(())
    }
}

#[test]
fn a_body_awaiting_a_foreign_future_does_not_stall_the_driver() {
    let model = Model::default();
    let released = core::cell::Cell::new(false);
    let mut port = core::pin::pin!(run_port(Backend(&model), Stalled(&released)));
    assert!(poll_once(port.as_mut()).is_pending());
    // The interrupt posts work while the body waits on the world: the
    // driver serves it at the task's next wake.
    model.posted.set(1);
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(model.served.get(), 1);
    model.posted.set(2);
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(model.served.get(), 2);
    released.set(true);
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("the released body uninstalls");
    };
    assert!(core::ptr::eq(ended.unwrap(), &model));
}

/// A body that uninstalls the disabled port at once.
struct UninstallAtOnce;

impl<'m> PortBody<Backend<'m>> for UninstallAtOnce {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        owner.uninstall(())
    }
}

#[test]
fn an_uninstall_retires_the_driver_then_detaches_then_takes_the_backend_apart() {
    let model = Model::default();
    let ended = now_ready(run_port(Backend(&model), UninstallAtOnce));
    assert!(core::ptr::eq(ended.unwrap(), &model));
    assert_eq!(*model.lifecycle.borrow(), ["retire", "detach", "uninstall"]);
}

#[test]
fn a_driver_that_misses_its_retire_bound_fail_stops_the_port() {
    let model = Model::default();
    model.stubborn_driver.set(true);
    let mut port = core::pin::pin!(run_port(Backend(&model), UninstallAtOnce));
    // Within the bound, `run_port` waits for the driver.
    assert!(poll_once(port.as_mut()).is_pending());
    assert!(model.lifecycle.borrow().is_empty());
    model.bound_passed.set(true);
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("a passed bound ends the port");
    };
    // No detach and no uninstall: the hardware stays installed.
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// A body that fail-stops after a wait, then yields once more before it
/// returns its `Ended`.
struct FailStopThenYield<'t>(&'t core::cell::Cell<bool>);

impl<'m> PortBody<Backend<'m>> for FailStopThenYield<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        foreign(self.0).await;
        let ended = owner.fail_stop();
        let mut yielded = false;
        core::future::poll_fn(|_| {
            if yielded {
                core::task::Poll::Ready(())
            } else {
                yielded = true;
                core::task::Poll::Pending
            }
        })
        .await;
        ended
    }
}

#[test]
fn a_fail_stop_halts_the_driver_at_its_next_poll() {
    let model = Model::default();
    let fail = core::cell::Cell::new(false);
    let mut port = core::pin::pin!(run_port(Backend(&model), FailStopThenYield(&fail)));
    assert!(poll_once(port.as_mut()).is_pending());
    let before = model.driver_polls.get();
    assert_eq!(before, 1);
    fail.set(true);
    // The body abandons the backend and yields: the driver is not polled.
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
    assert_eq!(model.driver_polls.get(), before);
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("the body returns its fail-stop");
    };
    assert!(ended.is_none());
    assert_eq!(model.driver_polls.get(), before);
}

/// A body that drops its owner and waits on.
struct DropTheOwnerAndWait;

impl<'m> PortBody<Backend<'m>> for DropTheOwnerAndWait {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        drop(owner);
        foreign(&core::cell::Cell::new(false)).await;
        unreachable!("run_port drops the body at the end of this poll")
    }
}

#[test]
fn a_dropped_owner_ends_the_port_at_the_end_of_that_poll() {
    let model = Model::default();
    let mut port = core::pin::pin!(run_port(Backend(&model), DropTheOwnerAndWait));
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("the port ends in the poll that dropped the owner");
    };
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
    assert_eq!(model.driver_polls.get(), 0);
}

/// A body that enables, posts work to the driver and drops its owner, all
/// in one poll.
struct PostThenDropTheOwner<'t>(&'t Model);

impl<'m> PortBody<Backend<'m>> for PostThenDropTheOwner<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        enabled.phase().admit(1);
        self.0.posted.set(self.0.posted.get() + 1);
        drop(enabled);
        core::future::pending().await
    }
}

#[test]
fn nothing_posted_in_the_poll_that_dropped_the_owner_is_published() {
    let model = Model::default();
    let ended = now_ready(run_port(Backend(&model), PostThenDropTheOwner(&model)));
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["enable", "abandon"]);
    // The driver never served the posted work.
    assert_eq!(model.posted.get(), 1);
    assert_eq!(model.served.get(), 0);
    assert_eq!(model.driver_polls.get(), 0);
}

#[test]
fn the_abandon_latch_is_atomic() {
    const fn shared<T: Send + Sync>() {}
    shared::<AbandonLatch>();
    let latch = AbandonLatch::new();
    assert!(!latch.is_set());
    std::thread::scope(|scope| {
        scope.spawn(|| latch.set());
    });
    assert!(latch.is_set());
}

#[test]
fn a_port_dropped_while_its_driver_retires_abandons_its_backend() {
    let model = Model::default();
    model.stubborn_driver.set(true);
    let dropped = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
        let mut port = core::pin::pin!(run_port(Backend(&model), UninstallAtOnce));
        // The body asked for the uninstall; `run_port` waits for the driver.
        assert!(poll_once(port.as_mut()).is_pending());
        assert!(model.lifecycle.borrow().is_empty());
    }));
    // Cancellation reaches the once-only fail-stop without unwinding.
    assert!(dropped.is_ok());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}
