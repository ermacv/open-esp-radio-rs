#![expect(
    clippy::result_large_err,
    reason = "no-alloc scan shutdown returns the complete RX frontier"
)]

//! Scan-specific view over the role-neutral ESP32-S31 RX-ring owner.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxDmaBufferAddresses, RxIngressConfig, RxRingError, RxRingHalted, RxRingLive,
    extract_management,
};
use open_esp_radio_ieee80211::scan::{ScanObservation, ScanTable};

use crate::{
    datapath::rx::dma::{
        ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage, Esp32s31RxEpochResources,
        Esp32s31StagedRxProducer,
    },
    datapath::rx::frontier::{
        EmbassyEsp32s31RxFrontierDelay, Esp32s31RxFrontier, Esp32s31RxFrontierError,
        Esp32s31RxFrontierPhase,
    },
    datapath::rx::hardware::RxDmaObservationDelay,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanRxProgress {
    pub completed_descriptors: u32,
    pub parsed_management_frames: u32,
    pub inserted_records: u32,
    pub updated_records: u32,
    pub malformed_or_irrelevant_frames: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

/// Optional observer for successfully extracted scan frames.
pub trait Esp32s31ScanFrameObserver {
    fn observe(&mut self, frame: &[u8], rssi: i8, table_outcome: ScanObservation);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEsp32s31ScanFrameObserver;

impl Esp32s31ScanFrameObserver for NoopEsp32s31ScanFrameObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

pub struct Esp32s31ScanObservationContext<'a, O, const RECORDS: usize> {
    channel: u8,
    frame: &'a mut [u8],
    table: &'a mut ScanTable<RECORDS>,
    observer: &'a mut O,
}

impl<'a, O, const RECORDS: usize> Esp32s31ScanObservationContext<'a, O, RECORDS> {
    pub fn new(
        channel: u8,
        frame: &'a mut [u8],
        table: &'a mut ScanTable<RECORDS>,
        observer: &'a mut O,
    ) -> Self {
        Self {
            channel,
            frame,
            table,
            observer,
        }
    }

    pub fn observe_management_frame(&mut self, frame: &[u8], rssi: i8) -> ScanObservation
    where
        O: Esp32s31ScanFrameObserver,
    {
        let outcome = self.table.observe_management(frame, self.channel, rssi);
        self.observer.observe(frame, rssi, outcome);
        outcome
    }
}

/// Scan policy bound to one role-neutral RX-ring owner.
pub struct Esp32s31ScanRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    receive: Esp32s31RxFrontier<'storage, EmbassyEsp32s31RxFrontierDelay, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        Esp32s31RxFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)
            .map(|receive| Self { receive, storage })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        Esp32s31RxFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)
            .map(|receive| Self { receive, storage })
    }

    pub const fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            receive: Esp32s31RxFrontier::from_halted(ring),
            storage,
        }
    }

    pub const fn from_live(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            receive: Esp32s31RxFrontier::from_live(ring),
            storage,
        }
    }

    pub const fn phase(&self) -> Esp32s31RxFrontierPhase {
        self.receive.phase()
    }

    pub fn prepare_initial_or_retry<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxFrontierError> {
        if self.phase() == Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive
                .prepare_initial_or_retry(hardware, self.storage)
        }
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxFrontierError> {
        if self.phase() == Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive.start_prepared(hardware)
        }
    }

    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31RxFrontierError>
    where
        H: RxDma,
        O: Esp32s31ScanFrameObserver,
    {
        let mut progress = Esp32s31ScanRxProgress::default();
        let ring = self
            .receive
            .service_completed_frontier(hardware, self.storage, |segment| {
                let rssi = segment.buffer[0] as i8;
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
                        let outcome =
                            context
                                .table
                                .observe_management(frame, context.channel, rssi);
                        context.observer.observe(frame, rssi, outcome);
                        match outcome {
                            ScanObservation::Inserted { .. } => {
                                progress.inserted_records =
                                    progress.inserted_records.saturating_add(1)
                            }
                            ScanObservation::Updated { .. } => {
                                progress.updated_records =
                                    progress.updated_records.saturating_add(1)
                            }
                            _ => {}
                        }
                    }
                    Err(_) => {
                        progress.malformed_or_irrelevant_frames =
                            progress.malformed_or_irrelevant_frames.saturating_add(1);
                    }
                }
            })?;
        progress.completed_descriptors = ring.completed_descriptors;
        progress.recycled_descriptors = ring.recycled_descriptors;
        progress.reload_pending = ring.reload_pending;
        progress.service_probe_pending = ring.service_probe_pending;
        Ok(progress)
    }

    pub fn park(&self) -> Result<(), Esp32s31RxFrontierError> {
        if self.phase() == Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            Err(Esp32s31RxFrontierError::OwnerUnavailable)
        }
    }

    pub fn prepare_next<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxFrontierError> {
        if self.phase() == Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive.prepare_next(hardware, self.storage)
        }
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { receive, storage } = self;
        receive
            .try_into_halted()
            .map_err(|receive| Self { receive, storage })
    }

    pub fn into_live(self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        let Self {
            mut receive,
            storage,
        } = self;
        receive.take_live().map_err(|_| Self { receive, storage })
    }
}

mod running;
pub use running::Esp32s31RunningScanRx;

#[cfg(test)]
mod tests;
