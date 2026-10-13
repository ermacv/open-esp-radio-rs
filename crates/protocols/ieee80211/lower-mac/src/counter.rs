//! Saturating diagnostics shared by the portable Wi-Fi owners.

/// A diagnostic count. Exhaustion keeps the maximum and never changes
/// protocol state, operation identity, or admission capacity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counter(u32);

impl Counter {
    /// No events observed.
    pub const ZERO: Self = Self(0);

    /// Count one event.
    pub fn increment(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    /// Combined diagnostic counts, with the same saturation policy.
    pub const fn combined(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Snapshot for reporting.
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturation_never_wraps_a_diagnostic_count() {
        let mut count = Counter(u32::MAX - 1);
        count.increment();
        count.increment();
        assert_eq!(count.get(), u32::MAX);
        assert_eq!(count.combined(count).get(), u32::MAX);
    }
}
