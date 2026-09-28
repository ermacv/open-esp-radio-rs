//! Counts of the connected station's beacons from RX dispatch to control.
//!
//! A beacon that RX dispatches is published to the control mailbox, or lost
//! to a full mailbox, and control later consumes it. Comparing the three
//! counts locates where beacons stop under load without an observer on the
//! datapath: each is one relaxed atomic increment per beacon.

use core::sync::atomic::{AtomicU32, Ordering};

static PUBLISHED: AtomicU32 = AtomicU32::new(0);
static OVERFLOWED: AtomicU32 = AtomicU32::new(0);
static CONSUMED: AtomicU32 = AtomicU32::new(0);

/// Beacon counts since boot; they wrap at `u32`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BeaconPathCounts {
    /// Beacons RX dispatch handed to the control mailbox.
    pub published: u32,
    /// Of those, beacons a full mailbox refused.
    pub overflowed: u32,
    /// Beacons control consumed.
    pub consumed: u32,
}

impl BeaconPathCounts {
    /// The counts accumulated since `earlier`.
    pub const fn since(self, earlier: Self) -> Self {
        Self {
            published: self.published.wrapping_sub(earlier.published),
            overflowed: self.overflowed.wrapping_sub(earlier.overflowed),
            consumed: self.consumed.wrapping_sub(earlier.consumed),
        }
    }
}

pub fn beacon_path_counts() -> BeaconPathCounts {
    BeaconPathCounts {
        published: PUBLISHED.load(Ordering::Relaxed),
        overflowed: OVERFLOWED.load(Ordering::Relaxed),
        consumed: CONSUMED.load(Ordering::Relaxed),
    }
}

pub(super) fn record_published(accepted: bool) {
    PUBLISHED.fetch_add(1, Ordering::Relaxed);
    if !accepted {
        OVERFLOWED.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_consumed() {
    CONSUMED.fetch_add(1, Ordering::Relaxed);
}
