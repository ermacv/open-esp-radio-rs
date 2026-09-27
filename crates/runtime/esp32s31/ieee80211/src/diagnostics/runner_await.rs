//! Where the radio loop last awaited, for a stalled image to report.
//!
//! The loop marks each await point before entering it. A task beside the
//! loop reads the mark: an unchanged point and count over a second shows the
//! await the loop cannot leave.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// An await point of the radio loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RunnerAwait {
    Control = 1,
    ActiveTx = 2,
    DrainTx = 3,
    Rx = 4,
    Idle = 5,
}

static POINT: AtomicU8 = AtomicU8::new(0);
static COUNT: AtomicU32 = AtomicU32::new(0);

pub(crate) fn mark(point: RunnerAwait) {
    POINT.store(point as u8, Ordering::Relaxed);
    COUNT.fetch_add(1, Ordering::Relaxed);
}

/// The last await point and how many await points the loop has entered.
pub fn current() -> (u8, u32) {
    (POINT.load(Ordering::Relaxed), COUNT.load(Ordering::Relaxed))
}
