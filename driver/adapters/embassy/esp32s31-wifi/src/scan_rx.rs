//! Concrete ESP32-S31 station-scan RX ownership and DMA composition.
//!
//! Cold scan and running rescan use different surrounding radio owners, but
//! both carry the same typed RX ring through prepared, live and halted phases.
//! This module owns that DMA lifecycle and management-frame observation only;
//! scan policy and active-probe TX live in their respective modules.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxIngressConfig, RxReloadObservation, RxRingError, RxRingHalted, RxRingLive,
    RxRingStopped, extract_management,
};
use open_esp_radio_ieee80211::scan::{ScanObservation, ScanTable};

use crate::{
    embassy_rx::RxReloadDelay,
    rx_dma_service::{
        ESP32S31_RX_WALKER_ENABLE_SETTLE_US, Esp32s31RxDmaStorage, Esp32s31RxEpochResources,
        Esp32s31StoppedRx,
    },
};

/// Hardware state held by [`Esp32s31ScanRx`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanRxPhase {
    Prepared,
    Live,
    Halted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ScanRxError {
    InvalidPhase {
        expected: Esp32s31ScanRxPhase,
        actual: Esp32s31ScanRxPhase,
    },
    Ring(RxRingError),
}

impl From<RxRingError> for Esp32s31ScanRxError {
    fn from(error: RxRingError) -> Self {
        Self::Ring(error)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31ScanRxProgress {
    pub completed_descriptors: u32,
    pub parsed_management_frames: u32,
    pub inserted_records: u32,
    pub updated_records: u32,
    pub malformed_or_irrelevant_frames: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
}

/// Optional observer for successfully extracted scan frames.
///
/// The frame borrow ends before any descriptor can be recycled. Production
/// policy should normally use the owned `ScanTable`; this hook exists for
/// qualification counters and diagnostics which must inspect the addressed
/// Probe Response without retaining DMA-backed memory.
pub trait Esp32s31ScanFrameObserver {
    fn observe(&mut self, frame: &[u8], rssi: i8, table_outcome: ScanObservation);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEsp32s31ScanFrameObserver;

impl Esp32s31ScanFrameObserver for NoopEsp32s31ScanFrameObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

/// One channel's borrowed observation destinations.
///
/// Bundling them prevents the RX hot path from growing another positional
/// argument list as scan telemetry evolves.
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

    /// Publish one already-extracted management frame through the owned scan
    /// table and its non-retaining observer.
    ///
    /// Concrete RX ports use this boundary when frame extraction is provided
    /// by another typed DMA owner. The frame borrow is never retained beyond
    /// this call.
    pub fn observe_management_frame(&mut self, frame: &[u8], rssi: i8) -> ScanObservation
    where
        O: Esp32s31ScanFrameObserver,
    {
        let outcome = self.table.observe_management(frame, self.channel, rssi);
        self.observer.observe(frame, rssi, outcome);
        outcome
    }
}

enum Esp32s31ScanRxState<'storage, const COUNT: usize> {
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Halted(RxRingHalted<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> Esp32s31ScanRxState<'_, COUNT> {
    const fn phase(&self) -> Esp32s31ScanRxPhase {
        match self {
            Self::Prepared(_) => Esp32s31ScanRxPhase::Prepared,
            Self::Live(_) => Esp32s31ScanRxPhase::Live,
            Self::Halted(_) => Esp32s31ScanRxPhase::Halted,
            Self::Vacant => unreachable!(),
        }
    }
}

/// Production RX-ring owner shared by cold scan and running rescan.
///
/// Unlike the former HIL mask, this value carries the MAC crate's unique ring
/// capability across `Prepared -> Live -> Halted`. A completed scan can hand
/// the exact halted ring to Authentication; no later phase needs to recreate
/// descriptor authority from addresses.
pub struct Esp32s31ScanRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    state: Esp32s31ScanRxState<'storage, COUNT>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

mod ring;
mod running;

pub use running::Esp32s31RunningScanRx;

#[cfg(test)]
mod tests;
