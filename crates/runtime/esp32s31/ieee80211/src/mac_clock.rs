//! The Wi-Fi MAC local time shared by the receive path, the radio port and
//! the station's power agent.
//!
//! The MAC local-time counter stamps every received frame
//! (`oer_esp32s31_ieee80211_mac::rx::decode_rx_local_timestamp`).
//! [`MacClock`] widens it to a 64-bit timeline
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
    /// The clock relation now, as a value ([`MacClock::snapshot`]).
    fn snapshot(&self) -> MacClockSnapshot;

    /// Start a new generation after an RF wake ([`MacClock::on_rf_wake`]).
    fn on_rf_wake(&self);
}

impl<M: RawMutex, L: LocalTimeCounter, C: Clock> ReceptionClock for MacClock<M, L, C> {
    fn snapshot(&self) -> MacClockSnapshot {
        MacClock::snapshot(self)
    }

    fn on_rf_wake(&self) {
        MacClock::on_rf_wake(self);
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
pub struct ReceptionTimer<'c, T, K: ?Sized> {
    pub timer: T,
    pub reception: &'c K,
}

impl<T: Clock, K: ?Sized> Clock for ReceptionTimer<'_, T, K> {
    fn now(&self) -> Instant {
        self.timer.now()
    }
}

impl<T: oer_time::Timer, K: ?Sized> oer_time::Timer for ReceptionTimer<'_, T, K> {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.timer.wait_until(deadline)
    }
}

impl<T, K: ReceptionClock + ?Sized> ReceptionClock for ReceptionTimer<'_, T, K> {
    fn snapshot(&self) -> MacClockSnapshot {
        self.reception.snapshot()
    }

    fn on_rf_wake(&self) {
        self.reception.on_rf_wake();
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

/// The MAC local time with its relation to the monotonic clock.
///
/// A fresh reading lands next to the monotonic projection of the latest
/// sample, so readings may lie any time apart. Outside an RF wake, a
/// reading that disagrees with that projection by more than the sample's
/// uncertainty and drift breaks the relation: the timeline continues at the
/// projection in a new generation, and earlier stamps lose their place.
pub struct MacClock<M: RawMutex, L, C> {
    counter: L,
    monotonic: C,
    state: Mutex<M, Cell<ClockState>>,
}

impl<M: RawMutex, L: LocalTimeCounter, C: Clock> MacClock<M, L, C> {
    pub const fn new(counter: L, monotonic: C) -> Self {
        Self {
            counter,
            monotonic,
            state: Mutex::new(Cell::new(ClockState {
                timeline: MacTimeline::new(),
                generation: 0,
                current: None,
                boundary: None,
            })),
        }
    }

    /// The MAC local time now.
    pub fn now(&self) -> Ieee80211Instant {
        self.update(|state| self.read_fresh(state, false).radio)
    }

    /// The MAC local time and the monotonic time read back to back, in the
    /// current generation.
    pub fn sample(&self) -> Ieee80211ClockSample {
        self.update(|state| self.read_fresh(state, false))
    }

    /// The clock relation now, as a value that converts the receive
    /// timestamps of frames received before it.
    pub fn snapshot(&self) -> MacClockSnapshot {
        self.update(|state| {
            let now = self.read_fresh(state, false);
            MacClockSnapshot { state: *state, now }
        })
    }

    /// The stamp of the raw receive timestamp `raw` ([`MacClockSnapshot::stamp`]).
    pub fn stamp(&self, raw: u32) -> Option<Ieee80211Stamp> {
        self.snapshot().stamp(raw)
    }

    /// The monotonic time of the reception the raw receive timestamp `raw`
    /// records ([`MacClockSnapshot::received_at`]).
    pub fn received_at(&self, raw: u32) -> Option<Instant> {
        self.snapshot().received_at(raw)
    }

    /// Start a new generation after an RF wake. A counter that ran on or
    /// held still across the sleep keeps earlier stamps converting with the
    /// last sample before it; a counter that went back or jumped continues
    /// at the monotonic projection of that sample, and earlier stamps lose
    /// their place.
    pub fn on_rf_wake(&self) {
        self.update(|state| {
            self.read_fresh(state, true);
        });
    }

    /// Read both clocks back to back, place the reading and record it as
    /// the current sample.
    fn read_fresh(&self, state: &mut ClockState, wake: bool) -> Ieee80211ClockSample {
        let before = self.monotonic.now();
        let raw = self.counter.read();
        let after = self.monotonic.now();
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

    fn update<R>(&self, f: impl FnOnce(&mut ClockState) -> R) -> R {
        self.state.lock(|cell| {
            let mut state = cell.get();
            let result = f(&mut state);
            cell.set(state);
            result
        })
    }
}

/// A counter that reads one fixed value, for tests that need a clock
/// snapshot but no receive timestamps.
#[cfg(test)]
pub(crate) struct FixedCounter(pub u32);

#[cfg(test)]
impl LocalTimeCounter for FixedCounter {
    fn read(&self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests;
