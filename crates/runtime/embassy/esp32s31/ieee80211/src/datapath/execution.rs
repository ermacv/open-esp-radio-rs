//! Event-driven pause/stop requests for one connected execution epoch.
//!
//! [`super::DatapathRunner::run_controlled`] drains active TX before choosing
//! pause or terminal stop. Pause preserves the connected state; stop retains
//! ordinary prepared-TX cancellation and link shutdown semantics. A pause
//! result is only a scheduler boundary, not MAC/DMA/IRQ or RF quiescence.
//!
//! The S31 connected child task uses this interface. Its parent owns physical
//! MAC/RX/IRQ pause and resumes the same runner without replacing this control
//! owner. Execution requests do not issue shared-radio maintenance grants.

use core::sync::atomic::{AtomicU8, Ordering};
use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

const NONE: u8 = 0;
const PAUSE: u8 = 1;
const STOP: u8 = 2;

/// One connection's execution requests. Stop remains latched until this owner
/// is discarded; resuming a paused runner never resets it. A new connection
/// uses a new control owner rather than clearing a live request channel.
pub struct Control<M: RawMutex> {
    pending: AtomicU8,
    wake: Signal<M, ()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exit<E> {
    Paused,
    Stopped,
    Role(E),
}

impl<M: RawMutex> Control<M> {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU8::new(NONE),
            wake: Signal::new(),
        }
    }

    /// Coalesce pause requests. An already pending stop has higher priority.
    pub fn request_pause(&self) {
        if self.pending.fetch_max(PAUSE, Ordering::AcqRel) == NONE {
            self.wake.signal(());
        }
    }

    pub fn request_stop(&self) {
        if self.pending.swap(STOP, Ordering::AcqRel) != STOP {
            self.wake.signal(());
        }
    }

    pub fn stop_requested(&self) -> bool {
        self.pending.load(Ordering::Acquire) == STOP
    }

    pub(super) async fn wait_boundary(&self) {
        while self.pending.load(Ordering::Acquire) == NONE {
            self.wake.wait().await;
        }
    }

    /// Acknowledge only pause. A stop arriving during TX drain or concurrently
    /// with acknowledgement remains latched for this or the resumed runner.
    pub(super) fn acknowledge_pause(&self) {
        let _ = self
            .pending
            .compare_exchange(PAUSE, NONE, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<M: RawMutex> Default for Control<M> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
