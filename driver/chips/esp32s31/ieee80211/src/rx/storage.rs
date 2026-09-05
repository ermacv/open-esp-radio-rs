//! Qualified storage geometry and aliases over the affine DMA arena.

use open_esp_radio_esp32s31_wifi_dma::rx_storage::{RxDmaBuffer, RxDmaStorage};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 96;
pub const ESP32S31_RX_BUFFER_SIZE: usize = 4_608;
pub const ESP32S31_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_SIZE + 4;
/// Platform settle edge between stopped-ring publication and walker enable.
pub const ESP32S31_RX_WALKER_ENABLE_SETTLE_US: u32 = 5;

/// Qualified large-RX profile aliases over the executor-independent MAC arena.
pub type Esp32s31RxDmaBuffer<
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>;

pub type Esp32s31RxDmaStorage<
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>;
