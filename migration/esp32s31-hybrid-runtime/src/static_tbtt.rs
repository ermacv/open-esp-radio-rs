//! Fixed ownership for the adaptive-TBTT scratch word.
//!
//! The pinned `pm_extend_tbtt_adaptive_attach` allocates
//! `(g_ic[0x2a2] + 1) * 4` bytes and publishes the result at byte offset 0x0c
//! of its static PM instance. The qualified STA/AP cold path attaches before
//! any extra virtual interface exists, so the recovered count is zero and the
//! exact owner is one `u32`.

use core::ptr;

const INTERFACE_COUNT_OFFSET: usize = 0x2a2;
const INTERFACE_PM_INSTANCE_OFFSET: usize = 0x430;
const INSTANCE_INTERFACE_OFFSET: usize = 0;
const INSTANCE_DATA_OFFSET: usize = 0x0c;
const ESP_OK: i32 = 0;
const ESP_ERR_WIFI_STATE: i32 = 0x3006;

#[link_section = ".critical.bss.wifi_strict.tbtt_adaptive_data"]
static mut TBTT_ADAPTIVE_DATA: u32 = 0;

unsafe extern "C" {
    fn pm_extend_tbtt_adaptive_instance() -> *mut u8;
}

unsafe fn instance_interface(instance: *mut u8) -> *mut *mut u8 {
    instance
        .add(INSTANCE_INTERFACE_OFFSET)
        .cast::<*mut u8>()
}

unsafe fn instance_data(instance: *mut u8) -> *mut *mut u32 {
    instance.add(INSTANCE_DATA_OFFSET).cast::<*mut u32>()
}

/// Return whether the pinned PM instance owns the fixed one-word backing.
///
/// # Safety
///
/// Wi-Fi initialization/deinitialization must not mutate the instance
/// concurrently.
pub unsafe fn static_tbtt_adaptive_bound() -> bool {
    let instance = pm_extend_tbtt_adaptive_instance();
    if instance.is_null() {
        return false;
    }
    let interface = instance_interface(instance).read_volatile();
    !interface.is_null()
        && interface
            .add(INTERFACE_COUNT_OFFSET)
            .cast::<u16>()
            .read_unaligned()
            == 0
        && interface
            .add(INTERFACE_PM_INSTANCE_OFFSET)
            .cast::<*mut u8>()
            .read_volatile()
            == instance
        && instance_data(instance).read_volatile() == ptr::addr_of_mut!(TBTT_ADAPTIVE_DATA)
}

/// Replace the one-allocation adaptive-TBTT attachment.
#[cfg(feature = "rust-static-tbtt-adaptive-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_extend_tbtt_adaptive_attach(interface: *mut u8) -> i32 {
    let instance = pm_extend_tbtt_adaptive_instance();
    if interface.is_null()
        || instance.is_null()
        || interface
            .add(INTERFACE_COUNT_OFFSET)
            .cast::<u16>()
            .read_unaligned()
            != 0
        || !interface
            .add(INTERFACE_PM_INSTANCE_OFFSET)
            .cast::<*mut u8>()
            .read_volatile()
            .is_null()
        || !instance_interface(instance).read_volatile().is_null()
        || !instance_data(instance).read_volatile().is_null()
    {
        return ESP_ERR_WIFI_STATE;
    }

    ptr::addr_of_mut!(TBTT_ADAPTIVE_DATA).write_volatile(0);
    instance_interface(instance).write_volatile(interface);
    interface
        .add(INTERFACE_PM_INSTANCE_OFFSET)
        .cast::<*mut u8>()
        .write_volatile(instance);
    instance_data(instance).write_volatile(ptr::addr_of_mut!(TBTT_ADAPTIVE_DATA));
    ESP_OK
}

/// Withdraw only the exact fixed adaptive-TBTT publication.
#[cfg(feature = "rust-static-tbtt-adaptive-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_extend_tbtt_adaptive_deattach() -> i32 {
    let instance = pm_extend_tbtt_adaptive_instance();
    if instance.is_null()
        || instance_data(instance).read_volatile() != ptr::addr_of_mut!(TBTT_ADAPTIVE_DATA)
    {
        return ESP_ERR_WIFI_STATE;
    }
    ptr::addr_of_mut!(TBTT_ADAPTIVE_DATA).write_volatile(0);
    instance_data(instance).write_volatile(ptr::null_mut());
    instance_interface(instance).write_volatile(ptr::null_mut());
    ESP_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_data_has_exact_one_interface_shape() {
        assert_eq!(size_of::<u32>(), 4);
        assert_eq!(align_of::<u32>(), 4);
        assert_eq!(INSTANCE_DATA_OFFSET, 0x0c);
    }
}
