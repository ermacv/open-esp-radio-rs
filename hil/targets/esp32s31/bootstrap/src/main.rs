#![no_main]
#![no_std]

esp_bootloader_esp_idf::esp_app_desc!(
    "0.1.0",
    "open-radio-hil-bootstrap",
    "00:00:00",
    "2026-07-31",
    "6.1",
    open_esp_radio_hil_esp32s31_board::FLASH_MMU_PAGE_SIZE_BYTES,
    0,
    u16::MAX,
    0
);

mod flash_mapping;

use core::{arch::asm, ffi::CStr, mem::size_of, ptr};

use open_esp_radio_hil_esp32s31_board as board;

const RUNTIME_MAGIC: u32 = 0x3247_5453;
const RUNTIME_ABI_VERSION: u32 = 1;
const RUNTIME_PSRAM_ADDRESS: usize = 0x5001_0000;
const RUNTIME_FLASH_ADDRESS: usize = 0x4000_0140;
const FLASH_TUNING_REFERENCE_WORDS: usize = 64 * 1024 / size_of::<u32>();

#[repr(C)]
#[derive(Clone, Copy)]
struct RuntimeHeader {
    magic: u32,
    abi_version: u32,
    load_address: u32,
    entry: u32,
    payload_end: u32,
    bss_start: u32,
    bss_end: u32,
    header_size: u32,
    text_start: u32,
    text_end: u32,
    payload_crc32: u32,
}

const RUNTIME_CRC_OFFSET: usize = core::mem::offset_of!(RuntimeHeader, payload_crc32);

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
const FLASH_TUNING_REFERENCE_DATA: [u32; FLASH_TUNING_REFERENCE_WORDS] = flash_tuning_reference();

#[used]
#[unsafe(link_section = ".flash.tuning.reference")]
static FLASH_TUNING_REFERENCE: [u32; FLASH_TUNING_REFERENCE_WORDS] = FLASH_TUNING_REFERENCE_DATA;

unsafe extern "C" {
    fn ets_install_usb_printf();
    fn ets_printf(format: *const core::ffi::c_char, ...);
    static __stack_chk_guard: u32;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    print(c"OPEN_RADIO_HIL bootstrap=PANIC\r\n");
    halt()
}

#[esp_hal::main]
fn main() -> ! {
    unsafe { ets_install_usb_printf() };
    print(c"OPEN_RADIO_HIL bootstrap=START\r\n");

    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut flash = match esp_hal::flash::Flash::from_bootloader(peripherals.FLASH) {
        Ok(flash) => flash,
        Err(_) => fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=flash-init\r\n"),
    };

    let psram = board::initialize_psram(peripherals.PSRAM, true);
    let (psram_base, psram_size) = psram.raw_parts();
    if psram_base as usize != 0x5000_0000 || !board::has_expected_psram_capacity(&psram) {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=psram-init\r\n");
    }
    verify_psram_probe(psram_base);

    let source = RUNTIME_PAYLOAD.as_ptr();
    let source_address = source as usize;
    let source_end = source_address
        .checked_add(RUNTIME_PAYLOAD.len())
        .unwrap_or_else(|| fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=source-overflow\r\n"));
    if !(flash_mapping::XIP_START..flash_mapping::XIP_END).contains(&source_address)
        || source_end > flash_mapping::XIP_END
    {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=source-not-xip\r\n");
    }

    let layout = validate_header(
        read_header(source),
        source_address,
        psram_base as usize,
        psram_size,
    );
    if layout.payload_len != RUNTIME_PAYLOAD.len() {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=payload-length\r\n");
    }
    let source_crc = payload_crc32(source, layout.payload_len);
    if source_crc != layout.expected_crc32 {
        print_crc_failure(c"source-crc-80mhz", layout.expected_crc32, source_crc);
    }

    if layout.code_in_psram {
        unsafe {
            ptr::copy_nonoverlapping(source, layout.load_address as *mut u8, layout.payload_len)
        };
    }
    unsafe {
        ptr::write_bytes(
            layout.bss_start as *mut u8,
            0,
            layout.bss_end - layout.bss_start,
        )
    };
    if payload_crc32(layout.load_address as *const u8, layout.payload_len) != layout.expected_crc32
    {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=destination-crc\r\n");
    }

    let tuning_address = FLASH_TUNING_REFERENCE.as_ptr() as usize;
    let tuning_physical_start = unsafe { flash_mapping::physical_address(tuning_address) }
        .unwrap_or_else(|| fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=tuning-physical\r\n"));
    if unsafe {
        flash.tune_120mhz(esp_hal::flash::FlashXipRegion {
            physical_start: tuning_physical_start,
            virtual_start: tuning_address,
            size: size_of::<[u32; FLASH_TUNING_REFERENCE_WORDS]>(),
        })
    }
    .is_err()
    {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=flash-tune\r\n");
    }
    // Do not read the tuning span again. Every candidate deliberately fetches
    // a distinct page, and rejected timings can leave corrupted cache lines
    // behind until a future S31 cache-invalidate primitive is available. The
    // region is disposable; stage two was copied from a disjoint range first.
    if !layout.code_in_psram {
        let flash_runtime_crc = payload_crc32(source, layout.payload_len);
        if flash_runtime_crc != layout.expected_crc32 {
            print_crc_failure(
                c"flash-runtime-crc-120mhz",
                layout.expected_crc32,
                flash_runtime_crc,
            );
        }
    }
    if layout.code_in_psram
        && unsafe {
            esp_hal::psram::prepare_code(layout.load_address as *const u8, layout.payload_len)
        }
        .is_err()
    {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=psram-code\r\n");
    }

    print(c"OPEN_RADIO_HIL bootstrap=PASS handoff=stage2\r\n");
    unsafe { release_bootstrap_stack_watchpoint() };
    unsafe { jump_to_runtime(layout.entry) }
}

#[derive(Clone, Copy)]
struct ValidatedLayout {
    load_address: usize,
    entry: usize,
    payload_len: usize,
    bss_start: usize,
    bss_end: usize,
    expected_crc32: u32,
    code_in_psram: bool,
}

fn read_header(source: *const u8) -> RuntimeHeader {
    if RUNTIME_PAYLOAD.len() < size_of::<RuntimeHeader>() {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=short-header\r\n");
    }
    unsafe { source.cast::<RuntimeHeader>().read_unaligned() }
}

fn validate_header(
    header: RuntimeHeader,
    source_address: usize,
    psram_base: usize,
    psram_size: usize,
) -> ValidatedLayout {
    let load_address = header.load_address as usize;
    let entry = header.entry as usize;
    let payload_end = header.payload_end as usize;
    let bss_start = header.bss_start as usize;
    let bss_end = header.bss_end as usize;
    let text_start = header.text_start as usize;
    let text_end = header.text_end as usize;
    let psram_end = psram_base
        .checked_add(psram_size)
        .unwrap_or_else(|| fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=psram-range\r\n"));
    let code_in_psram = load_address == RUNTIME_PSRAM_ADDRESS;
    let code_in_flash = load_address == RUNTIME_FLASH_ADDRESS && load_address == source_address;
    let code_end_valid = if code_in_psram {
        payload_end <= psram_end
    } else {
        code_in_flash && payload_end <= flash_mapping::XIP_END
    };

    if header.magic != RUNTIME_MAGIC
        || header.abi_version != RUNTIME_ABI_VERSION
        || header.header_size as usize != size_of::<RuntimeHeader>()
        || (!code_in_psram && !code_in_flash)
        || payload_end <= load_address
        || text_start < load_address + size_of::<RuntimeHeader>()
        || text_end <= text_start
        || text_end > payload_end
        || entry < text_start
        || entry >= text_end
        || !entry.is_multiple_of(2)
        || bss_start < payload_end
        || bss_end < bss_start
        || !code_end_valid
        || bss_end > psram_end
    {
        fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=header\r\n");
    }

    ValidatedLayout {
        load_address,
        entry,
        payload_len: payload_end - load_address,
        bss_start,
        bss_end,
        expected_crc32: header.payload_crc32,
        code_in_psram,
    }
}

fn verify_psram_probe(base: *mut u8) {
    let probe = unsafe { base.add(0x100).cast::<u32>() };
    for (index, expected) in [0x31a5_c33c, 0xc35a_3cc3].into_iter().enumerate() {
        unsafe { probe.add(index).write_volatile(expected) };
        if unsafe { probe.add(index).read_volatile() } != expected {
            fail(c"OPEN_RADIO_HIL bootstrap=FAIL reason=psram-probe\r\n");
        }
    }
}

fn payload_crc32(address: *const u8, len: usize) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for index in 0..len {
        let byte = if (RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + size_of::<u32>()).contains(&index) {
            0
        } else {
            unsafe { address.add(index).read_volatile() }
        };
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
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

unsafe fn release_bootstrap_stack_watchpoint() {
    let expected_tdata2 = core::ptr::addr_of!(__stack_chk_guard) as usize | 1;
    let observed_tdata2: usize;
    unsafe {
        asm!(
            "csrw 0x7a0, zero",
            "csrr {observed}, 0x7a2",
            observed = out(reg) observed_tdata2,
            options(nostack),
        )
    };
    if observed_tdata2 == expected_tdata2 {
        unsafe {
            asm!(
                "csrw 0x7a1, zero",
                "csrw 0x7a2, zero",
                "fence rw, rw",
                options(nostack),
            )
        };
    }
}

unsafe fn jump_to_runtime(entry: usize) -> ! {
    unsafe {
        asm!(
            "csrci mstatus, 8",
            "fence rw, rw",
            "fence.i",
            "jalr zero, 0({entry})",
            entry = in(reg) entry,
            options(noreturn),
        )
    }
}

fn print(message: &'static CStr) {
    unsafe { ets_printf(message.as_ptr()) };
}

fn print_crc_failure(reason: &'static CStr, expected: u32, observed: u32) -> ! {
    unsafe {
        ets_printf(
            c"OPEN_RADIO_HIL bootstrap=FAIL reason=%s expected=%08x observed=%08x\r\n".as_ptr(),
            reason.as_ptr(),
            expected,
            observed,
        )
    };
    halt()
}

fn fail(message: &'static CStr) -> ! {
    print(message);
    halt()
}

fn halt() -> ! {
    loop {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
