use core::marker::PhantomData;

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDescriptorSnapshot, RxDma, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment,
};

use crate::rx_dma_service::{ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage};

use super::state::Esp32s31PreconnectedRxState;
use super::{
    Esp32s31PreconnectedRx, Esp32s31PreconnectedRxContinuation, Esp32s31PreconnectedRxDelay,
    Esp32s31PreconnectedRxDirective, Esp32s31PreconnectedRxError,
    Esp32s31PreconnectedRxIntoLiveFailure, Esp32s31PreconnectedRxPhase,
    Esp32s31PreconnectedRxProgress, Esp32s31PreconnectedRxSchedulerSnapshot,
    Esp32s31RecycledRxDirective, Esp32s31RecycledRxProgress,
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
    ///
    /// Native models may provide an explicit buffer-preparation callback.
    /// Hardware builds must use [`Self::start_with_storage`] so the callback
    /// cannot bypass the DMA arena's guard restoration.
    #[cfg(not(target_pointer_width = "32"))]
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

    /// Read one descriptor image in any owned, non-vacant RX phase.
    ///
    /// This is observation only: callers receive no descriptor mutation or
    /// DMA-publication authority. It allows lifecycle fault reports to retain
    /// the terminal hardware image after the walker has been stopped.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        match &self.state {
            Esp32s31PreconnectedRxState::Halted(ring) => ring.descriptor_snapshot(index),
            Esp32s31PreconnectedRxState::Prepared(ring) => ring.descriptor_snapshot(index),
            Esp32s31PreconnectedRxState::Live(ring) => ring.descriptor_snapshot(index),
            Esp32s31PreconnectedRxState::Vacant => None,
        }
    }

    /// Snapshot the live software frontier without exposing publication
    /// authority. AP fault evidence retains this before stop consumes the
    /// live owner and discards scheduler bookkeeping.
    pub fn scheduler_snapshot(&self) -> Option<Esp32s31PreconnectedRxSchedulerSnapshot> {
        let Esp32s31PreconnectedRxState::Live(ring) = &self.state else {
            return None;
        };
        Some(Esp32s31PreconnectedRxSchedulerSnapshot {
            recycle_start: ring.recycle_start(),
            accepted_tail: ring.accepted_tail(),
            observed_mask: ring.observed_mask(),
            topology: ring.topology_snapshot(),
        })
    }

    /// Whether a published append still needs its finite reload suffix.
    ///
    /// An exhausted walker cannot raise another RX interrupt while this edge
    /// is pending. Role runners must therefore poll the owner again instead
    /// of treating the absence of a new interrupt as an empty receive ring.
    pub const fn reload_pending(&self) -> bool {
        match &self.state {
            Esp32s31PreconnectedRxState::Live(ring) => ring.reload_pending(),
            _ => false,
        }
    }

    /// Select the next RX scheduler edge from the complete live-ring state.
    ///
    /// Complete vendor `wdevProcessRxSucDataAll` processes through its saved
    /// LAST descriptor, refreshes LAST and continues in the same `ppTask`
    /// invocation. A terminal descriptor writeback, a deferred link-release
    /// proof, or an exhausted-list BASE publication therefore cannot require
    /// another RX-success interrupt: none is guaranteed after `NEXT=0`.
    pub fn service_continuation<M: RxDma>(
        &mut self,
        hardware: &mut M,
    ) -> Result<Esp32s31PreconnectedRxContinuation, Esp32s31PreconnectedRxError> {
        let ring = self.live_mut()?;
        let probe_pending = ring.reload_pending()
            || ring.completion_release_probe_pending()
            || ring.exhausted_republication_probe_pending()
            // RX_DONE can become visible before LAST advances far enough to
            // authorize recycling. The current PP event already covers that
            // writeback; waiting for another interrupt would lose the vendor
            // worker's same-invocation frontier refresh.
            || ring.completed_descriptor_frontier().descriptor_count != 0
            || ring.exhausted_terminal_writeback_pending(hardware);
        Ok(if probe_pending {
            Esp32s31PreconnectedRxContinuation::ProbePending
        } else {
            Esp32s31PreconnectedRxContinuation::AwaitInterrupt
        })
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
        let frontier = ring.completed_descriptor_frontier();
        // This finite port publishes one physical half at a time. Do not mark
        // a later descriptor observed merely because it was needed as the
        // LAST-advance proof for the preceding half; that descriptor belongs
        // to the next service/recycle epoch.
        for step in 0..frontier.descriptor_count.min(COUNT / 2) {
            let index = (frontier.start_index + step) % COUNT;
            let Some(completed) = storage.take_completed(ring, index)? else {
                return Err(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt));
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

        // Batch the append transaction over one physical half-ring. Cold
        // epochs always begin at descriptor zero, so the two halves remain
        // aligned with rev0's physical walker boundary. The AP runner drives
        // the explicit pending-reload suffix even if no later RX IRQ arrives.
        storage.recycle_completed_half(ring, hardware)?;
        if ring.all_observed()
            && !ring.completion_release_probe_pending()
            && !ring.exhausted_republication_probe_pending()
        {
            return Err(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt));
        }
        Ok(progress)
    }

    /// Copy, recycle and completely publish the first complete RX unit.
    ///
    /// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: `wDev_AppendRxBlocks` accepts
    /// the exact head/count returned by one completed receive unit. The AP
    /// path therefore must not retain descriptors until a physical half-ring
    /// happens to complete. `wDev_AppendRxBlocks` does not return between its
    /// reload doorbell and base-repair suffix. Preserve that transaction
    /// boundary asynchronously: the observer sees the independent staging
    /// copy only after hardware has accepted the returned descriptors.
    pub async fn service_completed_unit<M, F, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        staging: &mut [u8],
        mut observe: F,
    ) -> Result<Esp32s31RecycledRxProgress, Esp32s31PreconnectedRxError>
    where
        M: RxDma,
        F: for<'frame> FnMut(RxSegment<'frame>) -> Esp32s31RecycledRxDirective,
    {
        let ring = self.live_mut()?;
        // A cancelled caller can leave an already-published append pending.
        // Finish that transaction before observing another ownership epoch.
        ring.complete_pending_reload(hardware)?;
        ring.observe_exhausted_republication(hardware);
        // SOURCE[libpp:wdevProcessRxSucDataAll]: the vendor receive worker
        // snapshots `hal_mac_rx_get_last_dscr`, processes through that finite
        // frontier, and refreshes LAST before extending the pass. It does not
        // wait for RX_NEXT_DESCRIPTOR before clearing and returning a node.
        let (last_descriptor_low, next_descriptor_low) = hardware.with_ordered_cursor(|cursor| {
            (cursor.last_descriptor_low(), cursor.next_descriptor_low())
        });
        // LAST is the ownership frontier for the following descriptor scan.
        // Order that MMIO observation before reading uncached SRAM.
        hardware.fence();
        let frontier = storage.first_completed_unit_frontier_through_cursor(
            ring,
            last_descriptor_low,
            next_descriptor_low,
        )?;
        if frontier.unit_count == 0 {
            return Ok(Esp32s31RecycledRxProgress::default());
        }
        if !ring.observe_current_completed_unit_link_release(hardware, frontier.descriptor_count) {
            // A repeated LAST/NEXT image is not an ownership proof. This
            // finite wait can only help when hardware actually completes a
            // later descriptor and advances LAST; otherwise retain the unit
            // for the next RX service edge. It remains outside the reload
            // suffix, which must be one uninterrupted transaction.
            D::after_micros(1).await;
            if !ring
                .observe_current_completed_unit_link_release(hardware, frontier.descriptor_count)
            {
                return Ok(Esp32s31RecycledRxProgress::default());
            }
        }

        let unit = storage
            .take_completed_unit(ring, frontier.descriptor_count)?
            .ok_or(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt))?;
        let descriptor_count = unit.descriptor_count();
        let descriptor_address = unit.metadata().descriptor_address();
        let descriptor_word0 = unit.metadata().staged_word0();
        let total_length = unit.total_length();

        let staged = if total_length == 0 || total_length > staging.len() {
            false
        } else {
            let mut offset = 0_usize;
            for step in 0..descriptor_count {
                let source = unit
                    .segment(step)
                    .ok_or(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt))?;
                let end = offset
                    .checked_add(source.len())
                    .ok_or(Esp32s31PreconnectedRxError::Ring(RxRingError::Overflow))?;
                staging
                    .get_mut(offset..end)
                    .ok_or(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt))?
                    .copy_from_slice(source);
                offset = end;
            }
            if offset != total_length {
                return Err(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt));
            }
            true
        };

        let append = unit
            .recycle(hardware)?
            .ok_or(Esp32s31PreconnectedRxError::Ring(RxRingError::Busy))?;
        if append.descriptor_count != descriptor_count {
            return Err(Esp32s31PreconnectedRxError::Ring(RxRingError::Corrupt));
        }

        // Complete the exact vendor append suffix before protocol processing
        // is allowed to borrow the same MAC register owner for a response TX.
        ring.complete_pending_reload(hardware)?;

        let mut progress = Esp32s31RecycledRxProgress {
            completed_units: 1,
            completed_descriptors: u32::try_from(descriptor_count).unwrap_or(u32::MAX),
            discarded_units: u32::from(!staged),
            paused: false,
        };
        if staged {
            let segment = RxSegment {
                descriptor_address,
                descriptor_word0,
                buffer: &staging[..total_length],
                next_descriptor_address: 0,
            };
            progress.paused = observe(segment) == Esp32s31RecycledRxDirective::Pause;
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
