//! Monotonic radio time.

/// Microseconds in the backend's monotonic radio epoch.
///
/// The epoch is not wall-clock time. A backend publishes every instant of
/// one controller instance in the same epoch.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RadioInstant(u64);

impl RadioInstant {
    /// Construct an instant from monotonic microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Monotonic microseconds.
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// The instant `duration` later, or `None` on overflow.
    pub const fn checked_add(self, duration: RadioDuration) -> Option<Self> {
        match self.0.checked_add(duration.0 as u64) {
            Some(micros) => Some(Self(micros)),
            None => None,
        }
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

/// A non-empty interval `[start, start + duration)` reserved for one event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RadioWindow {
    start: RadioInstant,
    duration: RadioDuration,
}

impl RadioWindow {
    /// Reserve `duration` from `start`.
    pub const fn new(start: RadioInstant, duration: RadioDuration) -> Result<Self, WindowError> {
        if duration.0 == 0 {
            return Err(WindowError::Empty);
        }
        if start.checked_add(duration).is_none() {
            return Err(WindowError::Overflow);
        }
        Ok(Self { start, duration })
    }

    /// First reserved instant.
    pub const fn start(self) -> RadioInstant {
        self.start
    }

    /// Reserved span.
    pub const fn duration(self) -> RadioDuration {
        self.duration
    }

    /// First instant after the window.
    pub const fn end(self) -> RadioInstant {
        RadioInstant(self.start.0 + self.duration.0 as u64)
    }

    /// Whether the two windows share an instant. Touching windows do not.
    pub const fn overlaps(self, other: Self) -> bool {
        self.start.0 < other.end().0 && other.start.0 < self.end().0
    }
}

#[cfg(test)]
mod tests {
    use super::{RadioDuration, RadioInstant, RadioWindow, WindowError};

    fn window(start: u64, duration: u32) -> RadioWindow {
        RadioWindow::new(
            RadioInstant::from_micros(start),
            RadioDuration::from_micros(duration),
        )
        .unwrap()
    }

    #[test]
    fn windows_are_non_empty_and_touching_windows_do_not_overlap() {
        assert_eq!(
            RadioWindow::new(RadioInstant::from_micros(5), RadioDuration::from_micros(0)),
            Err(WindowError::Empty)
        );
        assert_eq!(
            RadioWindow::new(
                RadioInstant::from_micros(u64::MAX),
                RadioDuration::from_micros(1)
            ),
            Err(WindowError::Overflow)
        );
        assert_eq!(window(100, 50).end(), RadioInstant::from_micros(150));
        assert!(!window(100, 50).overlaps(window(150, 10)));
        assert!(window(100, 50).overlaps(window(149, 10)));
        assert!(window(100, 50).overlaps(window(90, 20)));
    }
}
