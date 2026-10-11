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

/// A backend in one cell, whose owner lends a phase of an event consumer
/// and two interface clients.
struct Model {
    /// Admitted work by identity, with the interface that admitted it.
    running: core::cell::RefCell<std::vec::Vec<(Id, u8)>>,
    events: core::cell::RefCell<std::collections::VecDeque<u32>>,
}

struct Owner<'m>(&'m Model);
struct Events<'m>(&'m Model);
struct Client<'m> {
    model: &'m Model,
    interface: u8,
}
struct Phase<'m>(&'m Model);
struct Control;

impl Client<'_> {
    fn admit(&self, id: Id) {
        self.model.running.borrow_mut().push((id, self.interface));
    }
}

fn ready<T>(value: T) -> core::future::Ready<T> {
    core::future::ready(value)
}

fn end(model: &Model, id: Id) -> Result<(), CancelError> {
    let mut running = model.running.borrow_mut();
    let index = running
        .iter()
        .position(|(running, _)| *running == id)
        .ok_or(CancelError::NotRunning)?;
    running.remove(index);
    model.events.borrow_mut().push_back(id.0);
    Ok(())
}

impl RadioPortOwner for Owner<'_> {
    type Event = u32;
    type Id = Id;
    type Domain = Port;
    type Fault = ();
    type Split<'a>
        = Phase<'a>
    where
        Self: 'a;

    fn next_event(&mut self) -> impl Future<Output = PortResult<u32, EventsLost, ()>> + '_ {
        ready(match self.0.events.borrow_mut().pop_front() {
            Some(event) => Ok(Ok(event)),
            None => Ok(Err(EventsLost)),
        })
    }

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        ready(Ok(Err(ClockError::Unavailable)))
    }

    fn cancel(&mut self, id: Id) -> impl Future<Output = PortResult<(), CancelError, ()>> + '_ {
        ready(Ok(end(self.0, id)))
    }

    fn lifecycle(
        &mut self,
        _: LifecycleCommand,
    ) -> impl Future<Output = PortResult<(), LifecycleError, ()>> + '_ {
        ready(Ok(Ok(())))
    }

    fn split(&mut self) -> Phase<'_> {
        Phase(self.0)
    }
}

impl<'m> RadioPortSplit for Phase<'m> {
    type Fault = ();
    type Events = Events<'m>;
    type Control = Control;
    type Clients = [Client<'m>; 2];

    fn into_parts(self) -> PhaseParts<Events<'m>, Control, [Client<'m>; 2]> {
        let client = |interface| Client {
            model: self.0,
            interface,
        };
        PhaseParts {
            events: Events(self.0),
            control: Control,
            clients: [client(0), client(1)],
        }
    }
}

impl RadioPortEvents for Events<'_> {
    type Event = u32;
    type Fault = ();

    fn next_event(&mut self) -> impl Future<Output = PortResult<u32, EventsLost, ()>> + '_ {
        ready(match self.0.events.borrow_mut().pop_front() {
            Some(event) => Ok(Ok(event)),
            None => Ok(Err(EventsLost)),
        })
    }
}

impl RadioPortClient for Client<'_> {
    type Id = Id;
    type Domain = Port;
    type Fault = ();

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        ready(Ok(Err(ClockError::Unavailable)))
    }

    fn cancel(&self, id: Id) -> impl Future<Output = PortResult<(), CancelError, ()>> + '_ {
        ready(Ok(end(self.model, id)))
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

#[test]
fn a_phase_lends_clients_scoped_to_their_interface_and_its_leftovers_reach_the_owner() {
    let model = Model {
        running: core::cell::RefCell::default(),
        events: core::cell::RefCell::default(),
    };
    let mut owner = Owner(&model);
    {
        let PhaseParts {
            mut events,
            clients: [station, access_point],
            ..
        } = owner.split().into_parts();
        station.admit(Id(1));
        access_point.admit(Id(2));
        // A client ends work once; a second cancel proves the work ended.
        assert_eq!(now_ready(station.cancel(Id(1))), Ok(Ok(())));
        assert_eq!(
            now_ready(station.cancel(Id(1))),
            Ok(Err(CancelError::NotRunning))
        );
        assert_eq!(now_ready(events.next_event()), Ok(Ok(1)));
    }
    // The phase ended with the access point's work in flight: the owner
    // ends it and takes its terminal, then runs an exclusive command.
    assert_eq!(now_ready(owner.cancel(Id(2))), Ok(Ok(())));
    assert_eq!(now_ready(owner.next_event()), Ok(Ok(2)));
    assert_eq!(
        now_ready(owner.lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
}
