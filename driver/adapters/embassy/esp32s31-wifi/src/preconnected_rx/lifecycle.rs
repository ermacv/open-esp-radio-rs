use core::marker::PhantomData;

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment,
};

use crate::rx_dma_service::{ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage};

use super::state::Esp32s31PreconnectedRxState;
use super::{
    Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxDirective,
    Esp32s31PreconnectedRxError, Esp32s31PreconnectedRxIntoLiveFailure,
    Esp32s31PreconnectedRxPhase, Esp32s31PreconnectedRxProgress,
};

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize>
    Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>
where
    D: Esp32s31PreconnectedRxDelay,
{
    pub const fn from_halted(ring: RxRingHalted<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31PreconnectedRxState::Halted(ring),
            _delay: PhantomData,
        }
    }

    pub const fn from_prepared(ring: RxRingStopped<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31PreconnectedRxState::Prepared(ring),
            _delay: PhantomData,
        }
    }

    pub const fn phase(&self) -> Esp32s31PreconnectedRxPhase {
        self.state.phase()
    }

    /// Move the current frontier into another finite phase while leaving an
    /// explicit vacant placeholder for its eventual returned owner.
    pub fn take(&mut self) -> Result<Self, Esp32s31PreconnectedRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        if matches!(state, Esp32s31PreconnectedRxState::Vacant) {
            return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
        }
        Ok(Self {
            state,
            _delay: PhantomData,
        })
    }

    /// Prepare a halted frontier if needed, wait the qualified walker settle
    /// edge, and publish one live RX epoch.
    pub async fn start<M, F>(
        &mut self,
        hardware: &mut M,
        prepare_buffer: F,
    ) -> Result<(), Esp32s31PreconnectedRxError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        let prepared = match state {
            Esp32s31PreconnectedRxState::Halted(halted) => {
                match halted.prepare(hardware, DMA_BUFFER_SIZE as u32, prepare_buffer) {
                    Ok(prepared) => prepared,
                    Err((halted, error)) => {
                        self.state = Esp32s31PreconnectedRxState::Halted(halted);
                        return Err(error.into());
                    }
                }
            }
            Esp32s31PreconnectedRxState::Prepared(prepared) => prepared,
            live @ Esp32s31PreconnectedRxState::Live(_) => {
                self.state = live;
                return Err(Esp32s31PreconnectedRxError::AlreadyStarted);
            }
            Esp32s31PreconnectedRxState::Vacant => {
                return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
            }
        };
        D::after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US).await;
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = Esp32s31PreconnectedRxState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31PreconnectedRxState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    /// Start RX using the production DMA storage bound to this ring.
    pub async fn start_with_storage<M, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<(), Esp32s31PreconnectedRxError>
    where
        M: RxDma,
    {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        let prepared = match state {
            Esp32s31PreconnectedRxState::Halted(halted) => {
                match storage.prepare_halted(halted, hardware) {
                    Ok(prepared) => prepared,
                    Err((halted, error)) => {
                        self.state = Esp32s31PreconnectedRxState::Halted(halted);
                        return Err(error.into());
                    }
                }
            }
            Esp32s31PreconnectedRxState::Prepared(prepared) => prepared,
            live @ Esp32s31PreconnectedRxState::Live(_) => {
                self.state = live;
                return Err(Esp32s31PreconnectedRxError::AlreadyStarted);
            }
            Esp32s31PreconnectedRxState::Vacant => {
                return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
            }
        };
        D::after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US).await;
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = Esp32s31PreconnectedRxState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31PreconnectedRxState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    pub fn stop<M: RxDma>(&mut self, hardware: &mut M) -> Result<(), Esp32s31PreconnectedRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        let Esp32s31PreconnectedRxState::Live(live) = state else {
            self.state = state;
            return Err(Esp32s31PreconnectedRxError::OwnerUnavailable);
        };
        match live.try_stop(hardware) {
            Ok(halted) => {
                self.state = Esp32s31PreconnectedRxState::Halted(halted);
                Ok(())
            }
            Err((live, error)) => {
                self.state = Esp32s31PreconnectedRxState::Live(live);
                Err(error.into())
            }
        }
    }

    pub fn live_mut(
        &mut self,
    ) -> Result<&mut RxRingLive<'storage, COUNT>, Esp32s31PreconnectedRxError> {
        match &mut self.state {
            Esp32s31PreconnectedRxState::Live(ring) => Ok(ring),
            _ => Err(Esp32s31PreconnectedRxError::OwnerUnavailable),
        }
    }

    /// Observe every currently completed descriptor, then recycle the
    /// completed half unless the observer reports a terminal frame.
    ///
    /// The higher-ranked observer lifetime prevents a DMA-buffer reference
    /// from escaping across the recycle edge. A terminal descriptor remains
    /// owned and observed by the live ring for transfer into the next finite
    /// protocol phase.
    pub fn service_completed<M, F, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        mut observe: F,
    ) -> Result<Esp32s31PreconnectedRxProgress, Esp32s31PreconnectedRxError>
    where
        M: RxDma,
        F: for<'frame> FnMut(RxSegment<'frame>) -> Esp32s31PreconnectedRxDirective,
    {
        let ring = self.live_mut()?;
        let mut progress = Esp32s31PreconnectedRxProgress::default();
        for index in 0..COUNT {
            let Some(completed) = storage.take_completed(ring, index)? else {
                continue;
            };
            progress.completed = progress.completed.saturating_add(1);
            match observe(completed.segment()) {
                Esp32s31PreconnectedRxDirective::Continue => {}
                Esp32s31PreconnectedRxDirective::Pause => break,
                Esp32s31PreconnectedRxDirective::Stop => {
                    progress.stopped = true;
                    return Ok(progress);
                }
            }
        }

        storage.recycle_completed_half(ring, hardware)?;
        if ring.all_observed() {
            return Err(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt));
        }
        Ok(progress)
    }

    pub fn take_live(
        &mut self,
    ) -> Result<RxRingLive<'storage, COUNT>, Esp32s31PreconnectedRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31PreconnectedRxState::Vacant);
        match state {
            Esp32s31PreconnectedRxState::Live(ring) => Ok(ring),
            state => {
                self.state = state;
                Err(Esp32s31PreconnectedRxError::OwnerUnavailable)
            }
        }
    }

    /// Consume a quiescent pre-connected owner and recover its halted ring.
    ///
    /// A prepared ring has not started DMA and can be demoted synchronously.
    /// Live or vacant owners are returned unchanged: a caller must never use
    /// this conversion as a substitute for stopping the walker.
    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        match self.state {
            Esp32s31PreconnectedRxState::Halted(ring) => Ok(ring),
            Esp32s31PreconnectedRxState::Prepared(ring) => Ok(ring.into_halted()),
            state => Err(Self {
                state,
                _delay: PhantomData,
            }),
        }
    }

    /// Consume the finite protocol owner and materialize a connected RX epoch.
    ///
    /// The pre-connected port deliberately stops and republishes the walker
    /// between finite authentication/WPA2 receive phases. Connected RX cannot
    /// inherit that port's partial completion frontier as though it were the
    /// vendor's continuously owned `wDevCtrl` list. Confirming walker stop and
    /// rebuilding the complete static ring makes the connected publication a
    /// new, explicit DMA ownership epoch; this is not a MAC or software reset.
    ///
    /// The complete pre-connected owner is returned on every failure. The
    /// application/HIL therefore cannot strand a halted or prepared ring
    /// between the WPA2 and connected phases.
    #[allow(clippy::result_large_err)]
    pub async fn try_into_live_with_storage<M, const DMA_STORAGE_SIZE: usize>(
        mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<
        RxRingLive<'storage, COUNT>,
        Esp32s31PreconnectedRxIntoLiveFailure<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    >
    where
        M: RxDma,
    {
        if self.phase() == Esp32s31PreconnectedRxPhase::Live
            && let Err(error) = self.stop(hardware)
        {
            return Err(Esp32s31PreconnectedRxIntoLiveFailure { owner: self, error });
        }
        if let Err(error) = self.start_with_storage(hardware, storage).await {
            return Err(Esp32s31PreconnectedRxIntoLiveFailure { owner: self, error });
        }
        match self.take_live() {
            Ok(ring) => Ok(ring),
            Err(error) => Err(Esp32s31PreconnectedRxIntoLiveFailure { owner: self, error }),
        }
    }
}
