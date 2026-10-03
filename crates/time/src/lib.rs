#![no_std]
#![forbid(unsafe_code)]

//! Chip-neutral monotonic time.
//!
//! Two time axes cross the radio contracts:
//!
//! - [`Instant`] and [`Duration`] are the image's monotonic time, the one
//!   axis every executor task waits on. A [`Clock`] reads it and a [`Timer`]
//!   waits for a deadline on it. Portable code takes both as ports; an
//!   adapter binds them to an executor's time driver and a host model binds
//!   them to virtual time.
//! - [`RadioInstant`] and [`RadioDuration`] are a radio backend's own
//!   monotonic epoch, in which it schedules air events. An instant carries
//!   its port's domain, so two ports' instants never mix. A radio port
//!   states how its epoch relates to the monotonic time, and its
//!   `ClockInfo` (in `oer-radio-port`) converts; nothing here does.
//!
//! Both axes count microseconds in `u64`, so neither wraps within the life
//! of an image. Arithmetic is checked: an operation that would leave the
//! representable range returns `None` or [`TimeOverflow`] instead of
//! wrapping or saturating silently. The one explicit exception is
//! [`Instant::saturating_add`] for deadlines, where the end of the range
//! means "never".

use core::future::Future;

pub mod radio;

pub use radio::{RadioDuration, RadioInstant, RadioWindow, WindowError};

/// A point on the image's monotonic time, in microseconds since an epoch
/// the [`Clock`] defines.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Instant(u64);

impl Instant {
    /// The clock's epoch.
    pub const EPOCH: Self = Self(0);

    /// Construct an instant from microseconds since the epoch.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Microseconds since the epoch.
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// The instant `duration` later, or `None` past the representable range.
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        match self.0.checked_add(duration.0) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
    }

    /// The deadline `duration` later, or the end of representable time when
    /// it lies past it: such a deadline never comes, and `u64` microseconds
    /// outlast any image. Use it for deadlines, never to measure time.
    pub const fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration.0))
    }

    /// The instant `duration` earlier, or `None` before the epoch.
    pub const fn checked_sub(self, duration: Duration) -> Option<Self> {
        match self.0.checked_sub(duration.0) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
    }

    /// The time from `earlier` to `self`, or `None` when `earlier` is later.
    pub const fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        match self.0.checked_sub(earlier.0) {
            Some(micros) => Some(Duration(micros)),
            None => None,
        }
    }

    /// The time from `earlier` to `self`, zero when `earlier` is later.
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }
}

/// A non-negative span of monotonic time, in microseconds.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Duration(u64);

impl Duration {
    /// No time.
    pub const ZERO: Self = Self(0);

    /// Construct a duration from microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Construct a duration from milliseconds; every `u32` fits.
    pub const fn from_millis(millis: u32) -> Self {
        Self(millis as u64 * 1_000)
    }

    /// Construct a duration from seconds; every `u32` fits.
    pub const fn from_secs(secs: u32) -> Self {
        Self(secs as u64 * 1_000_000)
    }

    /// Microseconds.
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// The sum, or `None` past the representable range.
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
    }

    /// The difference, zero when `other` is longer.
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// A deadline lies past the representable range of monotonic time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeOverflow;

/// Reads the image's monotonic time.
///
/// Successive reads never decrease. The resolution is the clock's own; a
/// clock states it where it is bound, and callers must not assume a read
/// changes between two nearby calls.
pub trait Clock {
    /// The current instant.
    fn now(&self) -> Instant;

    /// The instant `duration` from now, or [`TimeOverflow`].
    fn deadline_after(&self, duration: Duration) -> Result<Instant, TimeOverflow> {
        self.now().checked_add(duration).ok_or(TimeOverflow)
    }
}

/// Waits on the image's monotonic time.
///
/// The returned future completes at or after `deadline` and immediately for
/// a deadline already reached. Dropping it cancels the wait and has no
/// other effect, so a timer can bound any other future through `select`.
pub trait Timer: Clock {
    /// Wait until `deadline`.
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()>;

    /// Wait for `duration` from now, or fail without waiting when the
    /// deadline would overflow.
    fn wait_for(&self, duration: Duration) -> impl Future<Output = Result<(), TimeOverflow>> {
        let deadline = self.deadline_after(duration);
        async move {
            self.wait_until(deadline?).await;
            Ok(())
        }
    }
}

impl<T: Clock + ?Sized> Clock for &T {
    fn now(&self) -> Instant {
        (**self).now()
    }
}

impl<T: Timer + ?Sized> Timer for &T {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        (**self).wait_until(deadline)
    }
}

#[cfg(test)]
mod tests;
