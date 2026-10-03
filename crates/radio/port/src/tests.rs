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
