//! Named static-memory sizing for the ESP32-S31 Wi-Fi composition.
//!
//! These values are integration policy, not hardware capabilities. The
//! default compact profile fits a direct-to-flash application in internal
//! SRAM. `high-throughput` selects the larger envelope used by qualification;
//! that profile requires a product linker which places CPU-only state in
//! initialized PSRAM while retaining DMA and latency-critical state in SRAM.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_dma::tx_storage::TxDmaStorage;
use open_esp_radio_ieee80211::scan::ScanTable;
use static_cell::{ConstStaticCell, StaticCell};

use crate::rx_dma_service::Esp32s31RxDmaStorage;
use crate::station::Esp32s31StationControlResources;

pub const ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT: usize = 32;
pub const ESP32S31_DEFAULT_RX_BUFFER_SIZE: usize = 1_700;
pub const ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_DEFAULT_RX_BUFFER_SIZE + 4;
pub const ESP32S31_DEFAULT_TX_BUFFER_SIZE: usize = 1_700;
pub const ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY: usize = 1_700;
pub const ESP32S31_DEFAULT_SCAN_RECORD_CAPACITY: usize = 32;

pub const ESP32S31_DEFAULT_RX_STAGE_CAPACITY: usize = 1_700;
// These slots form the latency-critical copy-before-DMA-reload working set and
// stay in SRAM. The high-throughput profile can retain one 32-descriptor burst
// while earlier frames remain behind a BlockAck gap; the compact profile
// deliberately accepts less burst elasticity.
#[cfg(not(feature = "high-throughput"))]
pub const ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT: usize = 16;
#[cfg(feature = "high-throughput")]
pub const ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT: usize = 64;
pub const ESP32S31_DEFAULT_RX_REORDER_WINDOW: usize = 8;
pub const ESP32S31_DEFAULT_CONTROL_QUEUE_DEPTH: usize = 16;

pub const ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY: usize = 1_600;
// The high-throughput queue retains a complete 32-frame RX burst plus overlap
// with the network consumer. Its CPU-only bytes belong in PSRAM. The compact
// queue trades burst elasticity for a direct-to-flash SRAM footprint.
#[cfg(not(feature = "high-throughput"))]
pub const ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH: usize = 8;
#[cfg(feature = "high-throughput")]
pub const ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH: usize = 40;
// The high-throughput profile represents a complete 32-member TX A-MPDU.
// Compact builds retain eight members. TX frame backing is DMA-visible SRAM.
#[cfg(not(feature = "high-throughput"))]
pub const ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH: usize = 8;
#[cfg(feature = "high-throughput")]
pub const ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH: usize = 32;
pub const ESP32S31_DEFAULT_NETWORK_TX_TRAILER: usize = 12;
#[cfg(not(feature = "high-throughput"))]
pub const ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT: usize = 8;
#[cfg(feature = "high-throughput")]
pub const ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT: usize = 32;

/// Compact marker exposed to board composition and memory reporting.
pub struct Esp32s31DefaultWifiResourceProfile;

impl Esp32s31DefaultWifiResourceProfile {
    pub const RX_DESCRIPTOR_COUNT: usize = ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT;
    pub const RX_BUFFER_SIZE: usize = ESP32S31_DEFAULT_RX_BUFFER_SIZE;
    pub const TX_BUFFER_SIZE: usize = ESP32S31_DEFAULT_TX_BUFFER_SIZE;
    pub const RX_STAGE_SLOT_COUNT: usize = ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT;
    pub const RX_REORDER_WINDOW: usize = ESP32S31_DEFAULT_RX_REORDER_WINDOW;
    pub const NETWORK_RX_QUEUE_DEPTH: usize = ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH;
    pub const NETWORK_TX_QUEUE_DEPTH: usize = ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH;
    pub const TX_AMPDU_FRAME_COUNT: usize = ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT;
}

/// Default statically placed RX DMA arena used by a normal station/monitor
/// composition.
pub type Esp32s31DefaultRxDmaStorage = Esp32s31RxDmaStorage<
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT,
    ESP32S31_DEFAULT_RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE,
>;

pub type Esp32s31DefaultScanTable = ScanTable<ESP32S31_DEFAULT_SCAN_RECORD_CAPACITY>;

/// One statically placed default station memory arena.
///
/// Keeping the large DMA buffers and scan scratch inside this aggregate makes
/// both placement and single ownership visible at the application boundary.
/// A board declares one `static` value instead of independently taking a set
/// of cells which could otherwise be only partially acquired.
pub struct Esp32s31DefaultStationMemory<M: RawMutex> {
    claimed: AtomicBool,
    rx_dma: ConstStaticCell<Esp32s31DefaultRxDmaStorage>,
    rx_buffer_addresses: ConstStaticCell<[u32; ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT]>,
    tx_dma: ConstStaticCell<TxDmaStorage<ESP32S31_DEFAULT_TX_BUFFER_SIZE>>,
    scan_table: StaticCell<Esp32s31DefaultScanTable>,
    scan_frame: ConstStaticCell<[u8; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY]>,
    station_control: ConstStaticCell<Esp32s31StationControlResources<M>>,
}

impl<M: RawMutex> Esp32s31DefaultStationMemory<M> {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            rx_dma: ConstStaticCell::new(Esp32s31DefaultRxDmaStorage::new()),
            rx_buffer_addresses: ConstStaticCell::new([0; ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT]),
            tx_dma: ConstStaticCell::new(TxDmaStorage::new()),
            scan_table: StaticCell::new(),
            scan_frame: ConstStaticCell::new([0; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY]),
            station_control: ConstStaticCell::new(Esp32s31StationControlResources::new()),
        }
    }

    /// Atomically acquire the complete initial station arena.
    pub fn claim(
        &'static self,
    ) -> Result<Esp32s31DefaultStationMemoryLease<M>, Esp32s31DefaultStationMemoryError> {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31DefaultStationMemoryError::InUse);
        }
        Ok(Esp32s31DefaultStationMemoryLease {
            rx_dma: self.rx_dma.take(),
            rx_buffer_addresses: self.rx_buffer_addresses.take(),
            tx_dma: self.tx_dma.take(),
            scan_table: self.scan_table.init_with(ScanTable::new),
            scan_frame: self.scan_frame.take(),
            station_control: self.station_control.take(),
        })
    }
}

impl<M: RawMutex> Default for Esp32s31DefaultStationMemory<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact owners acquired from one default station arena.
pub struct Esp32s31DefaultStationMemoryLease<M: RawMutex + 'static> {
    pub rx_dma: &'static mut Esp32s31DefaultRxDmaStorage,
    pub rx_buffer_addresses: &'static mut [u32; ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT],
    pub tx_dma: &'static mut TxDmaStorage<ESP32S31_DEFAULT_TX_BUFFER_SIZE>,
    pub scan_table: &'static mut Esp32s31DefaultScanTable,
    pub scan_frame: &'static mut [u8; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY],
    pub station_control: &'static Esp32s31StationControlResources<M>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31DefaultStationMemoryError {
    InUse,
}

const _: () =
    assert!(ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY <= ESP32S31_DEFAULT_RX_STAGE_CAPACITY);
const _: () = assert!(ESP32S31_DEFAULT_RX_REORDER_WINDOW <= ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT);
const _: () =
    assert!(ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT <= ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH);

#[cfg(test)]
mod tests {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;

    #[test]
    fn marker_and_public_dimensions_cannot_drift() {
        assert_eq!(
            Esp32s31DefaultWifiResourceProfile::RX_DESCRIPTOR_COUNT,
            ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
        );
        assert_eq!(
            Esp32s31DefaultWifiResourceProfile::TX_AMPDU_FRAME_COUNT,
            ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT
        );
    }

    #[test]
    fn default_station_memory_is_acquired_as_one_owner_graph() {
        static MEMORY: Esp32s31DefaultStationMemory<NoopRawMutex> =
            Esp32s31DefaultStationMemory::new();

        let lease = MEMORY.claim().expect("fresh station memory is available");
        assert_eq!(
            lease.rx_buffer_addresses.len(),
            ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
        );
        assert_eq!(lease.scan_frame.len(), ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY);
        assert!(matches!(
            MEMORY.claim(),
            Err(Esp32s31DefaultStationMemoryError::InUse)
        ));
    }
}
