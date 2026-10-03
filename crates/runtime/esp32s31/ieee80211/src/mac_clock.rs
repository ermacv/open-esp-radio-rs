//! The Wi-Fi MAC local time shared by the receive path, the radio port and
//! the station's power agent.
//!
//! The MAC local-time counter stamps every received frame
//! (`oer_esp32s31_ieee80211_mac::rx::decode_rx_local_timestamp`). A
//! [`MacClockStorage`] lives in static memory, where every task that reads
//! the clock finds it; each radio start [`starts`](MacClockStorage::start) a
//! fresh clock in it and receives the only [`MacClockHandle`] that reads
//! that clock. The clock widens the counter to a 64-bit timeline
//! ([`MacTimeline`]), pairs it with the image's monotonic clock, and counts
//! the generations of that relation: each RF wake starts one, since the
//! counter's relation to the monotonic clock across RF sleep is not
//! established.

use core::{cell::Cell, future::Future};

use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};
use oer_esp32s31_ieee80211_mac::local_time::MacTimeline;
use oer_ieee80211_lower_mac::{
    ClockInfo, Ieee80211ClockSample, Ieee80211Instant, Ieee80211Stamp, RadioEpoch,
};
use oer_time::{Clock, Duration, Instant};

/// The largest rate difference between the MAC local time and the
/// monotonic clock, in parts per million.
///
/// The HIL scenario `diagnostic-rx-clock` measured about 0.003 ppm (both
/// count one crystal) and ±0.6 µs of residual over 34 s; the bound leaves a
/// wide margin for the conditions it did not exercise.
pub const MAC_CLOCK_DRIFT_PPM: u32 = 1;

/// The radio clock of the MAC local time.
pub const MAC_CLOCK_INFO: ClockInfo = ClockInfo {
    resolution: Duration::from_micros(1),
    epoch: RadioEpoch::Affine {
        drift_ppm: MAC_CLOCK_DRIFT_PPM,
    },
};

/// A source of raw MAC local-time readings.
pub trait LocalTimeCounter {
    /// The 32-bit counter now.
    fn read(&self) -> u32;
}

impl LocalTimeCounter for oer_esp32s31_hal::root::MacLocalTime {
    fn read(&self) -> u32 {
        self.now()
    }
}

/// The MAC clock as the owners that convert receive timestamps and the
/// station's power agent use it.
pub trait ReceptionClock {
    /// The clock relation now, as a value ([`MacClockHandle::snapshot`]);
    /// `None` for a clock a later radio start replaced.
    fn snapshot(&self) -> Option<MacClockSnapshot>;

    /// Start a new generation after an RF wake ([`MacClockHandle::on_rf_wake`]).
    fn on_rf_wake(&self);

    /// The raw counter and the monotonic time read back to back
    /// ([`MacClockHandle::counter_reading`]).
    fn counter_reading(&self) -> Option<(u32, Instant)>;
}

impl<M: RawMutex, L: LocalTimeCounter + Copy, C: Clock> ReceptionClock
    for MacClockHandle<'_, M, L, C>
{
    fn snapshot(&self) -> Option<MacClockSnapshot> {
        MacClockHandle::snapshot(self)
    }

    fn on_rf_wake(&self) {
        MacClockHandle::on_rf_wake(self);
    }

    fn counter_reading(&self) -> Option<(u32, Instant)> {
        MacClockHandle::counter_reading(self)
    }
}

/// The MAC clock relation at one instant, as a value: it converts the
/// receive timestamps of frames received before it without reading the
/// hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacClockSnapshot {
    state: ClockState,
    now: Ieee80211ClockSample,
}

impl MacClockSnapshot {
    /// The sample the snapshot was taken with.
    pub const fn sample(&self) -> Ieee80211ClockSample {
        self.now
    }

    /// The stamp of the raw receive timestamp `raw`, in the generation it
    /// was taken in; `None` when it has no place on the timeline: before
    /// the timeline's start, later than the snapshot, or before a break
    /// that left earlier stamps without a place.
    pub fn stamp(&self, raw: u32) -> Option<Ieee80211Stamp> {
        self.classify(raw).map(|(stamp, _)| stamp)
    }

    /// The monotonic time of the reception the raw receive timestamp `raw`
    /// records, converted with a sample of its own generation; `None` when
    /// the stamp has no place ([`Self::stamp`]).
    pub fn received_at(&self, raw: u32) -> Option<Instant> {
        let (stamp, sample) = self.classify(raw)?;
        MAC_CLOCK_INFO
            .to_monotonic_with(stamp, &sample)
            .ok()
            .map(|projected| projected.at)
    }

    /// The stamp of `raw` and the sample of its generation.
    fn classify(&self, raw: u32) -> Option<(Ieee80211Stamp, Ieee80211ClockSample)> {
        let at = self.state.timeline.place(raw)?;
        if at > self.now.radio.as_micros() {
            return None;
        }
        let sample = match self.state.boundary {
            Some(boundary) if at < boundary.at => boundary.previous?,
            _ => self.now,
        };
        Some((
            Ieee80211Stamp {
                at: Ieee80211Instant::from_micros(at),
                generation: sample.generation,
            },
            sample,
        ))
    }
}

/// A monotonic timer paired with the MAC clock, for owners that wait on the
/// one and convert receive timestamps with the other.
pub struct ReceptionTimer<T, K> {
    pub timer: T,
    pub reception: K,
}

impl<T: Clock, K> Clock for ReceptionTimer<T, K> {
    fn now(&self) -> Instant {
        self.timer.now()
    }
}

impl<T: oer_time::Timer, K> oer_time::Timer for ReceptionTimer<T, K> {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.timer.wait_until(deadline)
    }
}

impl<T, K: ReceptionClock> ReceptionClock for ReceptionTimer<T, K> {
    fn snapshot(&self) -> Option<MacClockSnapshot> {
        self.reception.snapshot()
    }

    fn on_rf_wake(&self) {
        self.reception.on_rf_wake();
    }

    fn counter_reading(&self) -> Option<(u32, Instant)> {
        self.reception.counter_reading()
    }
}

/// Where the last break split the timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Boundary {
    /// The timeline place of the wake.
    at: u64,
    /// The last sample before the wake, for stamps placed before it; `None`
    /// when the counter went back at the wake, which leaves earlier stamps
    /// without a place.
    previous: Option<Ieee80211ClockSample>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClockState {
    timeline: MacTimeline,
    generation: u32,
    /// The latest sample of the current generation.
    current: Option<Ieee80211ClockSample>,
    boundary: Option<Boundary>,
}

/// Static memory for the MAC clock of the radio start in progress.
///
/// A fresh reading lands next to the monotonic projection of the latest
/// sample, so readings may lie any time apart. Outside an RF wake, a
/// reading that disagrees with that projection by more than the sample's
/// uncertainty and drift breaks the relation: the timeline continues at the
/// projection in a new generation, and earlier stamps lose their place.
pub struct MacClockStorage<M: RawMutex, L, C> {
    monotonic: C,
    state: Mutex<M, Cell<StorageState<L>>>,
}

#[derive(Clone, Copy)]
struct StorageState<L> {
    /// The radio start whose handle reads the clock.
    start: u32,
    /// That start's counter; `None` before the first start.
    counter: Option<L>,
    clock: ClockState,
    /// The last TSF epoch handed out ([`MacClockHandle::tsf_epoch`]).
    tsf_epoch: u32,
}

const FRESH_CLOCK: ClockState = ClockState {
    timeline: MacTimeline::new(),
    generation: 0,
    current: None,
    boundary: None,
};

impl<M: RawMutex, L: LocalTimeCounter + Copy, C: Clock> MacClockStorage<M, L, C> {
    /// Storage with no clock started.
    pub const fn new(monotonic: C) -> Self {
        Self {
            monotonic,
            state: Mutex::new(Cell::new(StorageState {
                start: 0,
                counter: None,
                clock: FRESH_CLOCK,
                tsf_epoch: 0,
            })),
        }
    }

    /// Start the clock of one radio start on its MAC local-time `counter`:
    /// a fresh timeline in a generation no earlier start used, so a stamp
    /// of an earlier start never converts. Every handle of an earlier start
    /// stops reading.
    pub fn start(&self, counter: L) -> MacClockHandle<'_, M, L, C> {
        let start = self.state.lock(|cell| {
            let mut state = cell.get();
            let generation = match state.counter {
                Some(_) => state.clock.generation.wrapping_add(1),
                None => FRESH_CLOCK.generation,
            };
            state.start = state.start.wrapping_add(1);
            state.counter = Some(counter);
            state.clock = ClockState {
                generation,
                ..FRESH_CLOCK
            };
            cell.set(state);
            state.start
        });
        MacClockHandle {
            storage: self,
            start,
        }
    }
}

/// The MAC clock of one radio start: the MAC local time with its relation to
/// the monotonic clock. A handle of a start a later one replaced reads
/// nothing.
pub struct MacClockHandle<'s, M: RawMutex, L, C> {
    storage: &'s MacClockStorage<M, L, C>,
    start: u32,
}

impl<M: RawMutex, L, C> Clone for MacClockHandle<'_, M, L, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, L, C> Copy for MacClockHandle<'_, M, L, C> {}

impl<M: RawMutex, L: LocalTimeCounter + Copy, C: Clock> MacClockHandle<'_, M, L, C> {
    /// The MAC local time now.
    pub fn now(&self) -> Option<Ieee80211Instant> {
        self.update(|state, counter, monotonic| read_fresh(state, counter, monotonic, false).radio)
    }

    /// The MAC local time and the monotonic time read back to back, in the
    /// current generation.
    pub fn sample(&self) -> Option<Ieee80211ClockSample> {
        self.update(|state, counter, monotonic| read_fresh(state, counter, monotonic, false))
    }

    /// The clock relation now, as a value that converts the receive
    /// timestamps of frames received before it.
    pub fn snapshot(&self) -> Option<MacClockSnapshot> {
        self.update(|state, counter, monotonic| {
            let now = read_fresh(state, counter, monotonic, false);
            MacClockSnapshot { state: *state, now }
        })
    }

    /// The stamp of the raw receive timestamp `raw` ([`MacClockSnapshot::stamp`]).
    pub fn stamp(&self, raw: u32) -> Option<Ieee80211Stamp> {
        self.snapshot()?.stamp(raw)
    }

    /// The monotonic time of the reception the raw receive timestamp `raw`
    /// records ([`MacClockSnapshot::received_at`]).
    pub fn received_at(&self, raw: u32) -> Option<Instant> {
        self.snapshot()?.received_at(raw)
    }

    /// Start a new generation after an RF wake. A counter that ran on or
    /// held still across the sleep keeps earlier stamps converting with the
    /// last sample before it; a counter that went back or jumped continues
    /// at the monotonic projection of that sample, and earlier stamps lose
    /// their place.
    pub fn on_rf_wake(&self) {
        self.update(|state, counter, monotonic| {
            read_fresh(state, counter, monotonic, true);
        });
    }

    /// The raw MAC local-time counter and the monotonic time, read back to
    /// back, without touching the relation: evidence of what the counter did
    /// across an RF sleep. `None` once a later start replaced this clock.
    pub fn counter_reading(&self) -> Option<(u32, Instant)> {
        self.update(|_, counter, monotonic| (counter.read(), monotonic.now()))
    }

    /// A TSF epoch no owner of a TSF relation took before
    /// ([`TsfRelation::new`](oer_ieee80211_lower_mac::TsfRelation::new)):
    /// one source for every owner, so two never share a generation, across
    /// reconnections and radio starts.
    pub fn tsf_epoch(&self) -> u32 {
        self.storage.state.lock(|cell| {
            let mut state = cell.get();
            state.tsf_epoch = state.tsf_epoch.wrapping_add(1);
            cell.set(state);
            state.tsf_epoch
        })
    }

    /// Run `f` on this start's clock; `None` once a later start replaced it.
    fn update<R>(&self, f: impl FnOnce(&mut ClockState, &L, &C) -> R) -> Option<R> {
        self.storage.state.lock(|cell| {
            let mut state = cell.get();
            if state.start != self.start {
                return None;
            }
            let counter = state.counter?;
            let result = f(&mut state.clock, &counter, &self.storage.monotonic);
            cell.set(state);
            Some(result)
        })
    }
}

/// Read both clocks back to back, place the reading and record it as the
/// current sample.
fn read_fresh<L: LocalTimeCounter, C: Clock>(
    state: &mut ClockState,
    counter: &L,
    monotonic: &C,
    wake: bool,
) -> Ieee80211ClockSample {
    let before = monotonic.now();
    let raw = counter.read();
    let after = monotonic.now();
    let window = after.saturating_duration_since(before);
    let last = state.current;
    let (at, continuous) = match last {
        None => (state.timeline.advance(raw), true),
        Some(sample) => {
            let placed = MAC_CLOCK_INFO
                .from_monotonic_with(before, &sample)
                .ok()
                .and_then(|projected| {
                    let near = projected.at.at.as_micros();
                    let tolerance = projected
                        .uncertainty
                        .as_micros()
                        .saturating_add(window.as_micros());
                    state
                        .timeline
                        .place_near(raw, near)
                        .map(|at| (at, near, tolerance))
                });
            match placed {
                Some((at, near, tolerance)) => {
                    // At a wake the counter may have held still for the
                    // sleep: it then lies anywhere since the last sample.
                    let low = if wake {
                        sample.radio.as_micros()
                    } else {
                        near.saturating_sub(tolerance)
                    };
                    if (low..=near.saturating_add(tolerance)).contains(&at) {
                        (state.timeline.settle(raw, at), true)
                    } else {
                        (state.timeline.settle(raw, near), false)
                    }
                }
                None => (state.timeline.settle(raw, sample.radio.as_micros()), false),
            }
        }
    };
    if wake || !continuous {
        state.generation = state.generation.wrapping_add(1);
        state.boundary = Some(Boundary {
            at,
            previous: if continuous { last } else { None },
        });
    }
    let sample = Ieee80211ClockSample {
        radio: Ieee80211Instant::from_micros(at),
        monotonic: before,
        uncertainty: window,
        generation: state.generation,
    };
    state.current = Some(sample);
    sample
}

/// A counter that reads one fixed value, for tests that need a clock
/// snapshot but no receive timestamps.
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct FixedCounter(pub u32);

#[cfg(test)]
impl LocalTimeCounter for FixedCounter {
    fn read(&self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests;
