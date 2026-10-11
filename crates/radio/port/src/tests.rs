extern crate std;

use super::*;
use oer_time::{Duration, Instant, RadioDuration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Id(u32);

impl Correlation for Id {
    fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    fn raw(self) -> u32 {
        self.0
    }
}

#[test]
fn a_caller_allocator_never_yields_a_backend_identity() {
    let mut ids = CorrelationIds::starting_at(*BACKEND_RESERVED.start() - 2);
    let first: Id = ids.next();
    let second: Id = ids.next();
    let wrapped: Id = ids.next();
    assert_eq!(
        [first.0, second.0, wrapped.0],
        [0xFFFF_FEFE, 0xFFFF_FEFF, 0]
    );
    assert!(
        ![first, second, wrapped]
            .iter()
            .any(|id| id.is_backend_reserved())
    );
    // A reserved start is not a caller identity either.
    let mut ids = CorrelationIds::starting_at(u32::MAX);
    assert_eq!(ids.next::<Id>(), Id(0));
}

#[test]
fn the_monotonic_clock_counts_microseconds_of_the_image_clock() {
    assert_eq!(
        ClockInfo::<Port>::MONOTONIC_MICROS.monotonic,
        Some(ClockRelation::IDENTICAL)
    );
    assert_eq!(
        ClockInfo::<Port>::MONOTONIC_MICROS.resolution,
        Duration::from_micros(1)
    );
}

/// A test port's domain.
enum Port {}

/// A second domain, for a relation that does not involve monotonic time.
enum Other {}

/// Generation `jumps` of the radio-clock relation opened in `epoch`.
fn generation(epoch: u32, jumps: u32) -> Generation<RadioInstant<Port>, Instant> {
    let mut source = GenerationSource::new(epoch);
    for _ in 0..jumps {
        source.jump();
    }
    source.current()
}

fn stamp(micros: u64, jumps: u32) -> RadioStamp<Port> {
    RadioStamp {
        at: RadioInstant::from_micros(micros),
        generation: generation(0, jumps),
    }
}

fn sample(radio: u64, monotonic: u64, uncertainty: u64, jumps: u32) -> ClockSample<Port> {
    ClockSample {
        source: RadioInstant::from_micros(radio),
        target: Instant::from_micros(monotonic),
        uncertainty: Duration::from_micros(uncertainty),
        generation: generation(0, jumps),
    }
}

#[test]
fn a_generation_source_starts_in_its_epoch_and_moves_at_each_break() {
    let mut source = GenerationSource::<RadioInstant<Port>, Instant>::new(7);
    let first = source.current();
    assert_eq!((first.epoch(), first.jump()), (7, 0));
    let second = source.jump();
    assert_ne!(first, second);
    assert_eq!((second.epoch(), second.jump()), (7, 1));
    assert_eq!(source.current(), second);
    // Two owners in different epochs never share a generation.
    assert_ne!(
        GenerationSource::<RadioInstant<Port>, Instant>::new(8).current(),
        first
    );
}

#[test]
fn an_identical_relation_converts_exactly_both_ways_and_ignores_the_sample() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::IDENTICAL;
    let sample = sample(1, 9, 5, 2);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(1_234),
            generation(0, 0),
            &sample
        ),
        Ok(Projected {
            at: Instant::from_micros(1_234),
            uncertainty: Duration::ZERO,
        })
    );
    assert_eq!(
        relation.project_back(Instant::from_micros(1_234), &sample),
        Ok(Projected {
            at: stamp(1_234, 2),
            uncertainty: RadioDuration::ZERO,
        })
    );
}

#[test]
fn an_affine_relation_refuses_a_coordinate_of_another_generation() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::affine(1);
    let sample = sample(1_000, 9_000, 2, 4);
    assert_eq!(sample.stamp(), stamp(1_000, 4));
    let at = RadioInstant::from_micros(1_500);
    assert_eq!(
        relation.project(at, generation(0, 3), &sample),
        Err(ProjectionError::StaleSample)
    );
    assert_eq!(
        relation.project(at, generation(0, 4), &sample),
        Ok(Projected {
            at: Instant::from_micros(9_500),
            uncertainty: Duration::from_micros(3),
        })
    );
    // A projection back carries the sample's generation.
    assert_eq!(
        relation
            .project_back(Instant::from_micros(9_500), &sample)
            .map(|projected| projected.at),
        Ok(stamp(1_500, 4))
    );
}

#[test]
fn an_affine_relation_projects_from_its_sample_with_a_drift_bound() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::affine(20);
    let sample = sample(1_000_000, 5_000_000, 3, 7);
    let current = generation(0, 7);
    // One second after the sample: 20 ppm of it is 20 us.
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(2_000_000),
            current,
            &sample
        ),
        Ok(Projected {
            at: Instant::from_micros(6_000_000),
            uncertainty: Duration::from_micros(23),
        })
    );
    // Before the sample, the distance counts the same way.
    assert_eq!(
        relation.project(RadioInstant::<Port>::from_micros(999_999), current, &sample),
        Ok(Projected {
            at: Instant::from_micros(4_999_999),
            uncertainty: Duration::from_micros(4),
        })
    );
    // Back into the radio domain, the uncertainty is a radio span.
    assert_eq!(
        relation.project_back(Instant::from_micros(6_000_000), &sample),
        Ok(Projected {
            at: stamp(2_000_000, 7),
            uncertainty: RadioDuration::from_micros(23),
        })
    );
}

#[test]
fn a_projection_outside_either_epoch_is_refused_never_clamped() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::affine(20);
    let early = sample(10, 5, 0, 0);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(0),
            generation(0, 0),
            &early
        ),
        Err(ProjectionError::OutOfRange)
    );
    let late = sample(0, u64::MAX, 0, 0);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(1),
            generation(0, 0),
            &late
        ),
        Err(ProjectionError::OutOfRange)
    );
    // An uncertainty past the range is refused the same way.
    let wide = sample(0, 0, u64::MAX, 0);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(1_000_000),
            generation(0, 0),
            &wide
        ),
        Err(ProjectionError::OutOfRange)
    );
}

#[test]
fn one_projection_serves_a_relation_between_two_radio_domains() {
    let relation = ClockRelation::<RadioInstant<Other>, RadioInstant<Port>>::affine(200);
    let mut source = GenerationSource::<RadioInstant<Other>, RadioInstant<Port>>::new(3);
    let sample = RelationSample {
        source: RadioInstant::<Other>::from_micros(1_000_000),
        target: RadioInstant::<Port>::from_micros(10),
        uncertainty: RadioDuration::from_micros(1),
        generation: source.current(),
    };
    let current = source.current();
    assert_eq!(
        relation.project(RadioInstant::from_micros(1_100_000), current, &sample),
        Ok(Projected {
            at: RadioInstant::from_micros(100_010),
            uncertainty: RadioDuration::from_micros(21),
        })
    );
    // After a jump, the sample of the earlier generation is stale.
    let jumped = source.jump();
    assert_eq!(
        relation.project(RadioInstant::from_micros(1_100_000), jumped, &sample),
        Err(ProjectionError::StaleSample)
    );
}

#[test]
fn a_radio_span_converts_only_through_a_relation() {
    let span = RadioDuration::<Port>::from_micros(1_000_000);
    assert_eq!(
        ClockRelation::<RadioInstant<Port>, Instant>::IDENTICAL.project_span(span),
        Ok(Projected {
            at: Duration::from_micros(1_000_000),
            uncertainty: Duration::ZERO,
        })
    );
    let affine = ClockRelation::<RadioInstant<Port>, Instant>::affine(30);
    // 30 ppm over one second, and a part of a microsecond rounds up.
    assert_eq!(
        affine.project_span(span),
        Ok(Projected {
            at: Duration::from_micros(1_000_000),
            uncertainty: Duration::from_micros(30),
        })
    );
    assert_eq!(
        affine
            .project_span(RadioDuration::from_micros(1))
            .map(|projected| projected.uncertainty),
        Ok(Duration::from_micros(1))
    );
}

/// A backend in one cell: its admitted work by slot and client, its
/// queued events, and what its next enable and transitions do.
#[derive(Default)]
struct Model {
    running: core::cell::RefCell<std::vec::Vec<(u32, u8)>>,
    events: core::cell::RefCell<std::collections::VecDeque<u32>>,
    next_slot: core::cell::Cell<u32>,
    refuse_enable: core::cell::Cell<bool>,
    poisoned: core::cell::Cell<bool>,
    /// The transition's end is reported after this many polls of nothing.
    pending_polls: core::cell::Cell<u32>,
    lifecycle: core::cell::RefCell<std::vec::Vec<&'static str>>,
}

/// The model's backend; only `install` wraps it.
struct Backend<'m>(&'m Model);

struct Events<'m>(&'m Model);
struct Control(());
struct Client<'m> {
    model: &'m Model,
    interface: u8,
}
struct Setup(());

/// One admitted operation's token: only the model mints it.
struct Token(u32);

impl RadioWork for Token {
    type Id = u32;

    fn id(&self) -> u32 {
        self.0
    }
}

impl Client<'_> {
    fn admit(&self) -> Work<'_, Token> {
        let slot = self.model.next_slot.get();
        self.model.next_slot.set(slot + 1);
        self.model.running.borrow_mut().push((slot, self.interface));
        Work::new(Token(slot))
    }
}

/// End one admitted operation by its slot; its terminal is queued.
fn end(model: &Model, slot: u32) -> Result<(), CancelError> {
    let mut running = model.running.borrow_mut();
    let index = running
        .iter()
        .position(|(running, _)| *running == slot)
        .ok_or(CancelError::NotRunning)?;
    running.remove(index);
    model.events.borrow_mut().push_back(slot);
    Ok(())
}

fn poisoned<T>(model: &Model) -> Result<T, Poisoned<()>> {
    if model.poisoned.get() {
        Err(Poisoned { cause: () })
    } else {
        Ok(unreachable_ok())
    }
}

fn unreachable_ok<T>() -> T {
    unreachable!("only called on a poisoned model")
}

/// A transition step: the queued events first, then, after the pending
/// polls, the end.
fn step<R>(
    model: &Model,
    end: impl FnOnce() -> R,
) -> core::task::Poll<Result<Step<u32, R>, Poisoned<()>>> {
    if model.poisoned.get() {
        return core::task::Poll::Ready(Err(Poisoned { cause: () }));
    }
    if let Some(event) = model.events.borrow_mut().pop_front() {
        return core::task::Poll::Ready(Ok(Step::Event(event)));
    }
    let pending = model.pending_polls.get();
    if pending > 0 {
        model.pending_polls.set(pending - 1);
        return core::task::Poll::Pending;
    }
    core::task::Poll::Ready(Ok(Step::Ended(end())))
}

impl<'m> PortBackend for Backend<'m> {
    type Event = u32;
    type Domain = Port;
    type Fault = ();
    type EnableFailure = &'static str;
    type Setup<'a>
        = Setup
    where
        Self: 'a;
    type Events<'a>
        = Events<'m>
    where
        Self: 'a;
    type Control<'a>
        = Control
    where
        Self: 'a;
    type Clients<'a>
        = [Client<'m>; 2]
    where
        Self: 'a;
    type Uninstall = ();
    type Uninstalled = &'m Model;

    fn admit_enable(&mut self) -> Result<(), Poisoned<()>> {
        if self.0.poisoned.get() {
            return poisoned(self.0);
        }
        self.0.lifecycle.borrow_mut().push("enable");
        Ok(())
    }

    fn admit_disable(&mut self) -> Result<(), Poisoned<()>> {
        if self.0.poisoned.get() {
            return poisoned(self.0);
        }
        self.0.lifecycle.borrow_mut().push("disable");
        // Disable ends every admitted work with its terminal first.
        let slots: std::vec::Vec<u32> = self
            .0
            .running
            .borrow()
            .iter()
            .map(|(slot, _)| *slot)
            .collect();
        for slot in slots {
            let _ = end(self.0, slot);
        }
        Ok(())
    }

    fn next_enabling(
        &mut self,
    ) -> impl Future<Output = Result<Step<u32, Result<(), &'static str>>, Poisoned<()>>> + '_ {
        let model = self.0;
        core::future::poll_fn(move |_| {
            step(model, || {
                if model.refuse_enable.get() {
                    Err("the radio refused the channel")
                } else {
                    Ok(())
                }
            })
        })
    }

    fn next_disabling(&mut self) -> impl Future<Output = Result<Step<u32, ()>, Poisoned<()>>> + '_ {
        let model = self.0;
        core::future::poll_fn(move |_| step(model, || ()))
    }

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn setup(&mut self) -> Lent<'_, Setup> {
        Lent::new(Setup(()))
    }

    fn split(&mut self) -> PhaseParts<'_, Events<'m>, Control, [Client<'m>; 2]> {
        let model = self.0;
        let client = |interface| Client { model, interface };
        PhaseParts {
            events: Lent::new(Events(model)),
            control: Lent::new(Control(())),
            clients: Lent::new([client(0), client(1)]),
        }
    }

    fn uninstall(self, (): ()) -> &'m Model {
        self.0
    }
}

impl RadioPortEvents for Events<'_> {
    type Event = u32;
    type Fault = ();

    fn next_event(&mut self) -> impl Future<Output = PortResult<u32, EventsLost, ()>> + '_ {
        core::future::ready(match self.0.events.borrow_mut().pop_front() {
            Some(event) => Ok(Ok(event)),
            None => Ok(Err(EventsLost)),
        })
    }
}

impl RadioPortClient for Client<'_> {
    type Token = Token;
    type Domain = Port;
    type Fault = ();

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn end(&self, token: Token) -> impl Future<Output = PortResult<(), CancelError, ()>> + '_ {
        core::future::ready(Ok(end(self.model, token.0)))
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

/// Enable `owner`, which the model accepts.
fn enabled(owner: Owner<Backend<'_>, Disabled>) -> Owner<Backend<'_>, Enabled> {
    let Ok(mut enabling) = owner.enable() else {
        panic!("a healthy model admits Enable");
    };
    match now_ready(enabling.next()) {
        EnableStep::Enabled(owner) => owner,
        _ => panic!("the model enables"),
    }
}

#[test]
fn a_port_runs_a_phase_between_its_lifecycle_commands_and_ends_its_leftovers() {
    let model = Model::default();
    let mut owner = enabled(Owner::install(Backend(&model)));
    {
        let PhaseParts {
            mut events,
            clients,
            ..
        } = owner.split();
        let [station, access_point] = &*clients;
        let joined = station.admit();
        let joined_id = joined.id();
        let _served = access_point.admit();
        // A client ends its own work by its token, once.
        assert_eq!(now_ready(station.cancel(joined)), Ok(Ok(())));
        assert_eq!(now_ready(events.next_event()), Ok(Ok(joined_id)));
    }
    // The access point's work is left in flight: `Disable` ends it, and its
    // terminal comes before the end.
    let Ok(mut disabling) = owner.disable() else {
        panic!("a healthy model admits Disable");
    };
    assert!(matches!(now_ready(disabling.next()), DisableStep::Event(1)));
    let DisableStep::Disabled(owner) = now_ready(disabling.next()) else {
        panic!("the model disables");
    };
    assert_eq!(*model.lifecycle.borrow(), ["enable", "disable"]);
    assert!(core::ptr::eq(owner.uninstall(()), &model));
}

#[test]
fn a_refused_enable_leaves_a_disabled_owner_with_the_worlds_cause() {
    let model = Model::default();
    model.refuse_enable.set(true);
    let Ok(mut enabling) = Owner::install(Backend(&model)).enable() else {
        panic!("a healthy model admits Enable");
    };
    let EnableStep::Failed(owner, cause) = now_ready(enabling.next()) else {
        panic!("the model refuses");
    };
    assert_eq!(cause, "the radio refused the channel");
    // The disabled owner enables again once the world allows it.
    model.refuse_enable.set(false);
    let _ = enabled(owner);
}

#[test]
fn a_dropped_step_loses_nothing() {
    let model = Model::default();
    let Ok(mut enabling) = Owner::install(Backend(&model)).enable() else {
        panic!("a healthy model admits Enable");
    };
    model.pending_polls.set(1);
    {
        let mut step = core::pin::pin!(enabling.next());
        let pending = step.as_mut().poll(&mut core::task::Context::from_waker(
            core::task::Waker::noop(),
        ));
        assert!(pending.is_pending());
        // The step future is dropped here, before the end.
    }
    assert!(matches!(now_ready(enabling.next()), EnableStep::Enabled(_)));
}

#[test]
fn a_poisoned_backend_leaves_only_through_its_uninstall() {
    let model = Model::default();
    let owner = enabled(Owner::install(Backend(&model)));
    model.poisoned.set(true);
    let Err(poisoned) = owner.disable() else {
        panic!("a poisoned model refuses Disable");
    };
    poisoned.cause();
    assert!(core::ptr::eq(poisoned.uninstall(()), &model));
}
