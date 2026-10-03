//! Deadlines of the station's state machines on the image's monotonic time.

use oer_time::{Duration, Instant};

/// The deadline `duration` after `now`.
///
/// A deadline past the representable range of monotonic time never comes, so
/// it is the end of that range rather than an error: `u64` microseconds last
/// far longer than any image runs.
pub(crate) const fn deadline_after(now: Instant, duration: Duration) -> Instant {
    now.saturating_add(duration)
}

/// The earlier of two optional deadlines.
pub const fn earliest(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.as_micros() <= right.as_micros() {
            left
        } else {
            right
        }),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_past_the_time_range_never_comes() {
        let late = Instant::from_micros(u64::MAX - 1);
        assert_eq!(
            deadline_after(late, Duration::from_secs(1)),
            Instant::from_micros(u64::MAX)
        );
        assert_eq!(
            deadline_after(Instant::from_micros(5), Duration::from_micros(10)),
            Instant::from_micros(15)
        );
    }

    #[test]
    fn earliest_keeps_the_sooner_deadline() {
        let soon = Some(Instant::from_micros(1));
        let late = Some(Instant::from_micros(2));
        assert_eq!(earliest(soon, late), soon);
        assert_eq!(earliest(late, soon), soon);
        assert_eq!(earliest(None, late), late);
        assert_eq!(earliest(None, None), None);
    }
}
