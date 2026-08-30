#![no_std]
#![deny(unsafe_code)]

//! Semantic ownership boundary for ESP32-S31 platform registers.
//!
//! Radio registers belong to `open-esp-radio-esp32s31-pac`. This crate owns
//! the official ESP-HAL singleton witnesses for non-radio platform blocks used
//! by the HIL and is the only handwritten layer allowed to touch their
//! generated register accessors.

#[cfg(any(test, feature = "axi-gdma-mem2mem"))]
mod axi_gdma_mem2mem;
#[cfg(feature = "axi-gdma-mem2mem")]
mod cache_maintenance;
mod cache_performance;
#[cfg(feature = "esp32s31")]
mod flash_mmu;

#[cfg(feature = "axi-gdma-mem2mem")]
pub use axi_gdma_mem2mem::{
    AxiGdmaDescriptor, AxiGdmaMem2Mem, AxiGdmaMem2MemError, AxiGdmaMem2MemPrepared,
    AxiGdmaMem2MemReport, AxiGdmaMem2MemTransfer, AxiGdmaMem2MemTransferError, BurstSize,
};
#[cfg(feature = "axi-gdma-mem2mem")]
pub use cache_maintenance::{PsramCacheWritebackError, writeback_psram_for_dma_read};
#[cfg(feature = "esp32s31")]
pub use cache_performance::L1CachePerformanceCounters;
pub use cache_performance::{L1CacheBusSnapshot, L1CacheCounterEnable, L1CachePerformanceSnapshot};
#[cfg(feature = "esp32s31")]
pub use flash_mmu::{FLASH_XIP_END, FLASH_XIP_START, FlashMmu};
