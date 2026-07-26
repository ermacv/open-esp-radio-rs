use core::ffi::c_void;

unsafe extern "C" {
    #[link_name = "dbg_read_tx_ppdu"]
    fn vendor_read_tx_ppdu(frame: *mut c_void, context: usize);
    #[link_name = "dbg_dump_rx_ppdu"]
    fn vendor_dump_rx_ppdu(rx_control: *mut c_void, descriptor: *mut c_void);
    #[link_name = "dbg_dump_rx_sigb"]
    fn vendor_dump_rx_sigb(descriptor: *mut c_void, enabled: u32);
    #[link_name = "wifi_log"]
    fn vendor_wifi_log();
    #[link_name = "wifi_gpio_debug"]
    fn vendor_wifi_gpio_debug(selector: u32, value: u32);
    #[link_name = "esp_test_tx_enab_statistics"]
    fn vendor_test_tx_enable_statistics(queue: u32) -> i32;
    #[link_name = "esp_test_set_rx_error_occurs"]
    fn vendor_test_set_rx_error_occurs() -> i32;
    #[link_name = "esp_test_rx_parse_mu"]
    fn vendor_test_rx_parse_mu(descriptor: *mut c_void, rx_control: *mut c_void);
    #[link_name = "esp_test_rx_process_complete"]
    fn vendor_test_rx_process_complete(
        descriptor: *mut c_void,
        discarded: u32,
        first_descriptor: *mut c_void,
        subframe_count: u32,
        error: u32,
    );
    #[link_name = "wifi_assert"]
    fn vendor_wifi_assert(expression: bool, file: *const u8, function: *const u8, line: i32);
}

pub(crate) fn runtime_debug_link_wrappers_active() -> bool {
    core::ptr::eq(
        vendor_read_tx_ppdu as *const (),
        __wrap_dbg_read_tx_ppdu as *const (),
    ) && core::ptr::eq(
        vendor_dump_rx_ppdu as *const (),
        __wrap_dbg_dump_rx_ppdu as *const (),
    ) && core::ptr::eq(
        vendor_dump_rx_sigb as *const (),
        __wrap_dbg_dump_rx_sigb as *const (),
    ) && core::ptr::eq(vendor_wifi_log as *const (), __wrap_wifi_log as *const ())
        && core::ptr::eq(
            vendor_wifi_gpio_debug as *const (),
            __wrap_wifi_gpio_debug as *const (),
        )
        && core::ptr::eq(
            vendor_test_tx_enable_statistics as *const (),
            __wrap_esp_test_tx_enab_statistics as *const (),
        )
        && core::ptr::eq(
            vendor_test_set_rx_error_occurs as *const (),
            wifi_strict_esp_test_set_rx_error_occurs as *const (),
        )
        && core::ptr::eq(
            vendor_test_rx_parse_mu as *const (),
            __wrap_esp_test_rx_parse_mu as *const (),
        )
        && core::ptr::eq(
            vendor_test_rx_process_complete as *const (),
            __wrap_esp_test_rx_process_complete as *const (),
        )
        && core::ptr::eq(
            vendor_wifi_assert as *const (),
            __wrap_wifi_assert as *const (),
        )
}

/// Remove the verbose TX PPDU decoder from the `WIFI_LOG_NONE` profile.
#[no_mangle]
pub unsafe extern "C" fn __wrap_dbg_read_tx_ppdu(_frame: *mut c_void, _context: usize) {}

/// Remove the verbose RX PPDU decoder from the `WIFI_LOG_NONE` profile.
#[no_mangle]
pub unsafe extern "C" fn __wrap_dbg_dump_rx_ppdu(
    _rx_control: *mut c_void,
    _descriptor: *mut c_void,
) {
}

/// Remove the verbose RX SIG-B decoder from the `WIFI_LOG_NONE` profile.
#[no_mangle]
pub unsafe extern "C" fn __wrap_dbg_dump_rx_sigb(_descriptor: *mut c_void, _enabled: u32) {}

/// Remove every radio logging/formatting path from the strict runtime.
///
/// `wifi_log` is variadic in the vendor C ABI. On RISC-V the unused argument
/// registers are caller-owned, so a no-argument callee is the complete
/// allocation-free/no-wait interposition required by this profile.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wifi_log() {}

/// Suppress the optional two-word GPIO trace callback in the strict profile.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wifi_gpio_debug(_selector: u32, _value: u32) {}

/// Disable the optional TX test-statistics collector.
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_test_tx_enab_statistics(_queue: u32) -> i32 {
    0
}

/// Disable the optional RX error-occurrence test collector.
///
/// The pinned `libpp.a[test_hal_rx_statis.o]` body only increments one of
/// three external diagnostic counters when `wDevCtrl[0x44]` is nonzero. The
/// strict AP/STA profile does not expose this test mode, so returning the
/// vendor success value removes both the diagnostic pointer and the unrelated
/// 72-byte `wDevCtrl` object from this path.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_esp_test_set_rx_error_occurs() -> i32 {
    0
}

/// Disable the optional RX MU test-statistics parser.
///
/// The pinned implementation first checks `esp_test_rx_mu_statistics` and
/// otherwise returns without affecting descriptor ownership or frame delivery.
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_test_rx_parse_mu(
    _descriptor: *mut c_void,
    _rx_control: *mut c_void,
) {
}

/// Disable the optional per-frame RX test-statistics collector.
///
/// The vendor caller ignores its return path and immediately continues with
/// ordinary frame accounting and `wDev_IndicateFrame`.
#[no_mangle]
pub unsafe extern "C" fn __wrap_esp_test_rx_process_complete(
    _descriptor: *mut c_void,
    _discarded: u32,
    _first_descriptor: *mut c_void,
    _subframe_count: u32,
    _error: u32,
) {
}

/// Preserve successful vendor assertions and turn failure into an immediate
/// machine trap instead of the pinned infinite logging loop.
#[no_mangle]
pub unsafe extern "C" fn __wrap_wifi_assert(
    expression: bool,
    _file: *const u8,
    _function: *const u8,
    _line: i32,
) {
    if !expression {
        core::arch::asm!("ebreak", options(noreturn));
    }
}
