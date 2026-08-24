#![no_std]

//! Retained Bluetooth-only entry points for compiled production comparison.

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Production-path probe for the reviewed BLE NRT interrupt MMIO prefix.
///
/// Verification stops the vendor side before its following log/callback
/// suffix; this function executes the exact restricted PAC transaction.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da() -> u32 {
    let [bank_0, bank_1] =
        open_esp_radio_esp32s31_bluetooth::validation::capture_and_acknowledge_interrupts();
    bank_0 ^ bank_1
}

/// Production-path probe for the finite scheduler-table MMIO transaction.
///
/// The vendor event/list suffix is deliberately outside this entry's claim.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr() {
    open_esp_radio_esp32s31_bluetooth::validation::initialize_scheduler_table();
}

/// Production-path probe for the exact finite MMIO behavior of
/// `bt_bb_v2_init_cmplx(1)`.
///
/// The vendor's version log is deliberately not part of the claim. The second
/// ABI argument supplies the reviewed byte at `phy_param + 0x120` to the Rust
/// side; the comparison profile seeds the same byte in the vendor image.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_btbb_v2_init_trace_r_sym_bt_bb_v2_init_cmplx_x1(
    _print_version: u32,
    gain_parameter: u32,
) {
    open_esp_radio_esp32s31_bluetooth::validation::initialize_baseband_v2(
        gain_parameter as u8,
    );
}

/// Retain every Bluetooth probe in the dedicated comparison image.
#[inline(never)]
pub fn retain_all_probes() {
    core::hint::black_box(
        open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da as *const (),
    );
    core::hint::black_box(
        open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr as *const (),
    );
    core::hint::black_box(open_btbb_v2_init_trace_r_sym_bt_bb_v2_init_cmplx_x1 as *const ());
}
