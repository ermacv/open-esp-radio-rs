//! Wire control of the shared PHY's periodic tracking timer.
//!
//! The timer runs from boot. A suspension ends it between ticks, so a started
//! tick always finishes; a resumption starts it again. The counts cover every
//! tick since boot, for comparing traffic with and without tracking.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use oer_esp32s31_phy::{ConcurrentPhyTrackingError, ConcurrentTrackingTick};
use oer_hil_protocol::{phy::PhyTrackingCommand, phy::PhyTrackingEvidence};

static SUSPENDED: AtomicBool = AtomicBool::new(false);
static CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static TRACKED: AtomicU32 = AtomicU32::new(0);
static NOT_DUE: AtomicU32 = AtomicU32::new(0);
static SKIPPED: AtomicU32 = AtomicU32::new(0);

/// Apply one wire command and report the timer.
pub(crate) fn control(command: PhyTrackingCommand) -> oer_hil_protocol::phy::TrackingState {
    match command {
        PhyTrackingCommand::Resume => set_suspended(false),
        PhyTrackingCommand::Suspend => set_suspended(true),
        PhyTrackingCommand::Status => {}
    }
    oer_hil_protocol::phy::TrackingState(PhyTrackingEvidence {
        running: !SUSPENDED.load(Ordering::Acquire),
        tracked: TRACKED.load(Ordering::Relaxed),
        not_due: NOT_DUE.load(Ordering::Relaxed),
        skipped: SKIPPED.load(Ordering::Relaxed),
    })
}

fn set_suspended(suspended: bool) {
    SUSPENDED.store(suspended, Ordering::Release);
    CHANGED.signal(());
}

/// Whether the timer is suspended now.
pub(crate) fn suspended() -> bool {
    SUSPENDED.load(Ordering::Acquire)
}

/// Wait for the next suspension or resumption.
pub(crate) async fn changed() {
    CHANGED.wait().await;
}

/// Complete once the timer is suspended.
pub(crate) async fn until_suspended() {
    while !suspended() {
        changed().await;
    }
}

/// Count one tick's result.
pub(crate) fn record(tick: &Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>) {
    let counter = match tick {
        Ok(ConcurrentTrackingTick::Tracked(_)) => &TRACKED,
        Ok(ConcurrentTrackingTick::NotDue) => &NOT_DUE,
        Ok(ConcurrentTrackingTick::Unavailable(_) | ConcurrentTrackingTick::AwaitingQuiescence) => {
            &SKIPPED
        }
        Err(_) => return,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}
