//! Same-epoch walker suspension. No descriptor publication or lease reclamation.

use super::*;

/// Exact boundary that prevented same-epoch RX resumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxResumeError {
    WalkerActive,
    ReloadPending,
    CursorChanged,
    EnableUnconfirmed,
}

/// Confirmed disabled walker retaining the complete live descriptor frontier.
///
/// Network leases may remain outstanding and may return while paused. They
/// are reclaimed only after resume through the original live owner. The arena
/// remains claimed as `Live`; dropping this token requires radio reset.
///
/// This is a walker boundary, not permission to calibrate or reset the MAC.
/// The caller must separately stop MAC activity and preserve RX configuration,
/// descriptor memory and cursor registers throughout the pause. Preservation
/// of in-progress radio reception requires hardware qualification.
///
/// A paused owner cannot process or recycle descriptors:
/// ```compile_fail
/// use oer_esp32s31_wifi_dma::rx_ring::RxRingPaused;
/// fn process(paused: &RxRingPaused<'_, 2>) {
///     paused.completed_descriptor_frontier();
/// }
/// ```
pub struct RxRingPaused<'a, const COUNT: usize> {
    ring: RxRingLive<'a, COUNT>,
    cursor: (u32, u32),
}

/// Retained DMA authority after resume could not establish a live epoch.
///
/// Hardware may already be enabled. No live or paused authority is exposed;
/// only a confirmed terminal stop is permitted, and the arena remains poisoned.
pub struct RxRingResumeFailure<'a, const COUNT: usize> {
    ring: RxRingLive<'a, COUNT>,
    error: RxResumeError,
}

impl<'a, const COUNT: usize> RxRingLive<'a, COUNT> {
    /// Disable the walker without discarding observations or rebuilding links.
    ///
    /// An outstanding append must first settle through ordinary RX service.
    /// This operation makes one finite observation; it does not poll for space
    /// or wait for network leases to return. Failure retains the live owner.
    #[allow(clippy::result_large_err)]
    pub fn try_pause<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingPaused<'a, COUNT>, (Self, RxRingError)> {
        if self
            .lifecycle
            .is_some_and(|state| RxDmaArenaState::load(state) == RxDmaArenaState::ResetRequired)
        {
            return Err((self, RxRingError::ResetRequired));
        }
        if self.reload_pending() || mmio.reload_pending() {
            return Err((self, RxRingError::Busy));
        }
        if let Err(error) = disable_receive_inner(mmio) {
            return Err((self, error));
        }
        let cursor = cursor(mmio);
        Ok(RxRingPaused { ring: self, cursor })
    }
}

impl<'a, const COUNT: usize> RxRingPaused<'a, COUNT> {
    /// Resume the same descriptor epoch without BASE writes or reload requests.
    ///
    /// A changed cursor, unexpected walker/reload activity, or failed enable
    /// confirmation quarantines the owner. None is repaired by republishing a
    /// cold ring, which could overwrite a buffer still held by the stack.
    #[allow(clippy::result_large_err)]
    pub fn try_resume<M: RxDma>(
        mut self,
        mmio: &mut M,
    ) -> Result<RxRingLive<'a, COUNT>, RxRingResumeFailure<'a, COUNT>> {
        let result = if mmio.walker_enabled() {
            Err(RxResumeError::WalkerActive)
        } else if mmio.reload_pending() {
            Err(RxResumeError::ReloadPending)
        } else if cursor(mmio) != self.cursor {
            Err(RxResumeError::CursorChanged)
        } else {
            enable_receive_inner(mmio, &self.ring.binding)
                .map_err(|_| RxResumeError::EnableUnconfirmed)
        };
        match result {
            Ok(()) => Ok(self.ring),
            Err(error) => {
                self.ring.require_reset();
                Err(RxRingResumeFailure {
                    ring: self.ring,
                    error,
                })
            }
        }
    }

    /// End the epoch instead of resuming it. Higher layers must first recover
    /// all network leases, exactly as for a normal live-ring terminal stop.
    #[allow(clippy::result_large_err)]
    pub fn try_stop<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingHalted<'a, COUNT>, (Self, RxRingError)> {
        match self.ring.try_stop(mmio) {
            Ok(halted) => Ok(halted),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    cursor: self.cursor,
                },
                error,
            )),
        }
    }
}

impl<'a, const COUNT: usize> RxRingResumeFailure<'a, COUNT> {
    pub const fn error(&self) -> RxResumeError {
        self.error
    }

    /// Confirm hardware stop while retaining the reset requirement.
    #[allow(clippy::result_large_err)]
    pub fn try_stop<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingHalted<'a, COUNT>, (Self, RxRingError)> {
        match self.ring.try_stop(mmio) {
            Ok(halted) => Ok(halted),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    error: self.error,
                },
                error,
            )),
        }
    }
}

fn cursor<M: RxDma>(mmio: &mut M) -> (u32, u32) {
    mmio.with_ordered_cursor(|cursor| (cursor.last_descriptor_low(), cursor.next_descriptor_low()))
}
