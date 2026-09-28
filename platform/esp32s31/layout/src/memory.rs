//! Address map of the staged boot.
//!
//! Sizes describe this board's fitted memories and the staged-boot contract;
//! they are not universal ESP32-S31 capabilities.

/// A contiguous address range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub origin: u32,
    pub length: u32,
}

impl Region {
    pub const fn new(origin: u32, length: u32) -> Self {
        Self { origin, length }
    }

    /// First address past the region.
    pub const fn end(self) -> u32 {
        self.origin + self.length
    }

    /// The remainder of this region after its first `offset` bytes.
    pub const fn after(self, offset: u32) -> Self {
        assert!(offset <= self.length);
        Self::new(self.origin + offset, self.length - offset)
    }

    /// Whether `start..end` is a (possibly empty) range inside this region.
    pub const fn contains_range(self, start: u64, end: u64) -> bool {
        start >= self.origin as u64 && end >= start && end <= self.end() as u64
    }
}

/// Application-owned internal SRAM, `SRAM_START..SRAM_SEG_END` of ESP-IDF's
/// application memory map; the ROM boot stack side lies above it.
///
/// SOURCE: ESP-IDF 4d59230d `components/esp_system/ld/esp32s31/ld.hp_mem_defs:9`
/// (`SRAM_SEG_END 0x2F07AFC0`) and `memory.ld.in:21-26`
/// (`SRAM_SIZE = SRAM_SEG_END - SRAM_START`, without the ULP HP-memory reserve).
pub const SRAM: Region = Region::new(0x2f00_0000, 0x0007_afc0);

/// Start of the second-stage loader's `iram_loader_seg`. The loader runs from
/// here up to its usable end while it loads the bootstrap image, so every
/// segment of that image it loads must end below this address; memory it does
/// not load (BSS, the stack and everything stage two copies) may overlap it.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/bootloader/subproject/main/ld/esp32s31/bootloader.memory.ld.in:39-64`
/// (usable end 0x2f07afb0 minus the 0x2000 stack, 0x5000 `dram_seg` and 0x7000
/// `iram_loader_seg`; asserted as `bootloader_iram_loader_seg_start`).
pub const SECOND_STAGE_LOADER_START: u32 = 0x2f06_cfb0;

/// The unified 64-MiB Flash XIP instruction/data window.
pub const FLASH_XIP: Region = Region::new(0x4000_0000, 0x0400_0000);

/// Cached external RAM aperture of the fitted 16-MiB PSRAM.
pub const PSRAM: Region = Region::new(0x5000_0000, 0x0100_0000);

/// ESP image segment header preceding the bootstrap's Flash-mapped text.
pub const BOOTSTRAP_FLASH_TEXT: Region = FLASH_XIP.after(0x20);

/// PSRAM page the bootstrap probes before it copies stage two; stage two
/// starts on the next 64-KiB MMU page.
pub const BOOTSTRAP_PSRAM_BYTES: u32 = 0x1_0000;

/// Stage-two code relocated to PSRAM, following the bootstrap page.
pub const RUNTIME_PSRAM: Region = PSRAM.after(BOOTSTRAP_PSRAM_BYTES);

/// Stage two's CPU0 task stack in PSRAM.
pub const CPU0_PSRAM_TASK_STACK_BYTES: u32 = 0x3_0000;

/// Each hart's dedicated SRAM interrupt stack; task stacks live in PSRAM.
pub const IRQ_STACK_BYTES: u32 = 0x8000;

// Every derived region stays inside its memory and the runtime follows the
// bootstrap's PSRAM probe page.
const _: () = {
    let nested = [(RUNTIME_PSRAM, PSRAM), (BOOTSTRAP_FLASH_TEXT, FLASH_XIP)];
    let mut index = 0;
    while index < nested.len() {
        let (inner, outer) = nested[index];
        assert!(outer.contains_range(inner.origin as u64, inner.end() as u64));
        index += 1;
    }
    assert!(RUNTIME_PSRAM.origin >= PSRAM.origin + BOOTSTRAP_PSRAM_BYTES);
    assert!(SRAM.contains_range(SECOND_STAGE_LOADER_START as u64, SRAM.end() as u64));
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_containment_rejects_inverted_and_overhanging_ranges() {
        let region = Region::new(0x100, 0x100);
        assert!(region.contains_range(0x100, 0x100));
        assert!(region.contains_range(0x100, 0x200));
        assert!(!region.contains_range(0x180, 0x170));
        assert!(!region.contains_range(0x180, 0x201));
        assert!(!region.contains_range(0xff, 0x100));
    }
}
