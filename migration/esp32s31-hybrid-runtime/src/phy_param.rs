//! Explicit ESP32-S31 PHY-parameter transfer boundaries.
//!
//! These are complete, finite bodies recovered from
//! `libphy.a[phy_init.o]`. They temporarily address the vendor-defined
//! `phy_param` object by symbol. Keeping every bulk mutation here makes the
//! remaining transition to Rust-owned storage explicit: once the other live
//! `phy_init.o` functions have been replaced, the extern declaration can be
//! changed to a Rust static without changing these transforms.

use core::cell::UnsafeCell;

pub(crate) const PHY_PARAM_LEN: usize = 0x1fc;
pub(crate) const PHY_INIT_DATA_LEN: usize = 0x80;
pub(crate) const PHY_CALIBRATION_PAYLOAD_OFFSET: usize = 0x0c;
pub(crate) const PHY_CALIBRATION_CHECKSUM_OFFSET: usize =
    PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN;
pub(crate) const PHY_CALIBRATION_PREFIX_LEN: usize = PHY_CALIBRATION_CHECKSUM_OFFSET + 4;
const EFUSE_RD_MAC_SYS0_ADDRESS: usize = 0x2071_5050;
const EFUSE_RD_MAC_SYS1_ADDRESS: usize = 0x2071_5054;
const PHY_XTAL_FREQUENCY_REGISTER_ADDRESS: usize = 0x2010_f028;
const ESP32S31_XTAL_FREQUENCY_MHZ: u32 = 40;
const PHY_ROM_FUNCTION_TABLE_POINTER_CELL: usize = 0x2f07_fc3c;
const PHY_PARAM_ROM_CELL: usize = 0x2f07_fc40;
pub(crate) const PHY_ROM_FUNCTION_TABLE_ADDRESS: u32 = 0x2f07_f944;
const PHY_ROM_TXCAL_DEBUG_MODE_ADDRESS: u32 = 0x2f82_44fe;
const PHY_ROM_TONE_SAR_DOUT_ADDRESS: u32 = 0x2f82_66da;

/// Named layout of the rev0 ROM PHY callback table at `0x2f07_f944`.
///
/// The pointer ABI is always 32-bit on ESP32-S31, so storing addresses as
/// `u32` keeps this layout testable on the host as well as exact on RV32.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhyRomFunctionTable {
    i2c_enter_critical: u32,
    i2c_exit_critical: u32,
    get_i2c_read_mask: u32,
    get_i2c_host_id: u32,
    txcal_debug_mode: u32,
    set_rx_compensation: u32,
    set_temperature_sensor_power: u32,
    set_temperature_sensor_range: u32,
    get_temperature_sensor_value: u32,
    get_wifi_tx_table: u32,
    get_bt_tx_table: u32,
    get_tone_sar_dout: u32,
    configure_tx_gain_compensation: u32,
}

#[derive(Clone, Copy)]
struct PhyRomFunctionOverrides {
    i2c_enter_critical: u32,
    i2c_exit_critical: u32,
    get_i2c_read_mask: u32,
    get_i2c_host_id: u32,
    set_rx_compensation: u32,
    set_temperature_sensor_power: u32,
    set_temperature_sensor_range: u32,
    get_temperature_sensor_value: u32,
    get_wifi_tx_table: u32,
    get_bt_tx_table: u32,
    configure_tx_gain_compensation: u32,
}

const _: () = {
    assert!(core::mem::size_of::<PhyRomFunctionTable>() == 52);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, i2c_enter_critical) == 0x00);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, i2c_exit_critical) == 0x04);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, get_i2c_read_mask) == 0x08);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, get_i2c_host_id) == 0x0c);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, txcal_debug_mode) == 0x10);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, set_rx_compensation) == 0x14);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, set_temperature_sensor_power) == 0x18);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, set_temperature_sensor_range) == 0x1c);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, get_temperature_sensor_value) == 0x20);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, get_wifi_tx_table) == 0x24);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, get_bt_tx_table) == 0x28);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, get_tone_sar_dout) == 0x2c);
    assert!(core::mem::offset_of!(PhyRomFunctionTable, configure_tx_gain_compensation) == 0x30);
};

fn apply_rom_function_overrides(
    original: PhyRomFunctionTable,
    replacements: PhyRomFunctionOverrides,
) -> PhyRomFunctionTable {
    PhyRomFunctionTable {
        i2c_enter_critical: replacements.i2c_enter_critical,
        i2c_exit_critical: replacements.i2c_exit_critical,
        get_i2c_read_mask: replacements.get_i2c_read_mask,
        get_i2c_host_id: replacements.get_i2c_host_id,
        txcal_debug_mode: original.txcal_debug_mode,
        set_rx_compensation: replacements.set_rx_compensation,
        set_temperature_sensor_power: replacements.set_temperature_sensor_power,
        set_temperature_sensor_range: replacements.set_temperature_sensor_range,
        get_temperature_sensor_value: replacements.get_temperature_sensor_value,
        get_wifi_tx_table: replacements.get_wifi_tx_table,
        get_bt_tx_table: replacements.get_bt_tx_table,
        get_tone_sar_dout: original.get_tone_sar_dout,
        configure_tx_gain_compensation: replacements.configure_tx_gain_compensation,
    }
}

pub(crate) fn apply_init_data(parameter: &mut [u8; PHY_PARAM_LEN], init: &[u8; PHY_INIT_DATA_LEN]) {
    parameter[0x4e] = init[0x00];

    let mut index = 0;
    while index != 18 {
        parameter[0x50 + index] = init[0x02 + index];
        index += 1;
    }

    parameter[0x64] = init[0x18];

    index = 0;
    while index != 14 {
        parameter[0x6e + index] = init[0x19 + index];
        parameter[0x7c + index] = init[0x27 + index];
        parameter[0x8a + index] = init[0x35 + index];
        index += 1;
    }

    index = 0;
    while index != 9 {
        parameter[0x65 + index] = init[0x43 + index];
        index += 1;
    }
}

fn backup_parameter(
    parameter: &[u8; PHY_PARAM_LEN],
    calibration: &mut [u8; PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN],
) {
    let mut index = 0;
    while index != PHY_PARAM_LEN {
        calibration[PHY_CALIBRATION_PAYLOAD_OFFSET + index] = parameter[index];
        index += 1;
    }
}

fn recover_parameter(
    parameter: &mut [u8; PHY_PARAM_LEN],
    calibration: &[u8; PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN],
) {
    let mut index = 0;
    while index != PHY_PARAM_LEN {
        parameter[index] = calibration[PHY_CALIBRATION_PAYLOAD_OFFSET + index];
        index += 1;
    }
}

fn calibration_identity_from_efuse_words(mac_sys0: u32, mac_sys1: u32) -> [u8; 8] {
    [
        (mac_sys1 >> 8) as u8,
        mac_sys1 as u8,
        (mac_sys0 >> 24) as u8,
        (mac_sys0 >> 16) as u8,
        (mac_sys0 >> 8) as u8,
        mac_sys0 as u8,
        (mac_sys1 >> 24) as u8,
        (mac_sys1 >> 16) as u8,
    ]
}

fn read_u32_le(bytes: &[u8; PHY_CALIBRATION_PREFIX_LEN], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn write_u32_le(bytes: &mut [u8; PHY_CALIBRATION_PREFIX_LEN], offset: usize, value: u32) {
    let value = value.to_le_bytes();
    bytes[offset] = value[0];
    bytes[offset + 1] = value[1];
    bytes[offset + 2] = value[2];
    bytes[offset + 3] = value[3];
}

pub(crate) fn calibration_record_check_or_write(
    calibration: &mut [u8; PHY_CALIBRATION_PREFIX_LEN],
    check: bool,
    version: u32,
    mac_sys0: u32,
    mac_sys1: u32,
) -> i32 {
    write_u32_le(calibration, 0, version);
    let identity = calibration_identity_from_efuse_words(mac_sys0, mac_sys1);
    let mut index = 0;
    while index != identity.len() {
        calibration[4 + index] = identity[index];
        index += 1;
    }

    let mut sum = 0_u32;
    let mut offset = 0;
    while offset != PHY_CALIBRATION_CHECKSUM_OFFSET {
        sum = sum.wrapping_add(read_u32_le(calibration, offset));
        offset += 4;
    }
    let checksum = !sum;

    if check {
        i32::from(checksum != read_u32_le(calibration, PHY_CALIBRATION_CHECKSUM_OFFSET))
    } else {
        write_u32_le(calibration, PHY_CALIBRATION_CHECKSUM_OFFSET, checksum);
        0
    }
}

pub(crate) const fn saturate_phy_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

/// Apply the arithmetic half of ROM `phy_rc_cal` to the explicit parameter
/// image after the asynchronous owner has obtained the six-bit RC result.
///
/// Reference: `esp32s31_rev0_rom.elf` SHA-256
/// `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
/// complete `phy_rc_cal` body at `0x2f82_6242` and its finite saturation leaf
/// `phy_get_data_sat` at `0x2f82_6024`.
pub(crate) fn apply_rc_calibration_result(parameter: &mut [u8; PHY_PARAM_LEN], result: u8) {
    const NUMERATOR_SCALE: i32 = 82;
    const AUXILIARY_NUMERATOR_SCALE: i32 = 0x334;
    const AUXILIARY_DIVISOR_SCALE: i32 = 104;
    const UPPER_BOUNDS: [u8; 4] = [0x28, 0x14, 0x1e, 0x14];
    const PRIMARY_DIVISORS: [u8; 2] = [0x14, 0x28];
    const AUXILIARY_DIVISORS: [u8; 4] = [0x24, 0x28, 0x16, 0x20];

    parameter[0xe8] = result;
    let bounded_result = if result > 45 { 50 } else { result };
    let base = bounded_result as i32 + 56;
    let primary_numerator = base * NUMERATOR_SCALE;

    let mut index = 0;
    while index != PRIMARY_DIVISORS.len() {
        let divisor = PRIMARY_DIVISORS[index] as i32 * 10;
        let value = primary_numerator / divisor - 8;
        parameter[0xe9 + index] = saturate_phy_value(value, UPPER_BOUNDS[index], 2);
        index += 1;
    }

    let auxiliary_numerator = base * AUXILIARY_NUMERATOR_SCALE;
    index = 0;
    while index != AUXILIARY_DIVISORS.len() {
        let divisor = AUXILIARY_DIVISORS[index] as i32 * AUXILIARY_DIVISOR_SCALE;
        let value = auxiliary_numerator / divisor - 8;
        parameter[0xed + index] = saturate_phy_value(value, UPPER_BOUNDS[2 + (index & 1)], 0);
        index += 1;
    }

    let mut flags = u32::from_le_bytes([
        parameter[0xa4],
        parameter[0xa5],
        parameter[0xa6],
        parameter[0xa7],
    ]);
    flags |= 1 << 23;
    parameter[0xa4..0xa8].copy_from_slice(&flags.to_le_bytes());
}

pub(crate) const fn xtal_parameter_code(frequency_mhz: u32) -> u8 {
    match frequency_mhz {
        26 => 1,
        32 => 2,
        _ => 0,
    }
}

const fn with_xtal_frequency(value: u32, frequency_mhz: u32) -> u32 {
    (value & !0x3f) | (frequency_mhz.wrapping_sub(1) & 0x3f)
}

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    static mut phy_param: [u8; PHY_PARAM_LEN];

    fn phy_get_i2c_read_mask_new();
    fn phy_get_i2c_hostid_new();
    fn phy_set_rx_comp_new();
    fn phy_set_tsens_power();
    fn phy_set_tsens_range();
    fn phy_get_tsens_value();
    fn phy_wifi_get_tx_tab_new();
    fn phy_bt_get_tx_tab_new();
    fn phy_txgain_comp_pacfg_new();
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn trap_invalid_pointer() -> ! {
    core::arch::asm!("ebreak", options(noreturn));
}

/// No-op critical-section callbacks used by the one-owner Rust PHY path.
///
/// The pinned weak vendor definitions are both a single `ret`. Keep these
/// callbacks in SRAM because ROM may call through the table while cached
/// execution is unavailable.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.phy_cold"]
pub unsafe extern "C" fn wifi_strict_phy_i2c_enter_critical() {}

#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.phy_cold"]
pub unsafe extern "C" fn wifi_strict_phy_i2c_exit_critical() {}

/// Immutable Rust-owned backing for the temporary vendor `g_phyFuns` ABI.
///
/// Ten still-delegated cold/calibration functions load the public
/// `g_phyFuns` symbol before dispatching through the fixed rev0 ROM callback
/// table. The linker aliases that public name to this word. Consequently
/// those functions retain their input ABI without owning or mutating a C
/// `.bss` pointer.
///
/// Keep the word in internal SRAM: some PHY callbacks execute while cached
/// flash is unavailable. The table itself is the rev0 ROM-ABI RAM object
/// validated by [`wifi_strict_phy_get_romfunc_addr`].
#[repr(transparent)]
pub struct PhyRomFunctionTableBinding(UnsafeCell<u32>);

// The field has no Rust mutation API and the one-owner cold-PHY path only
// permits vendor readers. UnsafeCell is used solely to retain writable ELF
// section flags so the initialized word is copied into internal SRAM.
unsafe impl Sync for PhyRomFunctionTableBinding {}

const _: () = {
    assert!(core::mem::size_of::<PhyRomFunctionTableBinding>() == 4);
    assert!(core::mem::align_of::<PhyRomFunctionTableBinding>() == 4);
};

#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".critical.data.wifi_strict.phy_rom_function_table_binding"]
pub static wifi_strict_phy_rom_function_table_binding: PhyRomFunctionTableBinding =
    PhyRomFunctionTableBinding(UnsafeCell::new(PHY_ROM_FUNCTION_TABLE_ADDRESS));

/// Publish the Rust PHY parameter object and typed rev0 ROM callback table.
///
/// Reference: pinned `libphy.a[phy_init.o]::phy_get_romfunc_addr`, size
/// `0x98`, plus complete rev0 ROM leaves `phy_get_romfuncs` at `0x2f824a82`
/// and `phy_param_addr` at `0x2f824a8c`.
///
/// The two ROM leaves only load the pointer cell at `0x2f07fc3c` and store
/// the parameter pointer to `0x2f07fc40`; Rust performs those transactions
/// explicitly. The two callback slots not replaced by the vendor body are
/// validated against the pinned rev0 table before any callback publication.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_phy_get_romfunc_addr() {
    let table_address = (PHY_ROM_FUNCTION_TABLE_POINTER_CELL as *const u32).read_volatile();
    if table_address != PHY_ROM_FUNCTION_TABLE_ADDRESS {
        trap_invalid_pointer();
    }

    let table = table_address as usize as *mut PhyRomFunctionTable;
    let original = table.read_volatile();
    if original.txcal_debug_mode != PHY_ROM_TXCAL_DEBUG_MODE_ADDRESS
        || original.get_tone_sar_dout != PHY_ROM_TONE_SAR_DOUT_ADDRESS
    {
        trap_invalid_pointer();
    }

    let parameter_address = core::ptr::addr_of_mut!(phy_param).cast::<u8>() as usize as u32;
    (PHY_PARAM_ROM_CELL as *mut u32).write_volatile(parameter_address);
    let replacements = PhyRomFunctionOverrides {
        i2c_enter_critical: wifi_strict_phy_i2c_enter_critical as usize as u32,
        i2c_exit_critical: wifi_strict_phy_i2c_exit_critical as usize as u32,
        get_i2c_read_mask: phy_get_i2c_read_mask_new as usize as u32,
        get_i2c_host_id: phy_get_i2c_hostid_new as usize as u32,
        set_rx_compensation: phy_set_rx_comp_new as usize as u32,
        set_temperature_sensor_power: phy_set_tsens_power as usize as u32,
        set_temperature_sensor_range: phy_set_tsens_range as usize as u32,
        get_temperature_sensor_value: phy_get_tsens_value as usize as u32,
        get_wifi_tx_table: phy_wifi_get_tx_tab_new as usize as u32,
        get_bt_tx_table: phy_bt_get_tx_tab_new as usize as u32,
        configure_tx_gain_compensation: phy_txgain_comp_pacfg_new as usize as u32,
    };
    let published = apply_rom_function_overrides(original, replacements);

    // Preserve the exact store order from phy_get_romfunc_addr. In
    // particular, the two validated ROM-owned fields are never rewritten.
    core::ptr::addr_of_mut!((*table).i2c_enter_critical)
        .write_volatile(published.i2c_enter_critical);
    core::ptr::addr_of_mut!((*table).i2c_exit_critical).write_volatile(published.i2c_exit_critical);
    core::ptr::addr_of_mut!((*table).set_temperature_sensor_power)
        .write_volatile(published.set_temperature_sensor_power);
    core::ptr::addr_of_mut!((*table).get_temperature_sensor_value)
        .write_volatile(published.get_temperature_sensor_value);
    core::ptr::addr_of_mut!((*table).set_temperature_sensor_range)
        .write_volatile(published.set_temperature_sensor_range);
    core::ptr::addr_of_mut!((*table).get_i2c_read_mask).write_volatile(published.get_i2c_read_mask);
    core::ptr::addr_of_mut!((*table).get_i2c_host_id).write_volatile(published.get_i2c_host_id);
    core::ptr::addr_of_mut!((*table).configure_tx_gain_compensation)
        .write_volatile(published.configure_tx_gain_compensation);
    core::ptr::addr_of_mut!((*table).get_wifi_tx_table).write_volatile(published.get_wifi_tx_table);
    core::ptr::addr_of_mut!((*table).set_rx_compensation)
        .write_volatile(published.set_rx_compensation);
    core::ptr::addr_of_mut!((*table).get_bt_tx_table).write_volatile(published.get_bt_tx_table);
}

/// Publish the ESP32-S31 crystal profile to PHY state and hardware.
///
/// Reference: the complete pinned `libphy.a[phy_init.o]::phy_get_xtal_freq`
/// body, size `0x40`. ESP-IDF and the S31 HAL both define a fixed 40 MHz
/// crystal for this chip, so the former `rtc_clk_xtal_freq_get` call is a
/// constant rather than a hidden state query. The vendor body stores crystal
/// code zero in `phy_param[0x4f]`, then replaces bits 5:0 of `0x2010_f028`
/// with 39. The field's hardware meaning is not inferred beyond that exact
/// transaction.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.phy_cold"]
pub unsafe extern "C" fn wifi_strict_phy_get_xtal_freq() {
    let parameter = &mut *core::ptr::addr_of_mut!(phy_param);
    parameter[0x4f] = xtal_parameter_code(ESP32S31_XTAL_FREQUENCY_MHZ);

    let register = PHY_XTAL_FREQUENCY_REGISTER_ADDRESS as *mut u32;
    register.write_volatile(with_xtal_frequency(
        register.read_volatile(),
        ESP32S31_XTAL_FREQUENCY_MHZ,
    ));
}

/// Apply the evidenced fields from an ESP32-S31 128-byte PHY init profile.
///
/// Reference: pinned
/// `libphy.a[phy_init.o]::register_chipv7_phy_init_param`, size `0x94`.
/// The body copies 71 bytes into six disjoint `phy_param` ranges. Field names
/// beyond the offsets are intentionally not guessed.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_register_chipv7_phy_init_param(init: *const u8) {
    if init.is_null() {
        trap_invalid_pointer();
    }

    let parameter = &mut *core::ptr::addr_of_mut!(phy_param);
    let init = &*init.cast::<[u8; PHY_INIT_DATA_LEN]>();
    apply_init_data(parameter, init);
}

/// Copy the complete 508-byte PHY parameter image to or from calibration data.
///
/// Reference: pinned `libphy.a[phy_init.o]::phy_rfcal_data_sub_new`, size
/// `0x64`, and the complete rev0 ROM `phy_byte_to_word` body at `0x2f826034`.
/// The calibration payload begins at byte 12. A nonzero direction copies to
/// the calibration buffer; zero restores `phy_param`. The loop bound is
/// compile-time fixed and has no allocation, wait, callback, or MMIO.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_phy_rfcal_data_sub_new(calibration: *mut u8, backup: u32) {
    if calibration.is_null() {
        trap_invalid_pointer();
    }

    let parameter = &mut *core::ptr::addr_of_mut!(phy_param);
    if backup != 0 {
        let calibration =
            &mut *calibration.cast::<[u8; PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN]>();
        backup_parameter(parameter, calibration);
    } else {
        let calibration =
            &*calibration.cast::<[u8; PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN]>();
        recover_parameter(parameter, calibration);
    }
}

/// Back up `phy_param` into the calibration payload and return the vendor's
/// constant success result.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_phy_rf_cal_data_backup_new(calibration: *mut u8) -> i32 {
    wifi_strict_phy_rfcal_data_sub_new(calibration, 1);
    0
}

/// Restore `phy_param` from the calibration payload.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_phy_rf_cal_data_recovery_new(calibration: *mut u8) {
    wifi_strict_phy_rfcal_data_sub_new(calibration, 0);
}

/// Refresh and write or validate the bounded PHY calibration-record checksum.
///
/// Reference: the complete pinned
/// `libphy.a[phy_init.o]::phy_rfcal_data_check_new` body, size `0x7e`, and
/// the complete rev0 ROM leaves `phy_set_mac_data`, `phy_get_mac_addr`, and
/// `phy_byte_to_word`. The first 520 bytes are 130 little-endian words; the
/// checksum at bytes 520 through 523 is the one's complement of their
/// wrapping sum. The third ABI argument is instruction-proven unused.
///
/// The two direct eFuse reads use the public S31 `EFUSE_RD_MAC_SYS0/1`
/// addresses. There is no allocation, wait, callback, hidden mutable state,
/// or hardware-dependent loop bound.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_phy_rfcal_data_check_new(
    check: u32,
    calibration: *mut u8,
    _init_data: *const u8,
    version: u32,
) -> i32 {
    if calibration.is_null() {
        trap_invalid_pointer();
    }

    let mac_sys0 = (EFUSE_RD_MAC_SYS0_ADDRESS as *const u32).read_volatile();
    let mac_sys1 = (EFUSE_RD_MAC_SYS1_ADDRESS as *const u32).read_volatile();
    let calibration = &mut *calibration.cast::<[u8; PHY_CALIBRATION_PREFIX_LEN]>();
    calibration_record_check_or_write(calibration, check != 0, version, mac_sys0, mac_sys1)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_init_data, apply_rc_calibration_result, apply_rom_function_overrides,
        backup_parameter, calibration_identity_from_efuse_words, calibration_record_check_or_write,
        read_u32_le, recover_parameter, with_xtal_frequency, xtal_parameter_code,
        PhyRomFunctionOverrides, PhyRomFunctionTable, PHY_CALIBRATION_CHECKSUM_OFFSET,
        PHY_CALIBRATION_PAYLOAD_OFFSET, PHY_CALIBRATION_PREFIX_LEN, PHY_INIT_DATA_LEN,
        PHY_PARAM_LEN, PHY_ROM_TONE_SAR_DOUT_ADDRESS, PHY_ROM_TXCAL_DEBUG_MODE_ADDRESS,
    };

    #[test]
    fn rom_callback_publication_preserves_only_the_two_unmodified_slots() {
        let original = PhyRomFunctionTable {
            i2c_enter_critical: 0,
            i2c_exit_critical: 1,
            get_i2c_read_mask: 2,
            get_i2c_host_id: 3,
            txcal_debug_mode: PHY_ROM_TXCAL_DEBUG_MODE_ADDRESS,
            set_rx_compensation: 5,
            set_temperature_sensor_power: 6,
            set_temperature_sensor_range: 7,
            get_temperature_sensor_value: 8,
            get_wifi_tx_table: 9,
            get_bt_tx_table: 10,
            get_tone_sar_dout: PHY_ROM_TONE_SAR_DOUT_ADDRESS,
            configure_tx_gain_compensation: 12,
        };
        let replacements = PhyRomFunctionOverrides {
            i2c_enter_critical: 0x100,
            i2c_exit_critical: 0x101,
            get_i2c_read_mask: 0x102,
            get_i2c_host_id: 0x103,
            set_rx_compensation: 0x105,
            set_temperature_sensor_power: 0x106,
            set_temperature_sensor_range: 0x107,
            get_temperature_sensor_value: 0x108,
            get_wifi_tx_table: 0x109,
            get_bt_tx_table: 0x10a,
            configure_tx_gain_compensation: 0x10c,
        };

        let published = apply_rom_function_overrides(original, replacements);
        assert_eq!(published.txcal_debug_mode, PHY_ROM_TXCAL_DEBUG_MODE_ADDRESS);
        assert_eq!(published.get_tone_sar_dout, PHY_ROM_TONE_SAR_DOUT_ADDRESS);
        assert_eq!(published.i2c_enter_critical, 0x100);
        assert_eq!(published.i2c_exit_critical, 0x101);
        assert_eq!(published.get_i2c_read_mask, 0x102);
        assert_eq!(published.get_i2c_host_id, 0x103);
        assert_eq!(published.set_rx_compensation, 0x105);
        assert_eq!(published.set_temperature_sensor_power, 0x106);
        assert_eq!(published.set_temperature_sensor_range, 0x107);
        assert_eq!(published.get_temperature_sensor_value, 0x108);
        assert_eq!(published.get_wifi_tx_table, 0x109);
        assert_eq!(published.get_bt_tx_table, 0x10a);
        assert_eq!(published.configure_tx_gain_compensation, 0x10c);
    }

    #[test]
    fn init_mapping_matches_all_recovered_disjoint_ranges() {
        let mut parameter = [0xa5; PHY_PARAM_LEN];
        let mut init = [0_u8; PHY_INIT_DATA_LEN];
        let mut index = 0;
        while index != init.len() {
            init[index] = index as u8;
            index += 1;
        }

        apply_init_data(&mut parameter, &init);

        let mut expected = [0xa5; PHY_PARAM_LEN];
        expected[0x4e] = init[0];
        expected[0x50..0x62].copy_from_slice(&init[0x02..0x14]);
        expected[0x64] = init[0x18];
        expected[0x65..0x6e].copy_from_slice(&init[0x43..0x4c]);
        expected[0x6e..0x7c].copy_from_slice(&init[0x19..0x27]);
        expected[0x7c..0x8a].copy_from_slice(&init[0x27..0x35]);
        expected[0x8a..0x98].copy_from_slice(&init[0x35..0x43]);
        assert_eq!(parameter, expected);
    }

    #[test]
    fn calibration_transfer_owns_exactly_bytes_12_through_519() {
        let mut parameter = [0_u8; PHY_PARAM_LEN];
        let mut index = 0;
        while index != parameter.len() {
            parameter[index] = index.wrapping_mul(37) as u8;
            index += 1;
        }

        let mut calibration = [0x5a; PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN];
        backup_parameter(&parameter, &mut calibration);
        assert_eq!(
            &calibration[..PHY_CALIBRATION_PAYLOAD_OFFSET],
            &[0x5a; PHY_CALIBRATION_PAYLOAD_OFFSET]
        );
        assert_eq!(
            &calibration[PHY_CALIBRATION_PAYLOAD_OFFSET..],
            parameter.as_slice()
        );

        let mut recovered = [0; PHY_PARAM_LEN];
        recover_parameter(&mut recovered, &calibration);
        assert_eq!(recovered, parameter);
    }

    #[test]
    fn calibration_record_identity_checksum_and_mismatch_match_the_pinned_transform() {
        let mut calibration = [0_u8; PHY_CALIBRATION_PREFIX_LEN];
        let mut index = 0;
        while index != calibration.len() {
            calibration[index] = index.wrapping_mul(29) as u8;
            index += 1;
        }

        let version = 0x1234_5678;
        let mac_sys0 = 0xa1b2_c3d4;
        let mac_sys1 = 0xe5f6_0718;
        assert_eq!(
            calibration_identity_from_efuse_words(mac_sys0, mac_sys1),
            [0x07, 0x18, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6]
        );
        assert_eq!(
            calibration_record_check_or_write(&mut calibration, false, version, mac_sys0, mac_sys1,),
            0
        );
        assert_eq!(&calibration[..4], &version.to_le_bytes());
        assert_eq!(
            &calibration[4..12],
            &[0x07, 0x18, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6]
        );

        let mut sum = 0_u32;
        let mut offset = 0;
        while offset != PHY_CALIBRATION_CHECKSUM_OFFSET {
            sum = sum.wrapping_add(read_u32_le(&calibration, offset));
            offset += 4;
        }
        assert_eq!(
            read_u32_le(&calibration, PHY_CALIBRATION_CHECKSUM_OFFSET),
            !sum
        );
        assert_eq!(
            calibration_record_check_or_write(&mut calibration, true, version, mac_sys0, mac_sys1,),
            0
        );

        calibration[0x40] ^= 0x80;
        assert_eq!(
            calibration_record_check_or_write(&mut calibration, true, version, mac_sys0, mac_sys1,),
            1
        );
    }

    #[test]
    fn xtal_profile_matches_the_complete_pinned_transform() {
        assert_eq!(xtal_parameter_code(26), 1);
        assert_eq!(xtal_parameter_code(32), 2);
        assert_eq!(xtal_parameter_code(40), 0);
        assert_eq!(with_xtal_frequency(0xffff_ffc0, 40), 0xffff_ffe7);
        assert_eq!(with_xtal_frequency(0x1234_567f, 26), 0x1234_5659);
    }

    #[test]
    fn rc_calibration_arithmetic_matches_both_rom_threshold_branches() {
        let mut parameter = [0_u8; PHY_PARAM_LEN];
        parameter[0xa4..0xa8].copy_from_slice(&0x1200_0042_u32.to_le_bytes());
        apply_rc_calibration_result(&mut parameter, 45);
        assert_eq!(parameter[0xe8..0xeb], [45, 33, 12]);
        assert_eq!(parameter[0xed..0xf1], [14, 11, 28, 16]);
        assert_eq!(
            u32::from_le_bytes(parameter[0xa4..0xa8].try_into().unwrap()),
            0x1280_0042
        );

        parameter.fill(0);
        apply_rc_calibration_result(&mut parameter, 46);
        assert_eq!(parameter[0xe8..0xeb], [46, 35, 13]);
        assert_eq!(parameter[0xed..0xf1], [15, 12, 29, 18]);
        assert_eq!(
            u32::from_le_bytes(parameter[0xa4..0xa8].try_into().unwrap()),
            1 << 23
        );
    }
}
