#![no_std]

//! Qualified electrical and memory facts for the ESP32-S31 HIL board.
//!
//! These constants are intentionally private test-platform policy. They are
//! not ESP32-S31 capabilities and must not leak into a public driver crate.

use esp_hal::{
    peripherals::PSRAM,
    psram::{Psram, PsramConfig, PsramSize, PsramTiming},
};

pub const BOARD_NAME: &str = "ESP32-S31-Function-CoreBoard-1";
pub const PSRAM_SIZE_BYTES: usize = 16 * 1024 * 1024;
pub const PSRAM_CLOCK_MHZ: u32 = 250;
pub const PSRAM_DATA_LINES: u8 = 8;
pub const FLASH_CLOCK_MHZ: u32 = 120;
pub const FLASH_SIZE_BYTES: usize = 16 * 1024 * 1024;
pub const FLASH_MMU_PAGE_SIZE_BYTES: u32 = 64 * 1024;
pub const FLASH_DATA_LINES: u8 = 4;

pub const fn psram_config(cache_already_initialized: bool) -> PsramConfig {
    PsramConfig {
        size: PsramSize::AutoDetect,
        cache_already_initialized,
        timing: PsramTiming::MHZ_250,
    }
}

pub fn initialize_psram(peripheral: PSRAM<'static>, cache_already_initialized: bool) -> Psram {
    Psram::new(peripheral, psram_config(cache_already_initialized))
}

pub fn has_expected_psram_capacity(psram: &Psram) -> bool {
    psram.raw_parts().1 == PSRAM_SIZE_BYTES
}
