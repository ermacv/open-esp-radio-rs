//! A radio backend's monotonic epoch.
//!
//! Each radio port names its own domain `D`, an empty type its package
//! declares, so an instant of one port never compares with, or schedules
//! on, another port's clock. A port's `ClockInfo` relates its epoch to the
//! image's monotonic time.

use core::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

/// Microseconds in the radio epoch of the port of domain `D`.
///
/// The epoch is not wall-clock time. A backend publishes every instant of
/// one controller instance in the same epoch.
#[repr(transparent)]
pub struct RadioInstant<D> {
    micros: u64,
    domain: PhantomData<fn() -> D>,
}

impl<D> RadioInstant<D> {
    /// Construct an instant from monotonic microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self {
            micros,
            domain: PhantomData,
        }
    }

    /// Monotonic microseconds.
    pub const fn as_micros(self) -> u64 {
        self.micros
    }

    /// The instant `duration` later, or `None` on overflow.
    pub const fn checked_add(self, duration: RadioDuration) -> Option<Self> {
        match self.micros.checked_add(duration.0 as u64) {
            Some(micros) => Some(Self::from_micros(micros)),
            None => None,
        }
    }
}

impl<D> Clone for RadioInstant<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for RadioInstant<D> {}

impl<D> PartialEq for RadioInstant<D> {
    fn eq(&self, other: &Self) -> bool {
        self.micros == other.micros
    }
}

impl<D> Eq for RadioInstant<D> {}

impl<D> PartialOrd for RadioInstant<D> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<D> Ord for RadioInstant<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.micros.cmp(&other.micros)
    }
}

impl<D> Hash for RadioInstant<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.micros.hash(state);
    }
}

impl<D> fmt::Debug for RadioInstant<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RadioInstant")
            .field(&self.micros)
            .finish()
    }
}

/// A non-negative span of microseconds.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RadioDuration(u32);

impl RadioDuration {
    /// Construct a duration from microseconds.
    pub const fn from_micros(micros: u32) -> Self {
        Self(micros)
    }

    /// Microseconds.
    pub const fn as_micros(self) -> u32 {
        self.0
    }
}

/// Why a window is not representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowError {
    /// The window has no duration.
    Empty,
    /// The window ends past the representable epoch.
    Overflow,
}

/// A non-empty interval `[start, start + duration)` reserved for one event
/// on the clock of the port of domain `D`.
pub struct RadioWindow<D> {
    start: RadioInstant<D>,
    duration: RadioDuration,
}

impl<D> RadioWindow<D> {
    /// Reserve `duration` from `start`.
    pub const fn new(start: RadioInstant<D>, duration: RadioDuration) -> Result<Self, WindowError> {
        if duration.0 == 0 {
            return Err(WindowError::Empty);
        }
        if start.checked_add(duration).is_none() {
            return Err(WindowError::Overflow);
        }
        Ok(Self { start, duration })
    }

    /// First reserved instant.
    pub const fn start(self) -> RadioInstant<D> {
        self.start
    }

    /// Reserved span.
    pub const fn duration(self) -> RadioDuration {
        self.duration
    }

    /// First instant after the window.
    pub const fn end(self) -> RadioInstant<D> {
        RadioInstant::from_micros(self.start.micros + self.duration.0 as u64)
    }

    /// Whether the two windows share an instant. Touching windows do not.
    pub const fn overlaps(self, other: Self) -> bool {
        self.start.micros < other.end().micros && other.start.micros < self.end().micros
    }
}

impl<D> Clone for RadioWindow<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for RadioWindow<D> {}

impl<D> PartialEq for RadioWindow<D> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.duration == other.duration
    }
}

impl<D> Eq for RadioWindow<D> {}

impl<D> Hash for RadioWindow<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.start.hash(state);
        self.duration.hash(state);
    }
}

impl<D> fmt::Debug for RadioWindow<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RadioWindow")
            .field("start", &self.start)
            .field("duration", &self.duration)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{RadioDuration, RadioInstant, RadioWindow, WindowError};

    /// A test port's domain.
    enum Port {}

    fn window(start: u64, duration: u32) -> RadioWindow<Port> {
        RadioWindow::new(
            RadioInstant::<Port>::from_micros(start),
            RadioDuration::from_micros(duration),
        )
        .unwrap()
    }

    #[test]
    fn windows_are_non_empty_and_touching_windows_do_not_overlap() {
        assert_eq!(
            RadioWindow::new(
                RadioInstant::<Port>::from_micros(5),
                RadioDuration::from_micros(0)
            ),
            Err(WindowError::Empty)
        );
        assert_eq!(
            RadioWindow::new(
                RadioInstant::<Port>::from_micros(u64::MAX),
                RadioDuration::from_micros(1)
            ),
            Err(WindowError::Overflow)
        );
        assert_eq!(
            window(100, 50).end(),
            RadioInstant::<Port>::from_micros(150)
        );
        assert!(!window(100, 50).overlaps(window(150, 10)));
        assert!(window(100, 50).overlaps(window(149, 10)));
        assert!(window(100, 50).overlaps(window(90, 20)));
    }
}
