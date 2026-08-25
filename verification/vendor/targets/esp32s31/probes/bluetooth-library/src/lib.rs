#![no_std]

//! Retained Bluetooth-only entry points for compiled production comparison.

use core::mem::ManuallyDrop;

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
pub extern "C" fn open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da() -> u32 {
    let [bank_0, bank_1] =
        open_esp_radio_esp32s31_bluetooth::validation::capture_and_acknowledge_interrupts();
    bank_0 ^ bank_1
}

/// Production-path probe for the primary BT MAC status/clear ISR prefix.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_btdm_primary_interrupt_trace_r_sym_bt_r9_gzfn_ubtn7k6m_htozbv() -> u32 {
    let [bank_0, bank_1] =
        open_esp_radio_esp32s31_bluetooth::validation::capture_primary_and_acknowledge_interrupts();
    bank_0 ^ bank_1
}

/// Production-path probe for baseline clear/enable and output preparation.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_btdm_primary_interrupt_prepare_trace() {
    open_esp_radio_esp32s31_bluetooth::validation::prepare_primary_interrupt_output();
}

/// Production-path probe for the finite scheduler-table MMIO transaction.
///
/// The vendor event/list suffix is deliberately outside this entry's claim.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr() {
    open_esp_radio_esp32s31_bluetooth::validation::clear_scheduler_table_low_bits();
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
/// This wrapper only constructs the official Bluetooth platform owner and
/// converts its typed result at the C ABI boundary. Host selection and the
/// complete `ANA_CONF2` transaction remain in the production PHY and ESP-HAL
/// adapter respectively.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_i2c_host_trace_phy_get_i2c_hostid_new(block: u32) -> u32 {
    // SAFETY: the verifier executes this entry in an isolated image and never
    // creates a second peripheral owner during the same execution.
    let peripherals = unsafe { esp_hal::peripherals::Peripherals::steal() };
    let platform = ManuallyDrop::new(
        open_esp_radio_esp32s31_radio_platform_esp_hal::EspHalRadioPlatform::new(
            peripherals.MODEM_SYSCON,
            peripherals.MODEM_LPCON,
            peripherals.HP_SYS_CLKRST,
            peripherals.PMU,
            peripherals.LP_AON_CLK_RST,
            peripherals.LP_PERI,
            peripherals.LP_TSENS,
            peripherals.I2C_ANA_MST,
        ),
    );
    let mut bluetooth = ManuallyDrop::new(
        platform
            .try_bluetooth()
            .unwrap_or_else(|_| unreachable!("isolated probe owns the Bluetooth lease")),
    );
    open_esp_radio_esp32s31_phy::validation::configure_and_select_phy_i2c_host(
        &mut *bluetooth,
        block as u8,
    )
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
    private_configuration_byte_0x10: u32,
    environment_address: u32,
    resolving_list_address: u32,
    option_byte_0x55_nonzero: u32,
    option_byte_0x59: u32,
) {
    // SAFETY: every comparison case models the recovered prerequisite state,
    // supplies live controller storage for the execution lifetime, performs
    // no later radio operation, and terminates without reconstructing cold
    // ownership.
    unsafe {
        let _accepted =
            open_esp_radio_esp32s31_bluetooth::validation::initialize_phy_registers(
                private_configuration_byte_0x10 as u8,
                environment_address,
                resolving_list_address,
                option_byte_0x55_nonzero != 0,
                option_byte_0x59 as u8,
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
    core::hint::black_box(
        open_btdm_hal_init_trace_r_sym_bt_a_gdrujd2_mu_az_wyh75ba_r as *const (),
    );
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
