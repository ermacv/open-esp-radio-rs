use super::*;

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
fn backend_identities_lie_in_the_reserved_range() {
    let pause: Id = backend_reserved(0).unwrap();
    assert!(pause.is_backend_reserved());
    assert_eq!(backend_reserved::<Id>(255), Some(Id(u32::MAX)));
    assert!(!Id(0).is_backend_reserved());
}

#[derive(Debug)]
enum Error {
    NotInstalled,
    Faulted,
}

impl PortError for Error {
    fn class(&self) -> FailureClass {
        match self {
            Self::NotInstalled => FailureClass::Rejected,
            Self::Faulted => FailureClass::Poisoned,
        }
    }
}

#[test]
fn only_a_poisoned_class_needs_a_reset() {
    assert!(!Error::NotInstalled.is_poisoned());
    assert!(Error::Faulted.is_poisoned());
}

#[test]
fn the_monotonic_clock_counts_microseconds_of_the_image_clock() {
    assert_eq!(ClockInfo::MONOTONIC_MICROS.epoch, RadioEpoch::Monotonic);
    assert_eq!(
        ClockInfo::MONOTONIC_MICROS.resolution,
        Duration::from_micros(1)
    );
}

/// A test port's domain.
enum Port {}

#[test]
fn a_monotonic_radio_clock_converts_exactly_both_ways() {
    let clock = ClockInfo::MONOTONIC_MICROS;
    let radio = RadioInstant::<Port>::from_micros(1_234);
    assert_eq!(clock.to_monotonic(radio), Ok(Instant::from_micros(1_234)));
    assert_eq!(clock.from_monotonic(Instant::from_micros(1_234)), Ok(radio));
}

#[test]
fn unrelated_and_affine_radio_clocks_never_pretend_to_convert() {
    let radio = RadioInstant::<Port>::from_micros(1_234);
    let unrelated = ClockInfo {
        epoch: RadioEpoch::Unrelated,
        ..ClockInfo::MONOTONIC_MICROS
    };
    assert_eq!(unrelated.to_monotonic(radio), Err(EpochError::Unrelated));
    assert_eq!(
        unrelated.from_monotonic::<Port>(Instant::from_micros(1)),
        Err(EpochError::Unrelated)
    );
    let affine = ClockInfo {
        epoch: RadioEpoch::Affine { drift_ppm: 20 },
        ..ClockInfo::MONOTONIC_MICROS
    };
    assert_eq!(affine.to_monotonic(radio), Err(EpochError::NeedsSample));
}

fn stamp(micros: u64, generation: u32) -> RadioStamp<Port> {
    RadioStamp {
        at: RadioInstant::from_micros(micros),
        generation,
    }
}

#[test]
fn an_affine_clock_refuses_a_stamp_of_another_generation() {
    let affine = ClockInfo {
        epoch: RadioEpoch::Affine { drift_ppm: 1 },
        ..ClockInfo::MONOTONIC_MICROS
    };
    let sample = ClockSample::<Port> {
        radio: RadioInstant::from_micros(1_000),
        monotonic: Instant::from_micros(9_000),
        uncertainty: Duration::from_micros(2),
        generation: 4,
    };
    assert_eq!(sample.stamp(), stamp(1_000, 4));
    assert_eq!(
        affine.to_monotonic_with(stamp(1_500, 3), &sample),
        Err(EpochError::StaleSample)
    );
    assert_eq!(
        affine.to_monotonic_with(stamp(1_500, 4), &sample),
        Ok(Projected {
            at: Instant::from_micros(9_500),
            uncertainty: Duration::from_micros(3),
        })
    );
    // A projection into the radio clock carries the sample's generation.
    assert_eq!(
        affine
            .from_monotonic_with::<Port>(Instant::from_micros(9_500), &sample)
            .map(|projected| projected.at),
        Ok(stamp(1_500, 4))
    );
}

#[test]
fn an_affine_clock_projects_from_its_sample_with_a_drift_bound() {
    let affine = ClockInfo {
        epoch: RadioEpoch::Affine { drift_ppm: 20 },
        ..ClockInfo::MONOTONIC_MICROS
    };
    let sample = ClockSample::<Port> {
        radio: RadioInstant::from_micros(1_000_000),
        monotonic: Instant::from_micros(5_000_000),
        uncertainty: Duration::from_micros(3),
        generation: 7,
    };
    // One second after the sample: 20 ppm of it is 20 us.
    assert_eq!(
        affine.to_monotonic_with(stamp(2_000_000, 7), &sample),
        Ok(Projected {
            at: Instant::from_micros(6_000_000),
            uncertainty: Duration::from_micros(23),
        })
    );
    // Before the sample, the distance counts the same way.
    assert_eq!(
        affine.to_monotonic_with(stamp(999_999, 7), &sample),
        Ok(Projected {
            at: Instant::from_micros(4_999_999),
            uncertainty: Duration::from_micros(4),
        })
    );
    assert_eq!(
        affine.from_monotonic_with::<Port>(Instant::from_micros(6_000_000), &sample),
        Ok(Projected {
            at: stamp(2_000_000, 7),
            uncertainty: Duration::from_micros(23),
        })
    );
    // A projection before either epoch is refused, never clamped.
    assert_eq!(
        affine.to_monotonic_with(
            stamp(0, 0),
            &ClockSample::<Port> {
                radio: RadioInstant::from_micros(10),
                monotonic: Instant::from_micros(5),
                uncertainty: Duration::ZERO,
                generation: 0,
            }
        ),
        Err(EpochError::OutOfRange)
    );
}

#[test]
fn a_monotonic_clock_ignores_the_sample_and_an_unrelated_one_refuses() {
    let sample = ClockSample::<Port> {
        radio: RadioInstant::from_micros(1),
        monotonic: Instant::from_micros(1_000),
        uncertainty: Duration::from_micros(50),
        generation: 1,
    };
    assert_eq!(
        ClockInfo::MONOTONIC_MICROS.to_monotonic_with(stamp(77, 0), &sample),
        Ok(Projected {
            at: Instant::from_micros(77),
            uncertainty: Duration::ZERO,
        })
    );
    let unrelated = ClockInfo {
        epoch: RadioEpoch::Unrelated,
        ..ClockInfo::MONOTONIC_MICROS
    };
    assert_eq!(
        unrelated.to_monotonic_with(stamp(77, 1), &sample),
        Err(EpochError::Unrelated)
    );
}
