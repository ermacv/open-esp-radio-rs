use core::marker::PhantomData;

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDescriptorSnapshot, RxDma, RxDmaBufferAddresses, RxRingError, RxRingHalted, RxRingLive,
    RxRingStopped, RxSegment,
};

use crate::rx::storage::{ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage};

use super::state::Esp32s31RxFrontierState;
use super::{
    Esp32s31RxFrontier, Esp32s31RxFrontierContinuation, Esp32s31RxFrontierDelay,
    Esp32s31RxFrontierDirective, Esp32s31RxFrontierError, Esp32s31RxFrontierIntoLiveFailure,
    Esp32s31RxFrontierPhase, Esp32s31RxFrontierProgress, Esp32s31RxFrontierSchedulerSnapshot,
    Esp32s31RxFrontierServiceProgress,
};

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize>
    Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>
where
    D: Esp32s31RxFrontierDelay,
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<M: RxDma, const DMA_STORAGE_SIZE: usize>(
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err(RxRingError::Size);
        }
        storage
            .prepare_ring(hardware, descriptor_base, buffer_addresses)
            .map(Self::from_prepared)
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<M: RxDma, const DMA_STORAGE_SIZE: usize>(
        hardware: &mut M,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err(RxRingError::Size);
        }
        storage
            .prepare_ring(hardware, descriptor_base, buffer_addresses)
            .map(Self::from_prepared)
    }

    pub const fn from_halted(ring: RxRingHalted<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31RxFrontierState::Halted(ring),
            _delay: PhantomData,
        }
    }

    pub const fn from_prepared(ring: RxRingStopped<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31RxFrontierState::Prepared(ring),
            _delay: PhantomData,
        }
    }

    /// Adopt the continuously running physical frontier of a preceding role.
    pub const fn from_live(ring: RxRingLive<'storage, COUNT>) -> Self {
        Self {
            state: Esp32s31RxFrontierState::Live(ring),
            _delay: PhantomData,
        }
    }

    pub const fn phase(&self) -> Esp32s31RxFrontierPhase {
        self.state.phase()
    }

    /// Prepare a halted ring without starting the DMA walker.
    pub fn prepare_with_storage<M: RxDma, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<(), Esp32s31RxFrontierError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        match state {
            Esp32s31RxFrontierState::Halted(halted) => {
                match storage.prepare_halted(halted, hardware) {
                    Ok(prepared) => {
                        self.state = Esp32s31RxFrontierState::Prepared(prepared);
                        Ok(())
                    }
                    Err((halted, error)) => {
                        self.state = Esp32s31RxFrontierState::Halted(halted);
                        Err(error.into())
                    }
                }
            }
            prepared @ Esp32s31RxFrontierState::Prepared(_) => {
                self.state = prepared;
                Ok(())
            }
            live @ Esp32s31RxFrontierState::Live(_) => {
                self.state = live;
                Err(Esp32s31RxFrontierError::AlreadyStarted)
            }
            Esp32s31RxFrontierState::Vacant => Err(Esp32s31RxFrontierError::OwnerUnavailable),
        }
    }

    /// Publish an already prepared ring after the role owner has satisfied
    /// its executor-specific settle edge.
    pub fn start_prepared<M: RxDma>(
        &mut self,
        hardware: &mut M,
    ) -> Result<(), Esp32s31RxFrontierError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        let Esp32s31RxFrontierState::Prepared(prepared) = state else {
            self.state = state;
            return Err(Esp32s31RxFrontierError::OwnerUnavailable);
        };
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = Esp32s31RxFrontierState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31RxFrontierState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    pub fn prepare_initial_or_retry<M: RxDma, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<(), Esp32s31RxFrontierError> {
        self.prepare_with_storage(hardware, storage)
    }

    /// Move the current frontier into another finite phase while leaving an
    /// explicit vacant placeholder for its eventual returned owner.
    pub fn take(&mut self) -> Result<Self, Esp32s31RxFrontierError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        if matches!(state, Esp32s31RxFrontierState::Vacant) {
            return Err(Esp32s31RxFrontierError::OwnerUnavailable);
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
    ) -> Result<(), Esp32s31RxFrontierError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        let prepared = match state {
            Esp32s31RxFrontierState::Halted(halted) => {
                match halted.prepare(hardware, DMA_BUFFER_SIZE as u32, prepare_buffer) {
                    Ok(prepared) => prepared,
                    Err((halted, error)) => {
                        self.state = Esp32s31RxFrontierState::Halted(halted);
                        return Err(error.into());
                    }
                }
            }
            Esp32s31RxFrontierState::Prepared(prepared) => prepared,
            live @ Esp32s31RxFrontierState::Live(_) => {
                self.state = live;
                return Err(Esp32s31RxFrontierError::AlreadyStarted);
            }
            Esp32s31RxFrontierState::Vacant => {
                return Err(Esp32s31RxFrontierError::OwnerUnavailable);
            }
        };
        D::after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US).await;
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = Esp32s31RxFrontierState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31RxFrontierState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    /// Start RX using the production DMA storage bound to this ring.
    pub async fn start_with_storage<M, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<(), Esp32s31RxFrontierError>
    where
        M: RxDma,
    {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        let prepared = match state {
            Esp32s31RxFrontierState::Halted(halted) => {
                match storage.prepare_halted(halted, hardware) {
                    Ok(prepared) => prepared,
                    Err((halted, error)) => {
                        self.state = Esp32s31RxFrontierState::Halted(halted);
                        return Err(error.into());
                    }
                }
            }
            Esp32s31RxFrontierState::Prepared(prepared) => prepared,
            live @ Esp32s31RxFrontierState::Live(_) => {
                self.state = live;
                return Err(Esp32s31RxFrontierError::AlreadyStarted);
            }
            Esp32s31RxFrontierState::Vacant => {
                return Err(Esp32s31RxFrontierError::OwnerUnavailable);
            }
        };
        D::after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US).await;
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = Esp32s31RxFrontierState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = Esp32s31RxFrontierState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    /// Visit every descriptor in one finite completed frontier and recycle
    /// its contiguous observed prefix.
    pub fn service_completed_frontier<M, F, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        mut observe: F,
    ) -> Result<Esp32s31RxFrontierServiceProgress, Esp32s31RxFrontierError>
    where
        M: RxDma,
        F: FnMut(RxSegment<'_>),
    {
        let ring = self.live_mut()?;
        ring.complete_pending_reload(hardware)?;
        ring.observe_exhausted_republication(hardware);
        let mut progress = Esp32s31RxFrontierServiceProgress {
            reload_pending: ring.reload_pending(),
            ..Esp32s31RxFrontierServiceProgress::default()
        };
        let frontier = ring.completed_descriptor_frontier();
        for step in 0..frontier.descriptor_count {
            let index = wrap_index::<COUNT>(frontier.start_index, step);
            let Some(completed) = storage.take_completed(ring, index)? else {
                return Err(Esp32s31RxFrontierError::Ring(RxRingError::Corrupt));
            };
            progress.completed_descriptors = progress.completed_descriptors.saturating_add(1);
            observe(completed.segment());
        }
        if !progress.reload_pending
            && let Some(append) = storage.recycle_completed_prefix::<COUNT, _>(ring, hardware)?
        {
            progress.recycled_descriptors =
                u32::try_from(append.descriptor_count).unwrap_or(u32::MAX);
        }
        // A current-LAST completion is released by a later RX completion and
        // therefore needs no synthetic wake. Only direct BASE republication
        // can consume its sole IRQ edge and must keep a finite role runnable.
        progress.service_probe_pending = ring.exhausted_republication_probe_pending();
        Ok(progress)
    }

    pub fn stop<M: RxDma>(&mut self, hardware: &mut M) -> Result<(), Esp32s31RxFrontierError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        let Esp32s31RxFrontierState::Live(live) = state else {
            self.state = state;
            return Err(Esp32s31RxFrontierError::OwnerUnavailable);
        };
        match live.try_stop(hardware) {
            Ok(halted) => {
                self.state = Esp32s31RxFrontierState::Halted(halted);
                Ok(())
            }
            Err((live, error)) => {
                self.state = Esp32s31RxFrontierState::Live(live);
                Err(error.into())
            }
        }
    }

    /// Poison a live arena before retaining this owner for board reset.
    ///
    /// This is a terminal quarantine marker, not proof of walker quiescence:
    /// it does not stop DMA or return the descriptor storage for reuse. The
    /// caller must retain the owner until the normal stop/reset boundary.
    /// Other frontier phases remain unchanged.
    pub fn require_reset(&mut self) {
        if let Esp32s31RxFrontierState::Live(ring) = &mut self.state {
            ring.require_reset();
        }
    }

    pub fn prepare_next<M: RxDma, const DMA_STORAGE_SIZE: usize>(
        &mut self,
        hardware: &mut M,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<(), Esp32s31RxFrontierError> {
        self.prepare_with_storage(hardware, storage)
    }

    pub fn live_mut(
        &mut self,
    ) -> Result<&mut RxRingLive<'storage, COUNT>, Esp32s31RxFrontierError> {
        match &mut self.state {
            Esp32s31RxFrontierState::Live(ring) => Ok(ring),
            _ => Err(Esp32s31RxFrontierError::OwnerUnavailable),
        }
    }

    /// Read one descriptor image in any owned, non-vacant RX phase.
    ///
    /// This is observation only: callers receive no descriptor mutation or
    /// DMA-publication authority. It allows lifecycle fault reports to retain
    /// the terminal hardware image after the walker has been stopped.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        match &self.state {
            Esp32s31RxFrontierState::Halted(ring) => ring.descriptor_snapshot(index),
            Esp32s31RxFrontierState::Prepared(ring) => ring.descriptor_snapshot(index),
            Esp32s31RxFrontierState::Live(ring) => ring.descriptor_snapshot(index),
            Esp32s31RxFrontierState::Vacant => None,
        }
    }

    /// Snapshot the live software frontier without exposing publication
    /// authority. AP fault evidence retains this before stop consumes the
    /// live owner and discards scheduler bookkeeping.
    pub fn scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot> {
        let Esp32s31RxFrontierState::Live(ring) = &self.state else {
            return None;
        };
        Some(Esp32s31RxFrontierSchedulerSnapshot {
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
            Esp32s31RxFrontierState::Live(ring) => ring.reload_pending(),
            _ => false,
        }
    }

    /// Select the next RX scheduler edge from the complete live-ring state.
    ///
    /// Complete vendor `datapathProcessRxSucDataAll` processes through its saved
    /// LAST descriptor, refreshes LAST and continues in the same `ppTask`
    /// invocation. A terminal descriptor writeback, a deferred link-release
    /// proof, or an exhausted-list BASE publication therefore cannot require
    /// another RX-success interrupt: none is guaranteed after `NEXT=0`.
    pub fn service_continuation<M: RxDma>(
        &mut self,
        hardware: &mut M,
    ) -> Result<Esp32s31RxFrontierContinuation, Esp32s31RxFrontierError> {
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
            Esp32s31RxFrontierContinuation::ProbePending
        } else {
            Esp32s31RxFrontierContinuation::AwaitInterrupt
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
    ) -> Result<Esp32s31RxFrontierProgress, Esp32s31RxFrontierError>
    where
        M: RxDma,
        F: for<'frame> FnMut(RxSegment<'frame>) -> Esp32s31RxFrontierDirective,
    {
        let ring = self.live_mut()?;
        let mut progress = Esp32s31RxFrontierProgress::default();
        let frontier = ring.completed_descriptor_frontier();
        // This finite port publishes one physical half at a time. Do not mark
        // a later descriptor observed merely because it was needed as the
        // LAST-advance proof for the preceding half; that descriptor belongs
        // to the next service/recycle epoch.
        for step in 0..frontier.descriptor_count.min(COUNT / 2) {
            let index = wrap_index::<COUNT>(frontier.start_index, step);
            let Some(completed) = storage.take_completed(ring, index)? else {
                return Err(Esp32s31RxFrontierError::Ring(RxRingError::Corrupt));
            };
            progress.completed = progress.completed.saturating_add(1);
            match observe(completed.segment()) {
                Esp32s31RxFrontierDirective::Continue => {}
                Esp32s31RxFrontierDirective::Pause => break,
                Esp32s31RxFrontierDirective::Stop => {
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
            return Err(Esp32s31RxFrontierError::Ring(RxRingError::Corrupt));
        }
        Ok(progress)
    }

    pub fn take_live(&mut self) -> Result<RxRingLive<'storage, COUNT>, Esp32s31RxFrontierError> {
        let state = core::mem::replace(&mut self.state, Esp32s31RxFrontierState::Vacant);
        match state {
            Esp32s31RxFrontierState::Live(ring) => Ok(ring),
            state => {
                self.state = state;
                Err(Esp32s31RxFrontierError::OwnerUnavailable)
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
            Esp32s31RxFrontierState::Halted(ring) => Ok(ring),
            Esp32s31RxFrontierState::Prepared(ring) => Ok(ring.into_halted()),
            state => Err(Self {
                state,
                _delay: PhantomData,
            }),
        }
    }

    /// Consume a live frontier without changing hardware state.
    pub fn try_into_live(mut self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        match self.take_live() {
            Ok(ring) => Ok(ring),
            Err(_) => Err(self),
        }
    }

    /// Consume the finite protocol owner and materialize a connected RX epoch.
    ///
    /// A live pre-connected frontier is transferred without touching the
    /// walker, NEXT/LAST or descriptor contents. Only a cold halted/prepared
    /// frontier is started here. This keeps physical descriptor credit
    /// continuous while logical scan/join/WPA2 ownership changes.
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
        Esp32s31RxFrontierIntoLiveFailure<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    >
    where
        M: RxDma,
    {
        if self.phase() == Esp32s31RxFrontierPhase::Live {
            return match self.take_live() {
                Ok(ring) => Ok(ring),
                Err(error) => Err(Esp32s31RxFrontierIntoLiveFailure { owner: self, error }),
            };
        }
        if let Err(error) = self.start_with_storage(hardware, storage).await {
            return Err(Esp32s31RxFrontierIntoLiveFailure { owner: self, error });
        }
        match self.take_live() {
            Ok(ring) => Ok(ring),
            Err(error) => Err(Esp32s31RxFrontierIntoLiveFailure { owner: self, error }),
        }
    }
}

fn wrap_index<const COUNT: usize>(index: usize, amount: usize) -> usize {
    debug_assert!(COUNT != 0);
    debug_assert!(index < COUNT);
    debug_assert!(amount <= COUNT);
    let sum = index + amount;
    if sum >= COUNT { sum - COUNT } else { sum }
}
