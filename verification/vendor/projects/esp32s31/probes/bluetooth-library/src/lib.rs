#![no_std]

//! Retained Bluetooth-only entry points for compiled production comparison.

use open_esp_radio_esp32s31_bluetooth::validation::{
    BluetoothControllerSramAddress, BluetoothMemoryListPointerImage, BluetoothMemoryListSelector,
    BluetoothMemoryListSlot,
};

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
pub extern "C" fn open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da() {
    open_esp_radio_esp32s31_bluetooth::validation::capture_and_acknowledge_interrupts();
}

/// Production-path probe for the finite scheduler-table MMIO transaction.
///
/// The vendor event/list suffix is deliberately outside this entry's claim.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr() {
    open_esp_radio_esp32s31_bluetooth::validation::clear_scheduler_hardware_list_heads();
}

/// Compiled production entry for the complete 50-operation BTDM controller
/// HAL-init body under its exact standalone caller-derived profile.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_btdm_hal_init_trace_r_sym_bt_a_gdrujd2_mu_az_wyh75ba_r() {
    // SAFETY: the comparison image models the recovered powered and quiescent
    // prerequisites, retains the inactive IRQ owner, and performs no later
    // radio operation.
    unsafe {
        open_esp_radio_esp32s31_bluetooth::validation::initialize_controller_hal_reviewed_standalone();
    }
}

/// Compiled production entry for the source-127 controller-register prefix.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_btdm_modem_lp_timer_register_prefix() {
    // SAFETY: this terminal comparison image models all earlier controller
    // software stages and never installs a CPU route or resumes radio work.
    unsafe {
        open_esp_radio_esp32s31_bluetooth::validation::prepare_modem_lp_timer_registers();
    }
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
    // SAFETY: this dedicated comparison image seeds the vendor and production
    // executions after their modeled common-PHY prerequisite. It performs no
    // subsequent radio operation and terminates without reconstructing cold
    // ownership.
    unsafe {
        open_esp_radio_esp32s31_bluetooth::validation::initialize_baseband_v2(gain_parameter as u8);
    }
}

/// Compiled production entry for the accredited domain of
/// `phy_get_i2c_hostid_new`.
///
/// This wrapper delegates through the standalone Bluetooth route's shared-PHY
/// owner and converts its typed result at the C ABI boundary. Host selection
/// and the complete `ANA_CONF2` transaction remain in production PHY/PAC
/// code.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_i2c_host_trace_phy_get_i2c_hostid_new(block: u32) -> u32 {
    open_esp_radio_esp32s31_bluetooth::validation::configure_and_select_phy_i2c_host(block as u8)
}

#[inline(always)]
fn memory_list_selector(raw: u32) -> Option<BluetoothMemoryListSelector> {
    match raw {
        1 => Some(BluetoothMemoryListSelector::One),
        2 => Some(BluetoothMemoryListSelector::Two),
        3 => Some(BluetoothMemoryListSelector::Three),
        _ => None,
    }
}

#[inline(always)]
fn memory_list_image(raw: u32) -> Option<BluetoothMemoryListPointerImage> {
    if raw == 0 {
        Some(BluetoothMemoryListPointerImage::Zero)
    } else {
        BluetoothControllerSramAddress::new(raw)
            .ok()
            .map(BluetoothMemoryListPointerImage::Address)
    }
}

/// Compiled production entry for the current-RX memory-list setter.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_memory_list_a_trace_r_sym_ble_lbo_ru27_ea_u8_mv8_q7_uuf_z(
    selector: u32,
    pointer: u32,
) {
    let (Some(selector), Some(image)) =
        (memory_list_selector(selector), memory_list_image(pointer))
    else {
        return;
    };
    // SAFETY: this isolated image supplies only reviewed selectors and
    // pointer encodings, performs no concurrent or subsequent radio work,
    // and terminates without reconstructing cold ownership.
    unsafe {
        open_esp_radio_esp32s31_bluetooth::validation::program_memory_list_pointer(
            selector,
            BluetoothMemoryListSlot::CurrentRx,
            image,
        );
    }
}

/// Compiled production entry for the next-RX memory-list setter.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_memory_list_b_trace_r_sym_ble_zzr_ex_mrn8_edi_tfi7_penk(
    selector: u32,
    pointer: u32,
) {
    let (Some(selector), Some(image)) =
        (memory_list_selector(selector), memory_list_image(pointer))
    else {
        return;
    };
    // SAFETY: same isolated-image conditions as the current-RX probe apply.
    unsafe {
        open_esp_radio_esp32s31_bluetooth::validation::program_memory_list_pointer(
            selector,
            BluetoothMemoryListSlot::NextRx,
            image,
        );
    }
}

/// Compiled production entry for the complete BLE PHY register-init body.
///
/// The vendor function obtains these five values from linked globals and
/// providers. The comparison profile seeds those exact vendor locations and
/// passes the same values through this explicit Rust ABI projection.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_phy_register_init_trace_r_sym_ble_3472b6b_ni_qdn_wk_yo6_ggv(
    private_timing_source_byte: u32,
    environment_address: u32,
    resolving_list_address: u32,
    set_branch_control_0470_bit_18: u32,
    runtime_configuration_low_byte: u32,
) {
    // SAFETY: every comparison case models the recovered prerequisite state,
    // supplies live controller storage for the execution lifetime, performs
    // no later radio operation, and terminates without reconstructing cold
    // ownership.
    unsafe {
        let _accepted = open_esp_radio_esp32s31_bluetooth::validation::initialize_phy_registers(
            private_timing_source_byte as u8,
            environment_address,
            resolving_list_address,
            set_branch_control_0470_bit_18 != 0,
            runtime_configuration_low_byte as u8,
        );
    }
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
    core::hint::black_box(open_btdm_hal_init_trace_r_sym_bt_a_gdrujd2_mu_az_wyh75ba_r as *const ());
    core::hint::black_box(open_btdm_modem_lp_timer_register_prefix as *const ());
    core::hint::black_box(open_btbb_v2_init_trace_r_sym_bt_bb_v2_init_cmplx_x1 as *const ());
    core::hint::black_box(open_phy_i2c_host_trace_phy_get_i2c_hostid_new as *const ());
    core::hint::black_box(
        open_ble_memory_list_a_trace_r_sym_ble_lbo_ru27_ea_u8_mv8_q7_uuf_z as *const (),
    );
    core::hint::black_box(
        open_ble_memory_list_b_trace_r_sym_ble_zzr_ex_mrn8_edi_tfi7_penk as *const (),
    );
    core::hint::black_box(
        open_ble_phy_register_init_trace_r_sym_ble_3472b6b_ni_qdn_wk_yo6_ggv as *const (),
    );
}
