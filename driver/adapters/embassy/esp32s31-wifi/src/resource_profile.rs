//! Named static-memory sizing for the ESP32-S31 Wi-Fi composition.
//!
//! These values are integration policy, not hardware capabilities. There is
//! one production profile: qualification may observe it, but must never
//! select a different queue, aggregation, or reorder topology. CPU-only state
//! belongs in initialized PSRAM while DMA-visible and latency-critical state
//! remains in SRAM.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_dma::tx_storage::TxDmaStorage;
use open_esp_radio_esp32s31_wifi_mac::rx::RX_GUARDED_RECLAIM_WINDOW_DESCRIPTORS;
use open_esp_radio_ieee80211::{beacon::WPA2_BEACON_CAPACITY, scan::ScanTable};
use static_cell::{ConstStaticCell, StaticCell};

use crate::rx_dma_service::Esp32s31RxDmaStorage;
use crate::station::Esp32s31StationControlResources;

pub const ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT: usize = 64;
pub const ESP32S31_DEFAULT_RX_BUFFER_SIZE: usize = 1_700;
pub const ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_DEFAULT_RX_BUFFER_SIZE + 4;
pub const ESP32S31_DEFAULT_TX_BUFFER_SIZE: usize = 1_700;
pub const ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY: usize = 1_700;
pub const ESP32S31_DEFAULT_SCAN_RECORD_CAPACITY: usize = 32;

pub const ESP32S31_DEFAULT_RX_STAGE_CAPACITY: usize = 1_700;
// These slots form the latency-critical copy-before-DMA-reload working set and
// stay in SRAM. Guarded reclaim needs a 32-descriptor completed span: 16 copied
// and returned, followed by 16 retained. The retained half remains unobserved
// in the DMA ring and consumes no staging lease, leaving another 16 slots of
// downstream elasticity in this 32-slot SRAM working set.
pub const ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT: usize = 32;
// Keep one complete negotiated HT BlockAck window of DMA descriptors in
// reserve while software copies and republishes the preceding window. A
// 32-entry agreement consumed the entire 32-descriptor frontier and exposed
// ordinary executor latency as RX BUFFER_FULL under sustained HT40 traffic.
pub const ESP32S31_DEFAULT_RX_REORDER_WINDOW: usize = 16;
pub const ESP32S31_DEFAULT_CONTROL_QUEUE_DEPTH: usize = 16;

pub const ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY: usize = 1_600;
// The CPU-only queue retains two complete 32-frame bursts and belongs in
// PSRAM. This prevents the network consumer from extending ownership of the
// latency-critical SRAM staging pool.
pub const ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH: usize = 64;
// One additional credit is reserved by the network adapter for the TX token
// paired with ingress. Application egress therefore retains two complete
// 32-MPDU arenas even while saturated. TX frame backing is DMA-visible SRAM.
pub const ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH: usize = 65;
pub const ESP32S31_DEFAULT_NETWORK_TX_TRAILER: usize = 12;
pub const ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT: usize = 32;

/// Production marker exposed to board composition and memory reporting.
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

/// One statically placed default Wi-Fi memory arena.
///
/// Keeping the unique DMA buffers and role scratch inside this aggregate
/// makes both placement and single ownership visible at the application boundary.
/// A board declares one `static` value instead of independently taking a set
/// of cells which could otherwise be only partially acquired.
pub struct Esp32s31DefaultWifiMemory<M: RawMutex> {
    claimed: AtomicBool,
    rx_dma: ConstStaticCell<Esp32s31DefaultRxDmaStorage>,
    rx_buffer_addresses: ConstStaticCell<[u32; ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT]>,
    tx_dma: ConstStaticCell<TxDmaStorage<ESP32S31_DEFAULT_TX_BUFFER_SIZE>>,
    scan_table: StaticCell<Esp32s31DefaultScanTable>,
    ap_beacon: ConstStaticCell<[u8; WPA2_BEACON_CAPACITY]>,
    scan_frame: ConstStaticCell<[u8; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY]>,
    station_control: ConstStaticCell<Esp32s31StationControlResources<M>>,
}

impl<M: RawMutex> Esp32s31DefaultWifiMemory<M> {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            rx_dma: ConstStaticCell::new(Esp32s31DefaultRxDmaStorage::new()),
            rx_buffer_addresses: ConstStaticCell::new([0; ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT]),
            tx_dma: ConstStaticCell::new(TxDmaStorage::new()),
            scan_table: StaticCell::new(),
            ap_beacon: ConstStaticCell::new([0; WPA2_BEACON_CAPACITY]),
            scan_frame: ConstStaticCell::new([0; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY]),
            station_control: ConstStaticCell::new(Esp32s31StationControlResources::new()),
        }
    }

    /// Atomically acquire the complete initial station arena.
    pub fn claim(
        &'static self,
    ) -> Result<Esp32s31DefaultWifiMemoryLease<M>, Esp32s31DefaultWifiMemoryError> {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Esp32s31DefaultWifiMemoryError::InUse);
        }
        Ok(Esp32s31DefaultWifiMemoryLease {
            rx_dma: self.rx_dma.take(),
            rx_buffer_addresses: self.rx_buffer_addresses.take(),
            tx_dma: self.tx_dma.take(),
            scan_table: self.scan_table.init_with(ScanTable::new),
            ap_beacon: self.ap_beacon.take(),
            scan_frame: self.scan_frame.take(),
            station_control: self.station_control.take(),
        })
    }
}

impl<M: RawMutex> Default for Esp32s31DefaultWifiMemory<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact owners acquired from one default station arena.
pub struct Esp32s31DefaultWifiMemoryLease<M: RawMutex + 'static> {
    pub rx_dma: &'static mut Esp32s31DefaultRxDmaStorage,
    pub rx_buffer_addresses: &'static mut [u32; ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT],
    pub tx_dma: &'static mut TxDmaStorage<ESP32S31_DEFAULT_TX_BUFFER_SIZE>,
    pub scan_table: &'static mut Esp32s31DefaultScanTable,
    pub ap_beacon: &'static mut [u8; WPA2_BEACON_CAPACITY],
    pub scan_frame: &'static mut [u8; ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY],
    pub station_control: &'static Esp32s31StationControlResources<M>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31DefaultWifiMemoryError {
    InUse,
}

const _: () =
    assert!(ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY <= ESP32S31_DEFAULT_RX_STAGE_CAPACITY);
const _: () = assert!(ESP32S31_DEFAULT_RX_REORDER_WINDOW <= ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT);
const _: () =
    assert!(ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT >= 2 * ESP32S31_DEFAULT_RX_REORDER_WINDOW);
const _: () =
    assert!(ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT >= RX_GUARDED_RECLAIM_WINDOW_DESCRIPTORS);
const _: () =
    assert!(ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT >= RX_GUARDED_RECLAIM_WINDOW_DESCRIPTORS);
const _: () =
    assert!(ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT < ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH);

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
        assert_eq!(Esp32s31DefaultWifiResourceProfile::RX_REORDER_WINDOW, 16);
        assert_eq!(Esp32s31DefaultWifiResourceProfile::RX_STAGE_SLOT_COUNT, 32);
    }

    #[test]
    fn default_station_memory_is_acquired_as_one_owner_graph() {
        static MEMORY: Esp32s31DefaultWifiMemory<NoopRawMutex> = Esp32s31DefaultWifiMemory::new();

        let lease = MEMORY.claim().expect("fresh station memory is available");
        assert_eq!(
            lease.rx_buffer_addresses.len(),
            ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
        );
        assert_eq!(lease.scan_frame.len(), ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY);
        assert_eq!(lease.ap_beacon.len(), WPA2_BEACON_CAPACITY);
        assert!(matches!(
            MEMORY.claim(),
            Err(Esp32s31DefaultWifiMemoryError::InUse)
        ));
    }
}
