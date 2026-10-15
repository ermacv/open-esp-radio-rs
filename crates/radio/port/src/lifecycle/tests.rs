use super::*;
use core::{
    cell::{Cell, RefCell},
    future::pending,
};
use std::{
    boxed::Box,
    sync::{Arc, atomic::AtomicUsize},
    task::Wake,
    vec::Vec,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    Hardware,
}
#[derive(Clone, Copy)]
enum Mode {
    Normal,
    FaultBeforeEnable,
    FaultAfterEnable,
    MissBound,
    StallRetire,
    StallEnable,
}
struct Model {
    mode: Mode,
    enabled: Cell<bool>,
    poison: RefCell<PoisonState<Fault>>,
    log: RefCell<Vec<&'static str>>,
    event: Cell<bool>,
    irq_masked: Cell<bool>,
    irq_state: Cell<usize>,
}
impl Model {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            enabled: Cell::new(false),
            poison: RefCell::new(PoisonState::new()),
            log: RefCell::new(Vec::new()),
            event: Cell::new(false),
            irq_masked: Cell::new(false),
            irq_state: Cell::new(0),
        }
    }
    fn installed(&self) -> Installed<Backend<'_>> {
        Installed::new(Backend(self), Driver(self))
    }
    fn interrupt(&self) {
        if !self.irq_masked.get() {
            self.irq_state.set(self.irq_state.get() + 1);
        }
    }
    fn record(&self, event: &'static str) {
        self.log.borrow_mut().push(event);
    }
}
struct Backend<'a>(&'a Model);
struct Driver<'a>(&'a Model);
struct Phase<'a>(&'a Model);
impl PortPhase for Phase<'_> {
    type Event = ();
    type Fault = Fault;
    async fn next_event(&mut self) -> PortResult<(), EventsLost, Fault> {
        poll_fn(|_| {
            if self.0.event.replace(false) {
                self.0.record("event");
                Poll::Ready(Ok(Ok(())))
            } else if let Some(poisoned) = self.0.poison.borrow().get() {
                Poll::Ready(Err(poisoned))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}
impl<'a> PortBackend for Backend<'a> {
    type Pools = ();
    type Event<'id> = ();
    type Terminal<'id> = ();
    type Domain = ();
    type Fault = Fault;
    type Refusal = ();
    type Setup<'p, 'id>
        = ()
    where
        Self: 'p;
    type Phase<'p, 'id>
        = Phase<'p>
    where
        Self: 'p,
        'id: 'p;
    type Resources<'id> = ();
    type Held<'id> = ();
    type Local<'id> = ();
    type Driver = Driver<'a>;
    type State = usize;
    type Uninstall = ();
    type Uninstalled = usize;
    async fn enable(&mut self, _: Call) -> EnableResult<(), Fault> {
        if matches!(self.0.mode, Mode::StallEnable) {
            pending::<()>().await;
        }
        if matches!(self.0.mode, Mode::FaultBeforeEnable) {
            poll_fn(|_| {
                if self.0.poison.borrow().get().is_some() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        }
        if let Some(poisoned) = self.0.poison.borrow().get() {
            return Err(poisoned);
        }
        self.0.record("enable");
        self.0.enabled.set(true);
        Ok(Ok(()))
    }
    async fn disable<'p, 'id>(
        &'p mut self,
        _: Call,
        _: &'p (),
        _: &'p mut (),
        _: &'p mut dyn FnMut(()),
    ) -> Result<(), Poisoned<Fault>>
    where
        'id: 'p,
    {
        self.0.record("disable");
        self.0.enabled.set(false);
        self.0.poison.borrow().get().map_or(Ok(()), Err)
    }
    async fn now(&self, _: Call) -> PortResult<RadioInstant<()>, ClockError, Fault> {
        Ok(Ok(RadioInstant::from_micros(0)))
    }
    fn resources<'id>(&mut self, mint: Mint<'id, ()>) -> InstallParts<'id, Self> {
        mint.into_pools();
        InstallParts {
            held: (),
            local: (),
            resources: (),
        }
    }
    fn setup<'p, 'id>(&'p mut self, _: Call, _: &'p mut Self::Local<'id>) -> Self::Setup<'p, 'id> {}
    fn phase<'p, 'id>(&'p mut self, _: Call, _: &'p (), _: &'p mut ()) -> Phase<'p>
    where
        'id: 'p,
    {
        Phase(self.0)
    }
    async fn drive<'p, 'id>(
        driver: Driver<'a>,
        _: &'p (),
        retire: Retire<'p>,
    ) -> DriverExit<usize, Fault> {
        let model = driver.0;
        model.record("drive");
        if matches!(model.mode, Mode::FaultBeforeEnable | Mode::FaultAfterEnable) {
            let (cause, poisoned) = model.poison.borrow_mut().publish(Fault::Hardware);
            model.event.set(true);
            model.record("poison");
            return DriverExit::Faulted {
                state: 7,
                cause: cause.cause,
                poisoned,
            };
        }
        if matches!(model.mode, Mode::MissBound | Mode::StallRetire) {
            pending::<()>().await;
        }
        let proof = retire.requested().await;
        model.record("retire");
        DriverExit::Retired { state: 7, proof }
    }

    fn retire_bound(&self, _: Call) -> impl Future<Output = ()> + use<'a> {
        let mode = self.0.mode;
        async move {
            if !matches!(mode, Mode::MissBound) {
                pending::<()>().await;
            }
        }
    }
    fn detach<'id>(&mut self, _: crate::Detach, _: &()) {
        self.0.record("detach");
    }
    fn uninstall<'id>(
        self,
        _: (),
        _: (),
        retired: Retired<'id, usize>,
        _: Detached<'id>,
        _: (),
    ) -> usize {
        self.0.record("uninstall");
        retired.into_state()
    }
    fn abandon(&mut self, call: crate::Quarantine) -> crate::Abandon {
        self.0.irq_masked.set(true);
        self.0.record("abandon");
        crate::Abandon::Stopped(call.stopped())
    }
}

enum Body {
    Uninstall,
    Phase,
    Drop,
    Forget,
    Foreign,
    CancelEnable,
}
impl<'a> PortBody<Backend<'a>> for Body {
    type Output = ();

    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'a>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'a>, ()> {
        match self {
            Self::Uninstall => crate::Ended::uninstall(owner, (), ()),
            Self::Drop => {
                drop(owner);
                pending().await
            }
            Self::Forget => {
                core::mem::forget(owner);
                pending().await
            }
            Self::Foreign => {
                pending::<()>().await;
                crate::Ended::uninstall(owner, (), ())
            }
            Self::CancelEnable => {
                let enabling = owner.enable();
                drop(enabling);
                pending().await
            }
            Self::Phase => {
                let mut enabled = match owner.enable().await {
                    Ok(enabled) => enabled,
                    Err(Failed::Poisoned(poisoned)) => {
                        return crate::Ended::uninstall(poisoned, (), ());
                    }
                    Err(failed) => return crate::Ended::fail_stopped(failed.fail_stop(), ()),
                };
                {
                    let mut phase = enabled.phase();
                    assert!(phase.next_event().await.is_ok());
                    assert!(phase.next_event().await.is_err());
                }
                match enabled.disable(&mut |_| {}).await {
                    Ok(disabled) => crate::Ended::uninstall(disabled, (), ()),
                    Err(poisoned) => crate::Ended::uninstall(poisoned, (), ()),
                }
            }
        }
    }
}
fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(core::task::Waker::noop()))
}
fn finish<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    for _ in 0..8 {
        if let Poll::Ready(value) = poll(future.as_mut()) {
            return value;
        }
    }
    panic!("model did not finish");
}
#[test]
fn uninstall_retires_detaches_and_returns_owners_in_order() {
    let m = Model::new(Mode::Normal);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Uninstall)),
        PortEnd::Uninstalled {
            uninstalled: 7,
            output: ()
        }
    ));
    assert_eq!(
        &*m.log.borrow(),
        &["drive", "retire", "detach", "uninstall"]
    );
}
#[test]
fn fault_before_enable_retains_state_for_poisoned_uninstall() {
    let m = Model::new(Mode::FaultBeforeEnable);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Phase)),
        PortEnd::Uninstalled {
            uninstalled: 7,
            output: ()
        }
    ));
    assert_eq!(
        &*m.log.borrow(),
        &["drive", "poison", "detach", "uninstall"]
    );
}
#[test]
fn fault_after_enable_lets_body_drain_before_uninstall() {
    let m = Model::new(Mode::FaultAfterEnable);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Phase)),
        PortEnd::Uninstalled {
            uninstalled: 7,
            output: ()
        }
    ));
    assert_eq!(
        &*m.log.borrow(),
        &[
            "enable",
            "drive",
            "poison",
            "event",
            "disable",
            "detach",
            "uninstall"
        ]
    );
}
#[test]
fn missed_retire_bound_fail_stops_without_detach_or_uninstall() {
    let m = Model::new(Mode::MissBound);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Uninstall)),
        PortEnd::FailStopped { output: () }
    ));
    assert_eq!(&*m.log.borrow(), &["drive", "abandon"]);
}
#[test]
fn dropped_owner_and_cancelled_transition_retire_asynchronously() {
    for body in [Body::Drop, Body::CancelEnable] {
        let m = Model::new(Mode::Normal);
        assert!(matches!(
            finish(run_port(m.installed(), body)),
            PortEnd::Abandoned
        ));
        assert_eq!(&*m.log.borrow(), &["abandon", "drive", "retire", "detach"]);
    }
}
#[test]
fn driver_progresses_beside_foreign_wait_and_core_cancellation_abandons_once() {
    let m = Model::new(Mode::Normal);
    let mut port = Box::pin(run_port(m.installed(), Body::Foreign));
    assert!(poll(port.as_mut()).is_pending());
    assert_eq!(&*m.log.borrow(), &["drive"]);
    drop(port);
    assert_eq!(&*m.log.borrow(), &["drive", "abandon"]);
}
#[test]
fn forgotten_owner_stays_owned_by_core_until_cancellation() {
    let m = Model::new(Mode::Normal);
    let mut port = Box::pin(run_port(m.installed(), Body::Forget));
    assert!(poll(port.as_mut()).is_pending());
    drop(port);
    assert_eq!(&*m.log.borrow(), &["drive", "abandon"]);
}
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
#[test]
fn retire_request_wakes_the_registered_driver_without_a_poll() {
    let request = Signal::new();
    let latch = AbandonLatch::new();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = wakes.clone().into();
    let mut future = pin!(
        Retire {
            request: &request,
            latch: &latch
        }
        .requested()
    );
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    request.set();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(poll(future.as_mut()).is_ready());
}
#[test]
fn abandon_wakes_both_core_and_driver_and_stays_latched() {
    let latch = AbandonLatch::new();
    let core = Arc::new(Wakes(AtomicUsize::new(0)));
    let driver = Arc::new(Wakes(AtomicUsize::new(0)));
    let cw = core.clone().into();
    let dw = driver.clone().into();
    assert!(latch.core.poll(&mut Context::from_waker(&cw)).is_pending());
    let mut future = pin!(latch.abandoned());
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(&dw))
            .is_pending()
    );
    latch.set();
    assert_eq!(core.0.load(Ordering::Relaxed), 1);
    assert_eq!(driver.0.load(Ordering::Relaxed), 1);
    assert!(poll(future.as_mut()).is_ready());
    assert!(latch.core.poll(&mut Context::from_waker(&cw)).is_ready());
}

#[test]
fn cancellation_while_transition_or_retirement_waits_abandons_once() {
    for (mode, body) in [
        (Mode::StallRetire, Body::Uninstall),
        (Mode::StallEnable, Body::Phase),
    ] {
        let model = Model::new(mode);
        let mut port = Box::pin(run_port(model.installed(), body));
        assert!(poll(port.as_mut()).is_pending());
        drop(port);
        assert_eq!(&*model.log.borrow(), &["drive", "abandon"]);
    }
}

struct StopThenWait<'a>(&'a Signal);
impl<'a, 'm> PortBody<Backend<'m>> for StopThenWait<'a> {
    type Output = &'static str;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'m>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'m>, Self::Output> {
        let proof = owner.fail_stop();
        poll_fn(|cx| self.0.poll(cx)).await;
        Ended::fail_stopped(proof, "body result")
    }
}

#[test]
fn fail_stop_masks_irqs_and_retires_while_body_waits_then_detaches_once() {
    let model = Model::new(Mode::Normal);
    let returning = Signal::new();
    model.interrupt();
    let mut port = Box::pin(run_port(model.installed(), StopThenWait(&returning)));
    assert!(poll(port.as_mut()).is_pending());
    model.interrupt();
    assert_eq!(
        model.irq_state.get(),
        1,
        "an IRQ cannot reach quarantined state"
    );
    assert_eq!(&*model.log.borrow(), &["abandon", "drive", "retire"]);
    returning.set();
    assert!(matches!(
        poll(port.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: "body result"
        })
    ));
    assert_eq!(
        &*model.log.borrow(),
        &["abandon", "drive", "retire", "detach"]
    );
    drop(port);
    assert_eq!(
        model
            .log
            .borrow()
            .iter()
            .filter(|event| **event == "detach")
            .count(),
        1
    );
}

#[test]
fn cancellation_during_fail_stopped_body_runs_synchronous_quarantine() {
    let model = Model::new(Mode::Normal);
    let returning = Signal::new();
    let mut port = Box::pin(run_port(model.installed(), StopThenWait(&returning)));
    assert!(poll(port.as_mut()).is_pending());
    drop(port);
    model.interrupt();
    assert_eq!(model.irq_state.get(), 0);
    assert_eq!(
        &*model.log.borrow(),
        &["abandon", "drive", "retire", "abandon"]
    );
}

#[test]
fn retire_timeout_does_not_wait_for_the_fail_stopped_body() {
    let model = Model::new(Mode::MissBound);
    let returning = Signal::new();
    let mut port = Box::pin(run_port(model.installed(), StopThenWait(&returning)));
    assert!(poll(port.as_mut()).is_pending());
    assert!(model.irq_masked.get());
    returning.set();
    assert!(matches!(
        poll(port.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: "body result"
        })
    ));
    assert_eq!(&*model.log.borrow(), &["abandon", "drive"]);
}

#[test]
fn forgetting_the_whole_port_cannot_return_owners_or_run_drop_cleanup() {
    let model = Model::new(Mode::Normal);
    let mut port = Box::pin(run_port(model.installed(), Body::Foreign));
    assert!(poll(port.as_mut()).is_pending());
    core::mem::forget(port);
    assert_eq!(&*model.log.borrow(), &["drive"]);
}
