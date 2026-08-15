//! Role-neutral ESP32-S31 receive-ring lifecycle.
//!
//! This owner contains only the prepared/live/halted DMA transition and one
//! finite completed-frontier visitor. Scan, standalone monitor and future AP
//! receive policy wrap it without acquiring another descriptor capability.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment,
};

use crate::rx_dma_service::Esp32s31RxDmaStorage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxRingPhase {
    Prepared,
    Live,
    Halted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxRingOwnerError {
    InvalidPhase {
        expected: Esp32s31RxRingPhase,
        actual: Esp32s31RxRingPhase,
    },
    Ring(RxRingError),
}

impl From<RxRingError> for Esp32s31RxRingOwnerError {
    fn from(error: RxRingError) -> Self {
        Self::Ring(error)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31RxRingServiceProgress {
    pub completed_descriptors: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

enum Esp32s31RxRingState<'storage, const COUNT: usize> {
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Halted(RxRingHalted<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> Esp32s31RxRingState<'_, COUNT> {
    const fn phase(&self) -> Esp32s31RxRingPhase {
        match self {
            Self::Prepared(_) => Esp32s31RxRingPhase::Prepared,
            Self::Live(_) => Esp32s31RxRingPhase::Live,
            Self::Halted(_) => Esp32s31RxRingPhase::Halted,
            Self::Vacant => unreachable!(),
        }
    }
}

pub struct Esp32s31RxRingOwner<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    state: Esp32s31RxRingState<'storage, COUNT>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31RxRingOwner<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<Self, RxRingError> {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err(RxRingError::Size);
        }
        let ring = storage.prepare_ring(hardware, descriptor_base, buffer_addresses)?;
        Ok(Self {
            state: Esp32s31RxRingState::Prepared(ring),
            storage,
        })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<Self, RxRingError> {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err(RxRingError::Size);
        }
        let ring = storage.prepare_ring(hardware, descriptor_base, buffer_addresses)?;
        Ok(Self {
            state: Esp32s31RxRingState::Prepared(ring),
            storage,
        })
    }

    pub const fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            state: Esp32s31RxRingState::Halted(ring),
            storage,
        }
    }

    pub const fn phase(&self) -> Esp32s31RxRingPhase {
        self.state.phase()
    }

    pub fn prepare_initial_or_retry<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxRingOwnerError> {
        match self.phase() {
            Esp32s31RxRingPhase::Prepared => Ok(()),
            Esp32s31RxRingPhase::Halted => self.prepare_next(hardware),
            actual @ Esp32s31RxRingPhase::Live => Err(Esp32s31RxRingOwnerError::InvalidPhase {
                expected: Esp32s31RxRingPhase::Prepared,
                actual,
            }),
        }
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxRingOwnerError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxRingState::Vacant);
        let Esp32s31RxRingState::Prepared(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31RxRingOwnerError::InvalidPhase {
                expected: Esp32s31RxRingPhase::Prepared,
                actual,
            });
        };
        match ring.try_start(hardware) {
            Ok(ring) => {
                self.state = Esp32s31RxRingState::Live(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31RxRingState::Prepared(ring);
                Err(error.into())
            }
        }
    }

    /// Visit every descriptor in one finite completed frontier and then
    /// recycle the contiguous observed prefix.
    pub fn service_completed<H, F>(
        &mut self,
        hardware: &mut H,
        mut observe: F,
    ) -> Result<Esp32s31RxRingServiceProgress, Esp32s31RxRingOwnerError>
    where
        H: RxDma,
        F: FnMut(RxSegment<'_>),
    {
        let actual = self.state.phase();
        let Esp32s31RxRingState::Live(ring) = &mut self.state else {
            return Err(Esp32s31RxRingOwnerError::InvalidPhase {
                expected: Esp32s31RxRingPhase::Live,
                actual,
            });
        };
        ring.complete_pending_reload(hardware)?;
        let mut progress = Esp32s31RxRingServiceProgress::default();
        ring.observe_exhausted_republication(hardware);
        let frontier = ring.completed_descriptor_frontier();
        for step in 0..frontier.descriptor_count {
            let index = (frontier.start_index + step) % COUNT;
            let Some(completed) = self.storage.take_completed(ring, index)? else {
                return Err(RxRingError::Corrupt.into());
            };
            progress.completed_descriptors = progress.completed_descriptors.saturating_add(1);
            observe(completed.segment());
        }
        if !progress.reload_pending
            && let Some(append) = self
                .storage
                .recycle_completed_prefix::<COUNT, _>(ring, hardware)?
        {
            progress.recycled_descriptors = append.descriptor_count as u32;
        }
        // A current-LAST completion is released by a later RX completion and
        // therefore needs no synthetic wake. Only direct BASE republication
        // can consume its sole IRQ edge and must keep the service runnable.
        progress.service_probe_pending = ring.exhausted_republication_probe_pending();
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxRingOwnerError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxRingState::Vacant);
        let Esp32s31RxRingState::Live(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31RxRingOwnerError::InvalidPhase {
                expected: Esp32s31RxRingPhase::Live,
                actual,
            });
        };
        match ring.try_stop(hardware) {
            Ok(ring) => {
                self.state = Esp32s31RxRingState::Halted(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31RxRingState::Live(ring);
                Err(error.into())
            }
        }
    }

    /// Poison a live arena before retaining this owner for board reset.
    pub(crate) fn require_reset(&mut self) {
        if let Esp32s31RxRingState::Live(ring) = &mut self.state {
            ring.require_reset();
        }
    }

    pub fn prepare_next<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxRingOwnerError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxRingState::Vacant);
        let Esp32s31RxRingState::Halted(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31RxRingOwnerError::InvalidPhase {
                expected: Esp32s31RxRingPhase::Halted,
                actual,
            });
        };
        match self.storage.prepare_halted(ring, hardware) {
            Ok(ring) => {
                self.state = Esp32s31RxRingState::Prepared(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31RxRingState::Halted(ring);
                Err(error.into())
            }
        }
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        match self.state {
            Esp32s31RxRingState::Halted(ring) => Ok(ring),
            Esp32s31RxRingState::Prepared(ring) => Ok(ring.into_halted()),
            state => Err(Self {
                state,
                storage: self.storage,
            }),
        }
    }
}
