use core::cell::Cell;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_time::{Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::{LocalTimeCounter, MAC_CLOCK_INFO, MacClock};

/// A MAC counter that reads the virtual monotonic time plus an offset, or
/// holds still while stopped.
struct Counter<'c> {
    clock: &'c VirtualClock,
    offset: Cell<u32>,
    stopped_at: Cell<Option<u32>>,
}

impl<'c> Counter<'c> {
    fn new(clock: &'c VirtualClock, offset: u32) -> Self {
        Self {
            clock,
            offset: Cell::new(offset),
            stopped_at: Cell::new(None),
        }
    }

    /// The raw value the counter reads at the monotonic instant `at`.
    fn raw_at(&self, at: Instant) -> u32 {
        (at.as_micros() as u32).wrapping_add(self.offset.get())
    }
}

impl LocalTimeCounter for &Counter<'_> {
    fn read(&self) -> u32 {
        self.stopped_at
            .get()
            .unwrap_or_else(|| self.raw_at(oer_time::Clock::now(self.clock)))
    }
}

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

#[test]
fn a_reception_converts_to_its_monotonic_time() {
    let clock = VirtualClock::new();
    clock.advance_to(at(1_000_000));
    let counter = Counter::new(&clock, 2_596_124);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    mac.now();
    let stamped = counter.raw_at(at(1_000_000));
    // The frame is handed off 400 us after its reception.
    clock.advance(Duration::from_micros(400)).unwrap();
    assert_eq!(mac.received_at(stamped), Some(at(1_000_000)));
    let sample = mac.sample();
    assert_eq!(sample.generation, 0);
    assert_eq!(sample.monotonic, at(1_000_400));
    assert_eq!(
        MAC_CLOCK_INFO
            .to_monotonic_with(mac.stamp(stamped).unwrap(), &sample)
            .map(|projected| projected.at),
        Ok(at(1_000_000))
    );
}

#[test]
fn a_stamp_later_than_now_has_no_place() {
    let clock = VirtualClock::new();
    clock.advance_to(at(5_000));
    let counter = Counter::new(&clock, 0);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    mac.now();
    assert_eq!(mac.received_at(counter.raw_at(at(6_000))), None);
}

#[test]
fn readings_carry_across_the_counter_wrap() {
    let clock = VirtualClock::new();
    clock.advance_to(at(10));
    let counter = Counter::new(&clock, u32::MAX - 100);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    let start = mac.now().as_micros();
    clock.advance(Duration::from_micros(500)).unwrap();
    assert_eq!(mac.now().as_micros(), start + 500);
    let stamped = counter.raw_at(at(300));
    assert_eq!(mac.received_at(stamped), Some(at(300)));
}

#[test]
fn a_wake_starts_a_generation_and_earlier_stamps_keep_their_sample() {
    let clock = VirtualClock::new();
    clock.advance_to(at(100_000));
    let counter = Counter::new(&clock, 1_000);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    mac.sample();
    let before_sleep = counter.raw_at(at(100_000));
    // The counter stops for a 20 ms sleep, then runs on from where it held.
    counter.stopped_at.set(Some(counter.raw_at(at(100_050))));
    clock.advance_to(at(120_050));
    counter.stopped_at.set(None);
    counter.offset.set(1_000u32.wrapping_sub(20_000));
    mac.on_rf_wake();
    let after_wake = counter.raw_at(at(120_050));
    clock.advance(Duration::from_micros(30)).unwrap();
    assert_eq!(mac.sample().generation, 1);
    // A frame from before the sleep converts in its own relation.
    assert_eq!(mac.stamp(before_sleep).unwrap().generation, 0);
    assert_eq!(mac.received_at(before_sleep), Some(at(100_000)));
    // A frame after the wake converts in the new one.
    assert_eq!(mac.stamp(after_wake).unwrap().generation, 1);
    assert_eq!(mac.received_at(after_wake), Some(at(120_050)));
}

#[test]
fn a_counter_that_went_back_across_a_wake_leaves_earlier_stamps_without_a_place() {
    let clock = VirtualClock::new();
    clock.advance_to(at(100_000));
    let counter = Counter::new(&clock, 3_000_000_000);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    mac.sample();
    let before_sleep = counter.raw_at(at(100_000));
    clock.advance_to(at(150_000));
    // The counter restarted during the sleep.
    counter
        .offset
        .set(0u32.wrapping_sub(150_000).wrapping_add(7));
    mac.on_rf_wake();
    let wake = mac.sample();
    assert_eq!(wake.generation, 1);
    // The timeline goes on at the monotonic projection of the last sample.
    assert_eq!(wake.radio.as_micros(), 3_000_150_000);
    assert_eq!(mac.received_at(before_sleep), None);
    let after_wake = counter.raw_at(at(150_000));
    clock.advance(Duration::from_micros(10)).unwrap();
    assert_eq!(mac.received_at(after_wake), Some(at(150_000)));
}

#[test]
fn readings_any_time_apart_keep_one_generation() {
    let clock = VirtualClock::new();
    clock.advance_to(at(1_000));
    let counter = Counter::new(&clock, 77);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    mac.sample();
    // Three counter wraps without a reading.
    let later = at(1_000 + (3 << 32) + 250);
    clock.advance_to(later);
    let stamped = counter.raw_at(later);
    clock.advance(Duration::from_micros(40)).unwrap();
    assert_eq!(mac.received_at(stamped), Some(later));
    assert_eq!(mac.sample().generation, 0);
}

#[test]
fn a_counter_jump_outside_a_wake_breaks_the_relation() {
    let clock = VirtualClock::new();
    clock.advance_to(at(10_000));
    let counter = Counter::new(&clock, 0);
    let mac = MacClock::<NoopRawMutex, _, _>::new(&counter, &clock);
    mac.sample();
    let before = counter.raw_at(at(10_000));
    clock.advance_to(at(11_000));
    counter.offset.set(5_000_000);
    let sample = mac.sample();
    assert_eq!(sample.generation, 1);
    // The timeline goes on at the projection, not at the jumped counter.
    assert_eq!(sample.radio.as_micros(), 11_000);
    assert_eq!(mac.received_at(before), None);
}
