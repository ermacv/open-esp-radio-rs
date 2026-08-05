use super::*;
impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    /// Prepare the first cold scan epoch under the caller's unique PAC owner.
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
            state: Esp32s31ScanRxState::Prepared(ring),
            storage,
        })
    }

    /// Prepare the first hardware scan epoch from permanently allocated DMA
    /// storage. Losing a later live-ring owner cannot deallocate this arena.
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
            state: Esp32s31ScanRxState::Prepared(ring),
            storage,
        })
    }

    /// Reuse a hardware-confirmed halted ring for a running rescan.
    pub const fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            state: Esp32s31ScanRxState::Halted(ring),
            storage,
        }
    }

    pub const fn phase(&self) -> Esp32s31ScanRxPhase {
        self.state.phase()
    }

    /// Admit either the first already-prepared cold scan or a later complete
    /// retry whose final channel returned the same ring halted.
    ///
    /// This is deliberately not an implicit live-ring restart: a caller that
    /// still owns a live scan epoch has violated the finite scan boundary and
    /// receives the exact phase error.
    pub fn prepare_initial_or_retry<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31ScanRxError> {
        match self.phase() {
            Esp32s31ScanRxPhase::Prepared => Ok(()),
            Esp32s31ScanRxPhase::Halted => self.prepare_next(hardware),
            actual @ Esp32s31ScanRxPhase::Live => Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Prepared,
                actual,
            }),
        }
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31ScanRxState::Vacant);
        let Esp32s31ScanRxState::Prepared(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Prepared,
                actual,
            });
        };
        match ring.try_start(hardware) {
            Ok(ring) => {
                self.state = Esp32s31ScanRxState::Live(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31ScanRxState::Prepared(ring);
                Err(error.into())
            }
        }
    }

    /// Drain the current completion frontier, copy scan frames into bounded
    /// caller storage and promptly recycle the contiguous observed prefix.
    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31ScanRxError>
    where
        H: RxDma,
        O: Esp32s31ScanFrameObserver,
    {
        let actual = self.state.phase();
        let Esp32s31ScanRxState::Live(ring) = &mut self.state else {
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Live,
                actual,
            });
        };
        let mut progress = Esp32s31ScanRxProgress::default();
        progress.reload_pending =
            ring.poll_pending_reload(hardware)? == RxReloadObservation::Pending;

        for index in 0..COUNT {
            let Some(completed) = self.storage.take_completed(ring, index)? else {
                continue;
            };
            progress.completed_descriptors = progress.completed_descriptors.saturating_add(1);
            let segment = completed.segment();
            let buffer = segment.buffer;
            let rssi = buffer[0] as i8;
            match extract_management(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                context.frame,
            ) {
                Ok(frame) => {
                    progress.parsed_management_frames =
                        progress.parsed_management_frames.saturating_add(1);
                    let frame = &context.frame[..frame.length];
                    let table_outcome =
                        context
                            .table
                            .observe_management(frame, context.channel, rssi);
                    match table_outcome {
                        ScanObservation::Inserted { .. } => {
                            progress.inserted_records = progress.inserted_records.saturating_add(1)
                        }
                        ScanObservation::Updated { .. } => {
                            progress.updated_records = progress.updated_records.saturating_add(1)
                        }
                        _ => {}
                    }
                    context.observer.observe(frame, rssi, table_outcome);
                }
                Err(_) => {
                    progress.malformed_or_irrelevant_frames =
                        progress.malformed_or_irrelevant_frames.saturating_add(1);
                }
            }
        }

        if !progress.reload_pending
            && let Some(append) = self
                .storage
                .recycle_completed_prefix::<COUNT, _>(ring, hardware)?
        {
            progress.recycled_descriptors = append.descriptor_count as u32;
        }
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31ScanRxState::Vacant);
        let Esp32s31ScanRxState::Live(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Live,
                actual,
            });
        };
        match ring.try_stop(hardware) {
            Ok(ring) => {
                self.state = Esp32s31ScanRxState::Halted(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31ScanRxState::Live(ring);
                Err(error.into())
            }
        }
    }

    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        let state = core::mem::replace(&mut self.state, Esp32s31ScanRxState::Vacant);
        let Esp32s31ScanRxState::Halted(ring) = state else {
            let actual = state.phase();
            self.state = state;
            return Err(Esp32s31ScanRxError::InvalidPhase {
                expected: Esp32s31ScanRxPhase::Halted,
                actual,
            });
        };
        match self.storage.prepare_halted(ring, hardware) {
            Ok(ring) => {
                self.state = Esp32s31ScanRxState::Prepared(ring);
                Ok(())
            }
            Err((ring, error)) => {
                self.state = Esp32s31ScanRxState::Halted(ring);
                Err(error.into())
            }
        }
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        match self.state {
            Esp32s31ScanRxState::Halted(ring) => Ok(ring),
            Esp32s31ScanRxState::Prepared(ring) => Ok(ring.into_halted()),
            state => Err(Self {
                state,
                storage: self.storage,
            }),
        }
    }
}
