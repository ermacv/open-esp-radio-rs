//! Static cold-init storage for the power-management callback table.
//!
//! The pinned `pm_funcs_init` allocates exactly 0x44 bytes with
//! `calloc(1, 0x44)`, publishes the result through
//! `ptr_beacon_offset_funcs`, and calls the separately audited finite
//! `pm_beacon_offset_funcs_init` leaf. The matching deinit only frees that
//! pointer. These wrappers preserve the publication and callback-table
//! initialization while replacing ownership with fixed internal SRAM.

use core::ptr;

const PM_BEACON_OFFSET_FUNCTION_WORDS: usize = 0x44 / size_of::<usize>();

#[repr(C, align(4))]
struct StaticPmBeaconOffsetFunctions([usize; PM_BEACON_OFFSET_FUNCTION_WORDS]);

#[link_section = ".critical.bss.wifi_strict.pm_beacon_offset_functions"]
static mut PM_BEACON_OFFSET_FUNCTIONS: StaticPmBeaconOffsetFunctions =
    StaticPmBeaconOffsetFunctions([0; PM_BEACON_OFFSET_FUNCTION_WORDS]);

unsafe extern "C" {
    static mut ptr_beacon_offset_funcs: *mut usize;
    fn pm_beacon_offset_funcs_init();
}

/// Return whether the vendor callback-table cell owns the fixed Rust backing.
///
/// # Safety
///
/// Wi-Fi initialization/deinitialization must not mutate the cell
/// concurrently.
pub unsafe fn static_pm_functions_bound() -> bool {
    ptr::addr_of!(ptr_beacon_offset_funcs).read_volatile()
        == ptr::addr_of_mut!(PM_BEACON_OFFSET_FUNCTIONS).cast::<usize>()
}

/// Replace the one-allocation vendor PM callback-table initializer.
#[cfg(feature = "rust-static-pm-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_funcs_init() -> i32 {
    let backing = ptr::addr_of_mut!(PM_BEACON_OFFSET_FUNCTIONS).cast::<usize>();
    backing.write_bytes(0, PM_BEACON_OFFSET_FUNCTION_WORDS);
    ptr::addr_of_mut!(ptr_beacon_offset_funcs).write_volatile(backing);
    pm_beacon_offset_funcs_init();
    0
}

/// Paired deinitializer: release publication without freeing fixed SRAM.
#[cfg(feature = "rust-static-pm-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_funcs_deinit() -> i32 {
    ptr::addr_of_mut!(ptr_beacon_offset_funcs).write_volatile(ptr::null_mut());
    0
}
