#![no_main]
#![no_std]

//! The ESP32-S31 bootstrap: the staged boot's chip-neutral steps
//! (`oer-espressif-staged-bootstrap`) around this board's PSRAM and its
//! 120-MHz Flash tuning, which needs the stage-two copy made first and a
//! cold XIP span.

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "oer-esp32s31-platform-bootstrap",
    "00:00:00",
    "2026-07-31",
    "6.1",
    oer_esp32s31_platform_board::FLASH_MMU_PAGE_SIZE_BYTES,
    0,
    u16::MAX,
    0
);

use core::mem::size_of;

use oer_esp32s31_platform_board as board;
use oer_esp32s31_platform_layout::LAYOUT;
use oer_esp32s31_soc_esp_hal::FlashMmu;
use oer_espressif_staged_bootstrap::{self as staged, fail, print};
use static_cell::ConstStaticCell;

const FLASH_TUNING_REFERENCE_WORDS: usize =
    LAYOUT.flash_tuning_reference_bytes as usize / size_of::<u32>();

// This is the load image, not its runtime placement. The bootstrap copies and
// verifies it at the bootloader's qualified 80 MHz setting before changing
// the Flash timing. The tuning sweep uses a separate, still-cold XIP span.
#[used]
#[unsafe(link_section = ".psram.runtime.payload")]
static RUNTIME_PAYLOAD: [u8; include_bytes!(env!("PSRAM_RUNTIME_BIN")).len()] =
    *include_bytes!(env!("PSRAM_RUNTIME_BIN"));

// Flash tuning must not sample pages already pulled into cache by the runtime
// copy. This private reference span is physically separate from the stage-two
// image and contains an incompressible deterministic pattern.
//
// SOURCE: pinned esp-hal fork, `esp-hal/src/flash.rs::Flash::tune_120mhz`,
// which requires at least fifteen distinct 4-KiB XIP pages.
#[used]
#[unsafe(link_section = ".flash.tuning.reference")]
static FLASH_TUNING_REFERENCE: [u32; FLASH_TUNING_REFERENCE_WORDS] = flash_tuning_reference();

// Flash tuning compares complete XIP pages while the flash cache is disabled,
// so its 124-KiB scratch area must live in internal SRAM. Keeping this unique
// owner static prevents the early-boot stack from being consumed by a
// short-lived local array; tune_120mhz still validates every original page.
static FLASH_TUNING_SCRATCH: ConstStaticCell<[u32; esp_hal::flash::FLASH_TUNING_SCRATCH_WORDS]> =
    ConstStaticCell::new([0; esp_hal::flash::FLASH_TUNING_SCRATCH_WORDS]);

#[esp_hal::main]
fn main() -> ! {
    staged::start();

    let peripherals = esp_hal::init(esp_hal::Config::default());
    print(c"OER_BOOT bootstrap=INIT\r\n");
    let mut flash_mmu = FlashMmu::new(peripherals.SPI0);
    let mut flash =
        match esp_hal::flash::Flash::new(peripherals.FLASH, esp_hal::flash::Config::default()) {
            Ok(flash) => flash,
            Err(_) => fail(c"OER_BOOT bootstrap=FAIL reason=flash-init\r\n"),
        };

    let psram = board::initialize_psram(peripherals.PSRAM);
    let (psram_base, psram_size) = psram.raw_parts();
    // SAFETY: `initialize_psram` mapped and initialized the board's PSRAM
    // at `psram_base..psram_base + psram_size` (word aligned) for reads and
    // writes; `psram` stays alive and mapped until `hand_off` and the boot
    // uses that region nowhere else, so `stage` owns it exclusively.
    let staged = unsafe { staged::stage(&LAYOUT, &RUNTIME_PAYLOAD, psram_base, psram_size) };

    let tuning_address = FLASH_TUNING_REFERENCE.as_ptr() as usize;
    let tuning_physical_start = flash_mmu
        .physical_address(tuning_address)
        .unwrap_or_else(|| fail(c"OER_BOOT bootstrap=FAIL reason=tuning-physical\r\n"));
    // SAFETY: the reference span is this bootstrap's own cold Flash rodata,
    // read by nothing else; stage two was copied from a disjoint range first.
    if unsafe {
        flash.tune_120mhz(
            esp_hal::flash::FlashXipRegion {
                physical_start: tuning_physical_start,
                virtual_start: tuning_address,
                size: size_of::<[u32; FLASH_TUNING_REFERENCE_WORDS]>(),
            },
            FLASH_TUNING_SCRATCH.take(),
        )
    }
    .is_err()
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=flash-tune\r\n");
    }
    print(c"OER_BOOT bootstrap=FLASH_TUNED\r\n");
    // Do not read the tuning span again. Every candidate deliberately fetches
    // a distinct page, and rejected timings can leave corrupted cache lines
    // behind until a future S31 cache-invalidate primitive is available. The
    // region is disposable; stage two was copied from a disjoint range first.
    staged::hand_off(staged)
}

const fn flash_tuning_reference() -> [u32; FLASH_TUNING_REFERENCE_WORDS] {
    let mut words = [0_u32; FLASH_TUNING_REFERENCE_WORDS];
    let mut state = 0x31a5_c33c_u32;
    let mut index = 0;
    while index < words.len() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        words[index] = state ^ (index as u32).rotate_left((index & 31) as u32);
        index += 1;
    }
    words
}
