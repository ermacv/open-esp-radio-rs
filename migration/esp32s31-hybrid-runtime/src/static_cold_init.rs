//! Direct serialized Wi-Fi cold-init orchestration.
//!
//! The pinned `wifi_init_in_caller_task` creates three OS objects indirectly:
//! one interrupt spin-lock token, one recursive global mutex and one ordinary
//! MAC-list mutex. It then calls four finite leaves in order:
//! `wifi_menuconfig_init`, `misc_nvs_init`, `ic_create_wifi_task`, and
//! `ieee80211_ioctl_init`.
//!
//! This replacement publishes fixed Rust lock objects and preserves the same
//! leaf order. The miscellaneous NVS and PP-task calls resolve through their
//! fixed-storage/taskless interposition boundaries.

use core::ffi::c_void;

const INIT_ERROR: i32 = 0x101;

unsafe extern "C" {
    fn wifi_menuconfig_init(config: *mut c_void) -> i32;
    fn misc_nvs_init() -> i32;
    fn misc_nvs_deinit() -> i32;
    fn ic_create_wifi_task() -> i32;
    fn ic_delete_wifi_task() -> i32;
    fn ieee80211_ioctl_init() -> i32;
    fn ieee80211_ioctl_deinit() -> i32;
}

unsafe fn deinitialize() -> i32 {
    let _ = ieee80211_ioctl_deinit();
    let _ = ic_delete_wifi_task();
    let _ = misc_nvs_deinit();
    if crate::adapter::unbind_static_wifi_init_locks() {
        0
    } else {
        INIT_ERROR
    }
}

/// Replace the indirect OS-object creation envelope while retaining only the
/// separately audited finite initialization leaves.
#[cfg(feature = "rust-static-wifi-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_wifi_init_in_caller_task(config: *mut c_void) -> i32 {
    if !crate::adapter::bind_static_wifi_init_locks() {
        return INIT_ERROR;
    }
    let mut status = wifi_menuconfig_init(config);
    if status == 0 {
        status = misc_nvs_init();
    }
    if status == 0 {
        status = ic_create_wifi_task();
    }
    if status == 0 {
        status = ieee80211_ioctl_init();
    }
    if status != 0 {
        let _ = deinitialize();
    }
    status
}

/// Matching direct deinitializer for the fixed cold-init publications.
#[cfg(feature = "rust-static-wifi-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_wifi_deinit_in_caller_task() -> i32 {
    deinitialize()
}
