//! Fixed storage for the non-persistent miscellaneous Wi-Fi settings.
//!
//! The pinned `misc_nvs_init` reaches `misc_nvs_load`, which unconditionally
//! obtains a zeroed 0x3c-byte block through the OSI allocator. With
//! `wifi_init_config_t::nvs_enable == 0`, it performs no NVS operation and
//! leaves the block zeroed. The live consumers only read or update the WPS
//! type/status words at offsets 4 and 8.
//!
//! These wrappers preserve that exact non-persistent state while replacing
//! dynamic ownership with fixed internal SRAM. They are cold-init leaves:
//! callers must serialize them with all Wi-Fi use.

use core::ptr;

const MISC_NVS_WORDS: usize = 0x3c / size_of::<u32>();

#[repr(C, align(4))]
struct StaticMiscNvs([u32; MISC_NVS_WORDS]);

#[link_section = ".critical.bss.wifi_strict.misc_nvs"]
static mut MISC_NVS: StaticMiscNvs = StaticMiscNvs([0; MISC_NVS_WORDS]);

#[link_section = ".critical.bss.wifi_strict.misc_nvs_initialized"]
static mut MISC_NVS_INITIALIZED: bool = false;

unsafe extern "C" {
    static mut g_misc_nvs: *mut u32;
}

/// Return whether the ROM ABI cell owns the fixed Rust backing.
///
/// # Safety
///
/// Wi-Fi initialization/deinitialization must not mutate this state
/// concurrently.
pub unsafe fn static_misc_nvs_bound() -> bool {
    ptr::addr_of!(MISC_NVS_INITIALIZED).read_volatile()
        && ptr::addr_of!(g_misc_nvs).read_volatile() == ptr::addr_of_mut!(MISC_NVS).cast::<u32>()
}

/// Replace the allocation in the non-persistent `misc_nvs_init` path.
#[cfg(feature = "rust-static-misc-nvs-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_misc_nvs_init() -> i32 {
    if !ptr::addr_of!(MISC_NVS_INITIALIZED).read_volatile() {
        let backing = ptr::addr_of_mut!(MISC_NVS).cast::<u32>();
        backing.write_bytes(0, MISC_NVS_WORDS);
        ptr::addr_of_mut!(g_misc_nvs).write_volatile(backing);
        ptr::addr_of_mut!(MISC_NVS_INITIALIZED).write_volatile(true);
    }
    0
}

/// Paired deinitializer: release publication without freeing fixed SRAM.
#[cfg(feature = "rust-static-misc-nvs-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_misc_nvs_deinit() -> i32 {
    ptr::addr_of_mut!(g_misc_nvs).write_volatile(ptr::null_mut());
    ptr::addr_of_mut!(MISC_NVS_INITIALIZED).write_volatile(false);
    0
}
