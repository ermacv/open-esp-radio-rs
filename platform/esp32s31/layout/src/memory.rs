//! Address map and runtime placement profiles.
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

/// Application-owned internal SRAM. ESP-IDF's `SRAM_SEG_END` retains the
/// memory above it for the second-stage loader and the ROM boot stack.
pub const SRAM: Region = Region::new(0x2f00_0000, 0x0007_afc0);

/// The unified 64-MiB Flash XIP instruction/data window.
pub const FLASH_XIP: Region = Region::new(0x4000_0000, 0x0400_0000);

/// Cached external RAM aperture of the fitted 16-MiB PSRAM.
pub const PSRAM: Region = Region::new(0x5000_0000, 0x0100_0000);

/// ESP image segment header preceding the bootstrap's Flash-mapped text.
pub const BOOTSTRAP_FLASH_TEXT: Region = FLASH_XIP.after(0x20);

/// PSRAM prefix reserved for bootstrap sections before stage two.
pub const BOOTSTRAP_PSRAM_BYTES: u32 = 0x1_0000;

/// Stage-two code relocated to PSRAM, following the bootstrap page.
pub const RUNTIME_PSRAM: Region = PSRAM.after(BOOTSTRAP_PSRAM_BYTES);

/// Stage-two code executed in place from its embedded Flash payload.
pub const RUNTIME_FLASH_CODE: Region = FLASH_XIP.after(0x140);

/// Minimum thread stack at the top of SRAM when task stacks stay in SRAM.
pub const MIN_SRAM_THREAD_STACK_BYTES: u32 = 0x1_0000;

/// Stage-two data in SRAM, leaving the minimum thread stack above it.
pub const RUNTIME_SRAM_DATA: Region =
    Region::new(SRAM.origin, SRAM.length - MIN_SRAM_THREAD_STACK_BYTES);

/// CPU0 task stack of the PSRAM task-stack profile.
pub const CPU0_PSRAM_TASK_STACK_BYTES: u32 = 0x3_0000;

/// Each hart's dedicated SRAM interrupt stack in the PSRAM task-stack profile.
pub const IRQ_STACK_BYTES: u32 = 0x8000;

// Every derived region stays inside its memory, the runtime follows the
// bootstrap's PSRAM prefix and SRAM data leaves the minimum thread stack.
const _: () = {
    let nested = [
        (RUNTIME_PSRAM, PSRAM),
        (RUNTIME_FLASH_CODE, FLASH_XIP),
        (BOOTSTRAP_FLASH_TEXT, FLASH_XIP),
        (RUNTIME_SRAM_DATA, SRAM),
    ];
    let mut index = 0;
    while index < nested.len() {
        let (inner, outer) = nested[index];
        assert!(outer.contains_range(inner.origin as u64, inner.end() as u64));
        index += 1;
    }
    assert!(RUNTIME_PSRAM.origin >= PSRAM.origin + BOOTSTRAP_PSRAM_BYTES);
    assert!(RUNTIME_SRAM_DATA.end() + MIN_SRAM_THREAD_STACK_BYTES <= SRAM.end());
};

/// Where stage-two code runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodePlacement {
    /// Executed in place from the Flash payload.
    Flash,
    /// Copied into PSRAM by the bootstrap.
    Psram,
}

/// Where stage-two ordinary data and BSS live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataPlacement {
    Sram,
    Psram,
}

/// A rejected placement combination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    /// Flash-resident code requires PSRAM data.
    FlashCodeWithSramData,
    /// PSRAM task stacks require PSRAM code and data.
    TaskStackWithoutPsram,
}

/// A validated stage-two placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeProfile {
    code: CodePlacement,
    data: DataPlacement,
    psram_task_stack: bool,
}

impl RuntimeProfile {
    /// Standalone applications: PSRAM code, data and CPU0 task stack.
    pub const STANDALONE: Self = Self {
        code: CodePlacement::Psram,
        data: DataPlacement::Psram,
        psram_task_stack: true,
    };

    pub const fn new(
        code: CodePlacement,
        data: DataPlacement,
        psram_task_stack: bool,
    ) -> Result<Self, ProfileError> {
        let psram_code = matches!(code, CodePlacement::Psram);
        let psram_data = matches!(data, DataPlacement::Psram);
        if !psram_code && !psram_data {
            Err(ProfileError::FlashCodeWithSramData)
        } else if psram_task_stack && !(psram_code && psram_data) {
            Err(ProfileError::TaskStackWithoutPsram)
        } else {
            Ok(Self {
                code,
                data,
                psram_task_stack,
            })
        }
    }

    pub const fn code(self) -> CodePlacement {
        self.code
    }

    pub const fn data(self) -> DataPlacement {
        self.data
    }

    pub const fn psram_task_stack(self) -> bool {
        self.psram_task_stack
    }

    pub const fn code_region(self) -> Region {
        match self.code {
            CodePlacement::Flash => RUNTIME_FLASH_CODE,
            CodePlacement::Psram => RUNTIME_PSRAM,
        }
    }

    pub const fn data_region(self) -> Region {
        match self.data {
            DataPlacement::Sram => RUNTIME_SRAM_DATA,
            DataPlacement::Psram => RUNTIME_PSRAM,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_require_psram_for_flash_code_and_task_stacks() {
        use CodePlacement as Code;
        use DataPlacement as Data;
        assert_eq!(
            RuntimeProfile::new(Code::Flash, Data::Sram, false),
            Err(ProfileError::FlashCodeWithSramData)
        );
        for (code, data) in [(Code::Flash, Data::Psram), (Code::Psram, Data::Sram)] {
            assert_eq!(
                RuntimeProfile::new(code, data, true),
                Err(ProfileError::TaskStackWithoutPsram)
            );
            assert!(RuntimeProfile::new(code, data, false).is_ok());
        }
        assert_eq!(
            RuntimeProfile::new(Code::Psram, Data::Psram, true),
            Ok(RuntimeProfile::STANDALONE)
        );
    }

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
