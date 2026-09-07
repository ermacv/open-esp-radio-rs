//! Bounded observation deadlines for the explicitly intrusive TX wait image.

#[derive(Default)]
pub(super) struct Schedule {
    published: u64,
    next: usize,
    armed: bool,
}

impl Schedule {
    const OFFSETS: [u64; 4] = [5_000, 10_000, 20_000, 40_000];

    pub(super) fn new(published: u64) -> Self {
        Self {
            published,
            next: 0,
            armed: true,
        }
    }

    pub(super) fn deadline(&self) -> Option<u64> {
        self.armed.then_some(())?;
        self.published.checked_add(*Self::OFFSETS.get(self.next)?)
    }

    /// Return publication age and wake lateness. Missed observation points
    /// are skipped rather than causing a burst of immediately ready polls.
    pub(super) fn sample(&mut self, now: u64) -> Option<(u64, u64)> {
        let deadline = self.deadline()?;
        if now < deadline {
            return None;
        }
        let elapsed = now - self.published;
        while Self::OFFSETS
            .get(self.next)
            .is_some_and(|offset| *offset <= elapsed)
        {
            self.next += 1;
        }
        Some((elapsed, now - deadline))
    }
}

#[cfg(test)]
mod tests {
    use super::Schedule;

    #[test]
    fn observations_are_bounded_and_early_wakes_do_not_consume_them() {
        let mut schedule = Schedule::new(100);
        assert_eq!(schedule.sample(5_099), None);
        assert_eq!(schedule.deadline(), Some(5_100));
        for offset in [5_000, 10_000, 20_000, 40_000] {
            assert_eq!(schedule.sample(100 + offset), Some((offset, 0)));
        }
        assert_eq!(schedule.deadline(), None);
        assert_eq!(schedule.sample(100_000), None);
    }

    #[test]
    fn late_wake_records_lateness_without_immediate_repolling() {
        let mut schedule = Schedule::new(100);
        assert_eq!(schedule.sample(21_100), Some((21_000, 16_000)));
        assert_eq!(schedule.deadline(), Some(40_100));
        assert_eq!(schedule.sample(21_100), None);
        assert_eq!(Schedule::default().deadline(), None);
        assert_eq!(Schedule::new(u64::MAX).deadline(), None);
    }
}
