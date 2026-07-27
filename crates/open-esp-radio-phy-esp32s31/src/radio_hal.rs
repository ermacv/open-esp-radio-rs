//! Narrow ESP32-S31 radio-register leaves.
//!
//! These functions reproduce complete, finite ROM bodies whose only state is
//! documented MMIO. They are temporary runtime-local HAL boundaries until the
//! register layer moves into the ESP32-S31 radio HAL crate.

use open_esp_radio_hal_esp32s31::radio_registers::{phy_clock_oracle, pmu};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_hal_esp32s31::RadioRegisters;

const TSF_CONTROL_ADDRESS: usize = 0x2010_d814;
const TSF_LOW_ADDRESS: usize = 0x2010_d820;
const TSF_HIGH_ADDRESS: usize = 0x2010_d824;
const RX_DESCRIPTOR_LAST_LOW_ADDRESS: usize = 0x2010_408c;
const RX_DESCRIPTOR_LAST_HIGH_ADDRESS: usize = 0x2010_4c70;
const TX_CCA_CONTROL_ADDRESS: usize = 0x2010_4c5c;
const TX_QUEUE_CONTROL_BASE_ADDRESS: usize = 0x2010_4d70;
const TX_QUEUE_CONTROL_STRIDE: usize = 0x10;
const MAC_ADDRESS_LOW_BASE_ADDRESS: usize = 0x2010_405c;
const MAC_ADDRESS_HIGH_BASE_ADDRESS: usize = 0x2010_4060;
const MAC_ADDRESS_STRIDE: usize = 8;
const MAC_RX_ADDRESS_POLICY_BASE_ADDRESS: usize = 0x2010_4004;
const MAC_RX_ADDRESS_POLICY_STRIDE: usize = 8;
const MAC_RX_FRAME_POLICY_BASE_ADDRESS: usize = 0x2010_40d8;
const MAC_RX_MANAGEMENT_POLICY_BASE_ADDRESS: usize = 0x2010_4060;
const MAC_RX_MANAGEMENT_POLICY_STRIDE: usize = 8;
const MAC_CONTROL_ADDRESS: usize = 0x2010_4cac;
const WIFI_MAC_REGDMA_CONTROL_ADDRESS: usize = 0x2010_d83c;
const MAC_ADDRESS_VALID_BIT: u32 = 1 << 16;
const MAC_RX_MODE_MASK: u32 = (1 << 10) | (1 << 4);
const MAC_RX_CONTROL_POLICY_BIT: u32 = 1 << 6;
const MAC_RX_CONTROL_ADDRESS_BIT: u32 = 1 << 31;
const MAC_RX_MANAGEMENT_POLICY_BIT: u32 = 1 << 16;
const MAC_RX_UNIQUE_BSSID_BITS: u32 = (1 << 8) | (1 << 1);
const MAC_NO_RETENTION_CLEAR_BITS: u32 = 0x00ff_1000;
const WIFI_MAC_REGDMA_LINK_MASK: u32 = 0x001e_0000;
const WIFI_MAC_ACTIVE_REGDMA_LINK: u32 = 4;
const MAC_INTERFACE_COUNT: u32 = 4;
const MAC_RX_POLICY_QUEUE_COUNT: u32 = 3;
const PHY_FE_BB_ENABLE_MASK: u32 =
    phy_clock_oracle::fe_bb_clock_control_opaque::ROM_FE_BB_ENABLE_UNKNOWN.mask();
const PHY_CALIBRATION_CLOCK_MASK: u32 =
    phy_clock_oracle::fe_bb_clock_control_opaque::PHY_CALIBRATION_CLOCK_UNKNOWN.mask();
const ROM_OPEN_FE_BB_PMU_MASK: u32 = pmu::hp_active_hp_ck_power::ROM_OPEN_FE_BB_UNKNOWN_LOW.mask()
    | pmu::hp_active_hp_ck_power::HP_ACTIVE_XPD_BB_I2C.mask();
const PHY_PBUS_STATUS_ADDRESS: usize = 0x2010_0890;
const PHY_PBUS_RX_DCO_READ_ADDRESS: usize = 0x2010_0894;
const PHY_CLOCK_CONTROL_ADDRESS: usize = 0x2010_0890;
const PHY_RX_DCO_CONTROL_ADDRESS: usize = 0x2010_0434;
const PHY_TONE_PATH0_CONTROL_ADDRESS: usize = 0x2010_041c;
const PHY_TONE_PATH1_CONTROL_ADDRESS: usize = 0x2010_0420;
const PHY_TONE_STOP_CONTROL_ADDRESS: usize = 0x2010_040c;
const PHY_TONE_SELECTOR_CONTROL_ADDRESS: usize = 0x2010_0428;
const PHY_TX_GAIN_COMPENSATION_CONTROL_ADDRESS: usize = 0x2010_0410;
const PHY_TX_GAIN_COMPENSATION_AUX_ADDRESS: usize = 0x2010_0414;
const PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS: usize = 0x2010_0418;
const PHY_DAC_SCALE_CONTROL_ADDRESS: usize = 0x2010_0c04;
const PHY_SDM_CYCLE_COUNTER_ADDRESS: usize = 0x2010_d800;
const PHY_I2C_CLOCK_SELECTION_0_ADDRESS: usize = 0x2010_f824;
const PHY_I2C_CLOCK_SELECTION_1_ADDRESS: usize = 0x2010_f828;
const PHY_I2C_CLOCK_SELECTION_2_ADDRESS: usize = 0x2010_f82c;
const PHY_FE_TXRX_RESET_ADDRESS: usize = 0x2010_0440;
const PHY_ADC_RATE_ADDRESS: usize = 0x2010_0448;
const PHY_FE_CONTROL_040C_ADDRESS: usize = 0x2010_040c;
const PHY_FE_CONTROL_0438_ADDRESS: usize = 0x2010_0438;
const PHY_FE_CONTROL_0444_ADDRESS: usize = 0x2010_0444;
const PHY_FE_CONTROL_0448_ADDRESS: usize = 0x2010_0448;
const PHY_FE_CONTROL_086C_ADDRESS: usize = 0x2010_086c;
const PHY_FE_CONTROL_0894_ADDRESS: usize = 0x2010_0894;
const PHY_FE_CONTROL_0C08_ADDRESS: usize = 0x2010_0c08;
const PHY_FE_CONTROL_0C0C_ADDRESS: usize = 0x2010_0c0c;
const PHY_FE_CONTROL_0C20_ADDRESS: usize = 0x2010_0c20;
const PHY_TEMPERATURE_SENSOR_POWER_ADDRESS: usize = 0x2081_8000;
const PHY_TEMPERATURE_SENSOR_CONTROL_ADDRESS: usize = 0x2081_8018;
const PHY_TEMPERATURE_SENSOR_SYSTEM_CONTROL_ADDRESS: usize = 0x2071_0030;
const PHY_IQ_CORRECTION_CONTROL_ADDRESS: usize = 0x2010_0438;
const PHY_IQ_CORRECTION_AUX_ADDRESS: usize = 0x2010_0c0c;
const PHY_REGISTER_FORCE_TXRX_ADDRESS: usize = 0x2010_0890;
const PHY_REGISTER_I2C_MASTER_STATUS_0_ADDRESS: usize = 0x2010_f800;
const PHY_REGISTER_I2C_MASTER_STATUS_1_ADDRESS: usize = 0x2010_f804;
const PHY_REGISTER_XTAL_CONTROL_ADDRESS: usize = 0x2010_f028;
const PHY_REGISTER_FORCE_TXRX_RETAIN_MASK: u32 = 0xffff_f0ff;
const PHY_REGISTER_I2C_MASTER_BUSY_BIT: u32 = 1 << 25;
const PHY_REGISTER_I2C_MASTER_RESET_BIT: u32 = 1 << 26;
const PHY_PBUS_TRANSACTION_BIT: u32 = 1 << 1;
const PHY_PBUS_BUSY_BIT: u32 = 1 << 31;

const fn with_phy_register_force_txrx(value: u32, enabled: bool, phase: u8) -> u32 {
    let forced = match (enabled, phase) {
        (true, 0) => 0x0000_0800,
        (true, _) => 0x0000_0a00,
        (false, 0) => 0x0000_0200,
        (false, _) => 0,
    };
    (value & PHY_REGISTER_FORCE_TXRX_RETAIN_MASK) | forced
}

const fn with_phy_register_xtal_frequency(value: u32, frequency_mhz: u8) -> u32 {
    (value & !0x3f) | (frequency_mhz.wrapping_sub(1) & 0x3f) as u32
}

/// Apply the two finite parent MMIO operations at `phy_bb_init+0x0..0x28`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn enable_phy_baseband_initialization() {
    set_register_bits(
        phy_clock_oracle::FE_BB_CLOCK_CONTROL_OPAQUE.address(),
        PHY_CALIBRATION_CLOCK_MASK,
    );
}

/// Apply one of the two finite register writes surrounding each one
/// microsecond edge in complete ROM `phy_force_txrx_off`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_register_force_txrx(
    registers: &mut RadioRegisters,
    enabled: bool,
    phase: u8,
) {
    // SAFETY: the identity-bound action restricts the address and field
    // encoding to the recovered finite parent operation.
    unsafe {
        let previous = registers.read(PHY_REGISTER_FORCE_TXRX_ADDRESS);
        registers.write(
            PHY_REGISTER_FORCE_TXRX_ADDRESS,
            with_phy_register_force_txrx(previous, enabled, phase),
        );
    }
}

/// Observe one PHY-I2C master reset status register.
#[cfg(target_arch = "riscv32")]
pub(crate) fn sample_phy_i2c_master_reset(registers: &mut RadioRegisters, index: u8) -> u32 {
    let address = if index == 0 {
        PHY_REGISTER_I2C_MASTER_STATUS_0_ADDRESS
    } else {
        PHY_REGISTER_I2C_MASTER_STATUS_1_ADDRESS
    };
    // SAFETY: lowering validates `index` and binds the exact status address.
    unsafe { registers.read(address) }
}

/// Issue one reset command. Completion is sampled asynchronously by the
/// caller; this leaf never waits for the busy bit to clear.
#[cfg(target_arch = "riscv32")]
pub(crate) fn pulse_phy_i2c_master_reset(registers: &mut RadioRegisters, index: u8) {
    let address = if index == 0 {
        PHY_REGISTER_I2C_MASTER_STATUS_0_ADDRESS
    } else {
        PHY_REGISTER_I2C_MASTER_STATUS_1_ADDRESS
    };
    // SAFETY: lowering validates `index`; the recovered parent writes exactly
    // the documented reset command.
    unsafe {
        registers.write(address, PHY_REGISTER_I2C_MASTER_RESET_BIT);
    }
}

/// Whether the exact busy bit sampled by complete ROM
/// `phy_i2c_master_reset` remains set.
pub(crate) const fn phy_i2c_master_reset_busy(value: u32) -> bool {
    value & PHY_REGISTER_I2C_MASTER_BUSY_BIT != 0
}

/// Gate the calibration region around `phy_rf_init` and `phy_bb_init`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_phy_register_calibration_clock(registers: &mut RadioRegisters, enabled: bool) {
    registers.modify32(
        phy_clock_oracle::FE_BB_CLOCK_CONTROL_OPAQUE,
        PHY_CALIBRATION_CLOCK_MASK,
        if enabled {
            PHY_CALIBRATION_CLOCK_MASK
        } else {
            0
        },
    );
}

/// Program the S31's fixed 40 MHz crystal field without consulting hidden
/// RTC or ROM state.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_register_xtal_frequency(registers: &mut RadioRegisters) {
    // SAFETY: ESP32-S31 uses the recovered fixed 40 MHz crystal field.
    unsafe {
        let previous = registers.read(PHY_REGISTER_XTAL_CONTROL_ADDRESS);
        registers.write(
            PHY_REGISTER_XTAL_CONTROL_ADDRESS,
            with_phy_register_xtal_frequency(previous, 40),
        );
    }
}

/// Complete rev0 ROM `phy_bb_agc_reg_update`, size `0xa6`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_bb_agc_register_update(registers: &mut RadioRegisters) {
    open_esp_radio_hal_esp32s31::phy_agc::update_baseband_registers(registers);
}

/// Complete rev0 ROM `phy_enable_agc`, size `0x28`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn enable_phy_agc(registers: &mut RadioRegisters) {
    open_esp_radio_hal_esp32s31::phy_agc::set_enabled(registers, true);
}

/// Select the exact AGC state used by `phy_chip_set_chan`.
///
/// Complete ROM `phy_disable_agc` only sets the PAC's recovered AGC-disable
/// field in the shared AGC/antenna control word.
/// Re-enabling uses the already recovered three-write `phy_enable_agc`
/// sequence. Both branches are finite and touch no software state.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_phy_channel_agc(registers: &mut RadioRegisters, enabled: bool) {
    open_esp_radio_hal_esp32s31::phy_agc::set_enabled(registers, enabled);
}

/// Complete both branches of rev0 ROM `phy_rx_11b_opt`, size `0xc4`.
#[cfg(target_arch = "riscv32")]
fn configure_phy_rx_11b_optimization(registers: &mut RadioRegisters, enabled: bool) {
    open_esp_radio_hal_esp32s31::phy_agc::configure_rx_11b_optimization(registers, enabled);
}

/// Complete rev0 ROM `phy_reg_init` at `0x2f82_3ef8`, size `0x52`, with
/// every direct and tail child reproduced by source-owned safe HAL leaves.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_registers(
    registers: &mut RadioRegisters,
    parameters: crate::phy_bb::PhyRegisterInitParameters,
) {
    open_esp_radio_hal_esp32s31::phy_baseband::enable_iq_correction(registers);
    open_esp_radio_hal_esp32s31::phy_agc::initialize_registers(
        registers,
        parameters.parameter_121,
        parameters.parameter_120,
    );
    open_esp_radio_hal_esp32s31::phy_agc::set_saturation_gain(registers, 0x0008_1825);
    open_esp_radio_hal_esp32s31::phy_baseband::initialize_baseband(registers);
    open_esp_radio_hal_esp32s31::phy_baseband::configure_watchdog(registers);
    open_esp_radio_hal_esp32s31::phy_baseband::configure_tx_pa_on(registers);
    configure_phy_rx_11b_optimization(registers, true);
    open_esp_radio_hal_esp32s31::phy_power_detector::configure_background(registers);
    open_esp_radio_hal_esp32s31::phy_baseband::configure_noise_floor_auto(registers);
    open_esp_radio_hal_esp32s31::phy_agc::configure_antenna(registers);
    open_esp_radio_hal_esp32s31::phy_frequency::configure_bt_filter(registers);
    open_esp_radio_hal_esp32s31::phy_frequency::enable_mac_baseband(registers);
}

/// Complete pinned `libphy.a[phy_rx_gain.o]::phy_rx_table_init`, size `0x7c`.
///
/// The unique [`crate::phy_cold::PhyColdState`] owner must call
/// `prepare_rx_table_init` before executing this action. That explicit local
/// step performs the reference's `phy_param[0x120] = 0x4f` mutation. This leaf
/// then publishes exactly 79 gain entries and runs the already complete
/// register-init, AGC-update and AGC-enable suffix.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_rx_table(
    registers: &mut RadioRegisters,
    parameters: crate::phy_bb::PhyRxTableInitParameters,
) {
    let mut index = 0_u8;
    while index != crate::phy_bb::PHY_RX_TABLE_ENTRY_COUNT {
        let entry = crate::phy_bb::phy_rx_table_gain_entry(parameters, index);
        open_esp_radio_hal_esp32s31::phy_memory::program_gain_memory_entry(
            registers,
            [entry.word0, entry.word1, entry.word2],
            entry.index,
        );
        index += 1;
    }
    configure_phy_registers(
        registers,
        crate::phy_bb::PhyRegisterInitParameters {
            parameter_121: parameters.parameter_121,
            parameter_120: crate::phy_bb::PHY_RX_TABLE_ENTRY_COUNT,
        },
    );
    configure_phy_bb_agc_register_update(registers);
    enable_phy_agc(registers);
}

const fn tsf_latch_mask(interface: u32) -> u32 {
    if interface == 0 {
        1
    } else {
        2
    }
}

const fn join_rx_descriptor_address(low_word: u32, high_word: u32) -> usize {
    ((low_word & 0x000f_ffff) | (high_word & 0xfff0_0000)) as usize
}

const fn tx_queue_control_address(queue: u8) -> usize {
    TX_QUEUE_CONTROL_BASE_ADDRESS.wrapping_sub((queue as usize) * TX_QUEUE_CONTROL_STRIDE)
}

const fn mac_address_registers(interface: u32) -> (usize, usize) {
    let offset = (interface as usize) * MAC_ADDRESS_STRIDE;
    (
        MAC_ADDRESS_LOW_BASE_ADDRESS + offset,
        MAC_ADDRESS_HIGH_BASE_ADDRESS + offset,
    )
}

const fn encode_mac_address(address: [u8; 6]) -> (u32, u32) {
    (
        u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
        u16::from_le_bytes([address[4], address[5]]) as u32 | MAC_ADDRESS_VALID_BIT,
    )
}

const fn mac_rx_frame_policy_address(queue: u32) -> usize {
    MAC_RX_FRAME_POLICY_BASE_ADDRESS + (queue as usize) * core::mem::size_of::<u32>()
}

const fn mac_rx_address_policy_address(queue: u32) -> usize {
    MAC_RX_ADDRESS_POLICY_BASE_ADDRESS + (queue as usize) * MAC_RX_ADDRESS_POLICY_STRIDE
}

const fn mac_rx_management_policy_address(queue: u32) -> usize {
    MAC_RX_MANAGEMENT_POLICY_BASE_ADDRESS + (queue as usize) * MAC_RX_MANAGEMENT_POLICY_STRIDE
}

const fn with_mac_rx_mode(value: u32, mode: u32) -> u32 {
    if mode <= 1 {
        value & !MAC_RX_MODE_MASK
    } else {
        value | MAC_RX_MODE_MASK
    }
}

const fn with_mac_rx_control_policy(value: u32, control: u32) -> u32 {
    if control <= 1 {
        value & !MAC_RX_CONTROL_POLICY_BIT
    } else {
        value | MAC_RX_CONTROL_POLICY_BIT
    }
}

const fn with_mac_rx_control_address_policy(value: u32, control: u32) -> u32 {
    match control {
        0 => value & !MAC_RX_CONTROL_ADDRESS_BIT,
        1 => value | MAC_RX_CONTROL_ADDRESS_BIT,
        _ => value,
    }
}

const fn with_mac_rx_management_policy(value: u32, management: u32) -> u32 {
    if management == 0 {
        value & !MAC_RX_MANAGEMENT_POLICY_BIT
    } else {
        value | MAC_RX_MANAGEMENT_POLICY_BIT
    }
}

const fn with_mac_rx_unique_bssid_policy(value: u32, enabled: u32) -> u32 {
    if enabled == 0 {
        value & !MAC_RX_UNIQUE_BSSID_BITS
    } else {
        value | MAC_RX_UNIQUE_BSSID_BITS
    }
}

const fn without_mac_tx_retention(value: u32) -> u32 {
    value & !MAC_NO_RETENTION_CLEAR_BITS
}

const fn with_wifi_mac_regdma_link(value: u32, link: u32) -> u32 {
    (value & !WIFI_MAC_REGDMA_LINK_MASK) | ((link << 17) & WIFI_MAC_REGDMA_LINK_MASK)
}

const fn with_tx_cca(value: u32, cca: u32) -> u32 {
    (value & 0x3fff_ffff) | (cca << 30)
}

const fn tx_queue_is_valid(value: u32) -> u32 {
    (value >> 30) & 1
}

const fn without_tx_queue_valid(value: u32) -> u32 {
    value & 0xbfff_ffff
}

const fn without_tx_queue_enable(value: u32) -> u32 {
    value & 0x3fff_ffff
}

const fn without_fe_bb_clock_enable(value: u32) -> u32 {
    value & !PHY_FE_BB_ENABLE_MASK
}

const fn with_phy_pbus_force_test(value: u32, selector: u8, path: u8, test_value: u16) -> u32 {
    let command = ((test_value as u32) << 6) | ((selector as u32) << 2) | ((path as u32) << 15);
    (value & 0xfffe_0001) | (command & 0x0001_fffc) | PHY_PBUS_TRANSACTION_BIT
}

const fn phy_pbus_is_busy(value: u32) -> bool {
    value & PHY_PBUS_BUSY_BIT != 0
}

const fn phy_pbus_rx_dco_read_value(value: u32) -> u16 {
    (value & 0x1ff) as u16
}

const fn phy_pbus_read_address(selector: u8, path: u8) -> usize {
    match selector {
        0 => 0x2010_08a4,
        1 => 0x2010_0894,
        2 => {
            if path == 1 {
                0x2010_0898
            } else {
                0x2010_089c
            }
        }
        3 => 0x2010_089c,
        4 => {
            if path == 1 {
                0x2010_08a0
            } else {
                0x2010_08a4
            }
        }
        5 => 0x2010_08a4,
        _ => 0x2010_08a4,
    }
}

const fn phy_pbus_read_shift(selector: u8, path: u8) -> u8 {
    match selector {
        0 => {
            if path == 1 {
                18
            } else {
                9
            }
        }
        1 | 3 => {
            if path == 1 {
                9
            } else {
                0
            }
        }
        2 | 4 => {
            if path == 1 {
                0
            } else {
                18
            }
        }
        5 => 0,
        _ => 0,
    }
}

const fn with_phy_tx_clock(value: u32, enabled: bool) -> u32 {
    (value & !0x0003_0000) | if enabled { 0x0003_0000 } else { 0 }
}

const fn with_phy_rx_clock(value: u32, enabled: bool) -> u32 {
    (value & !0x0000_c000) | if enabled { 0x0000_c000 } else { 0 }
}

const fn without_phy_rx_dco_control_field(value: u32) -> u32 {
    value & !0x00c0_0000
}

const fn with_restored_phy_rx_dco_control_field(value: u32, saved_field: u32) -> u32 {
    without_phy_rx_dco_control_field(value) | (saved_field & 0x00c0_0000)
}

const fn with_phy_tone_path(value: u32, enable: i32, selector: i32, step: i32) -> u32 {
    let encoded = (enable as u32).wrapping_shl(18)
        | ((selector >> 2) as u32)
        | ((step.wrapping_neg() as u32) & 0xff).wrapping_shl(10);
    (value & 0xf000_0000) | (encoded & 0x0fff_ffff)
}

const fn with_phy_tone_path0_selector(value: u32, selector: i32) -> u32 {
    (value & !0x3) | ((selector as u32) & 0x3)
}

const fn with_phy_tone_path1_selector(value: u32, selector: i32) -> u32 {
    (value & !0xc) | (((selector as u32).wrapping_shl(2)) & 0xc)
}

const fn with_phy_txiq_calibration_enabled(value: u32) -> u32 {
    (value & !0x0000_4000) | 0x0000_2000
}

const fn with_phy_txiq_calibration_complete(value: u32) -> u32 {
    value | 0x0000_4000
}

const fn with_phy_txiq_first_polarity(
    value: u32,
    polarity: bool,
    attenuation: u8,
    selector: u16,
) -> u32 {
    let encoded = (((attenuation as u32).wrapping_mul(0xffff_fc00)) & 0x0003_fc00)
        | ((selector as u32) >> 2)
        | ((polarity as u32) << 26);
    (value & 0xf000_0000) | ((encoded & 0x0fff_ffff) | 0x002c_0000)
}

const fn with_phy_txiq_second_polarity(value: u32, polarity: bool) -> u32 {
    let polarity = polarity as u32;
    (value & 0xf0ff_ffff) | ((((!polarity) & 1) | ((polarity & 1) << 3)) << 24)
}

const fn with_phy_txiq_gain(value: u32, coefficient: i8) -> u32 {
    (value & 0xffff_ffc0) | (coefficient as u8 as u32 & 0x3f)
}

const fn with_phy_txiq_phase(value: u32, coefficient: i8) -> u32 {
    (value & 0xffff_e03f) | ((coefficient as u8 as u32 & 0x7f) << 6)
}

const fn with_phy_rxiq_gain(value: u32, coefficient: i8) -> u32 {
    (value & !0x003f_0000) | ((coefficient as u8 as u32 & 0x3f) << 16)
}

const fn with_phy_rxiq_phase(value: u32, coefficient: i8) -> u32 {
    (value & !0x1fc0_0000) | ((coefficient as u8 as u32 & 0x7f) << 22)
}

const fn with_phy_rxiq_calibration_mode(value: u32) -> u32 {
    (value & !0x4000_0000) | 0x2000_0000
}

const fn with_phy_rxiq_root_correction_begin(value: u32) -> u32 {
    (value & !0x4000_0000) | 0x2000_0000
}

const fn with_phy_rxiq_root_aux_begin(value: u32) -> u32 {
    (value & !0x0000_4000) | 0x0000_2000
}

const fn without_phy_tx_gain_compensation_low_byte(value: u32) -> u32 {
    value & 0xffff_ff00
}

const fn with_phy_tx_gain_compensation_byte1(value: u32) -> u32 {
    (value & 0xffff_00ff) | 0x0000_fa00
}

const fn with_phy_tx_gain_compensation_byte2(value: u32) -> u32 {
    value | 0x00ff_0000
}

const fn without_phy_tx_gain_compensation_high_byte(value: u32) -> u32 {
    value & 0x00ff_ffff
}

const fn with_phy_i2c_clock_selection_high(value: u32, selection: u32) -> u32 {
    (value & !0x0000_07c0) | ((selection << 4) & 0x0000_07c0)
}

const fn with_phy_i2c_clock_selection_low(value: u32, selection: u32) -> u32 {
    (value & !0x0000_003f) | ((selection >> 1) & 0x0000_003f)
}

const fn without_phy_fe_txrx_reset(value: u32) -> u32 {
    value & !0x0600_0000
}

const fn with_phy_fe_txrx_reset(value: u32) -> u32 {
    value | 0x0600_0000
}

const fn with_phy_adc_rate_high(value: u32, rate: u32) -> u32 {
    (value & !0x0000_0002) | ((rate << 1) & 0x0000_0002)
}

const fn with_phy_adc_rate_low(value: u32, rate: u32) -> u32 {
    (value & !0x0000_0001) | (rate & 0x0000_0001)
}

const fn with_register_bits(value: u32, bits: u32) -> u32 {
    value | bits
}

const fn with_phy_front_end_update_first(value: u32) -> u32 {
    with_register_bits(value, 0x0200_0000)
}

const fn with_phy_front_end_update_second(value: u32) -> u32 {
    with_register_bits(value, 0x0400_0000)
}

const fn with_phy_front_end_adc_update(value: u32) -> u32 {
    with_register_bits(value, 0x0000_0003)
}

const fn without_register_bits(value: u32, bits: u32) -> u32 {
    value & !bits
}

const fn with_register_field(value: u32, mask: u32, field: u32) -> u32 {
    (value & !mask) | (field & mask)
}

const fn tx_baseband_gain_index(gain: u16) -> usize {
    match gain {
        0x0080 => 1,
        0x0100 => 2,
        0x0020 => 3,
        0x00a0 => 4,
        _ => 0,
    }
}

const fn encode_phy_gain_memory_words(
    gain_72: u16,
    gain_64: u16,
    gain_32: u8,
    seed_0: u16,
    seed_1: u16,
    seed_2: u16,
    seed_3: u16,
    config: u16,
) -> (u32, u32, u32) {
    let gain_72 = gain_72 as u32;
    let gain_64 = gain_64 as u32;
    let word_0 = ((config & 0x1fff) as u32)
        | ((seed_2 as u32) << 22)
        | ((seed_1 as u32) << 31)
        | ((seed_3 as u32) << 13);
    let word_1 = ((seed_0 as u32) << 8)
        | ((seed_1 as u32) >> 1)
        | (((gain_64 >> 6) & 0xff) << 17)
        | ((gain_72 & 7) << 31)
        | ((gain_64 & 0x3f) << 20)
        | 0x1000_0000;
    let word_2 =
        ((gain_72 & 7) >> 1) | ((gain_72 >> 1) & 0x1c) | ((gain_32 as u32) << 15) | 0x0000_7f80;
    (word_0, word_1, word_2)
}

#[cfg(any(target_arch = "riscv32", test))]
const fn packed_halfword(words: &[u32], index: usize) -> u16 {
    (words[index >> 1] >> ((index & 1) * 16)) as u16
}

#[cfg(any(target_arch = "riscv32", test))]
const fn packed_byte(words: &[u32], index: usize) -> u8 {
    (words[index >> 2] >> ((index & 3) * 8)) as u8
}

#[cfg(any(target_arch = "riscv32", test))]
fn tx_gain_seed_halfword(image: &crate::phy_channel::PhyWifiTxGainImage, index: usize) -> u16 {
    if index < image.seed.len() * 2 {
        packed_halfword(&image.seed, index)
    } else {
        packed_halfword(&image.output_32, index - image.seed.len() * 2)
    }
}

/// Read one of the two MAC TSF domains through the hardware latch.
///
/// Reference: `esp32s31_rev0_rom.elf`, SHA-256
/// `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
/// `hal_get_tsf_time` at `0x2f82b9f8`, size `0x3e`.
///
/// The meaning of the three registers is inferred from the complete ROM body:
/// setting control bit zero or one latches the selected domain, the ROM reads
/// high then low, and clearing the same bit releases the latch. No polling,
/// delay, call, allocation, or ROM-owned RAM is involved.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_get_tsf_time(interface: u32) -> u64 {
    let mask = tsf_latch_mask(interface);
    let control = TSF_CONTROL_ADDRESS as *mut u32;
    control.write_volatile(control.read_volatile() | mask);

    // Preserve the ROM read order while returning the standard RV32 u64 ABI:
    // low in a0 and high in a1.
    let high = (TSF_HIGH_ADDRESS as *const u32).read_volatile();
    let low = (TSF_LOW_ADDRESS as *const u32).read_volatile();

    control.write_volatile(control.read_volatile() & !mask);
    (u64::from(high) << 32) | u64::from(low)
}

/// Read the MAC's last completed RX descriptor address.
///
/// Reference: `esp32s31_rev0_rom.elf`, SHA-256
/// `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
/// `hal_mac_rx_get_last_dscr` at `0x2f8386a2`, size `0x1e`.
///
/// The complete ROM body reads the low-address register first, keeps its low
/// 20 bits, reads the high-address register, keeps its high 12 bits, and joins
/// the two fields. It has no call, cycle, wait, allocation, or ROM-owned RAM
/// access. Preserve that read order because the hardware publication contract
/// beyond the recovered two-register snapshot is not yet known.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_mac_rx_get_last_dscr() -> *mut u8 {
    let low_word = (RX_DESCRIPTOR_LAST_LOW_ADDRESS as *const u32).read_volatile();
    let high_word = (RX_DESCRIPTOR_LAST_HIGH_ADDRESS as *const u32).read_volatile();
    join_rx_descriptor_address(low_word, high_word) as *mut u8
}

/// Program one of the four recovered MAC-address register pairs.
///
/// References: pinned `libpp.a[if_hwctrl.o]::ic_set_mac`, an exact tail call,
/// and `libpp.a[hal_mac.o]::hal_mac_set_addr`, size `0x48`. The complete leaf
/// packs the six input bytes little-endian, writes the low four bytes first,
/// writes the high two bytes, then sets bit 16 in the high register through a
/// fresh read/modify/write. No C/ROM-owned state, call, loop, wait, delay or
/// allocation remains. The meaning of high-register bit 16 is inferred only
/// as address-valid from that transaction.
///
/// The archive callers are interface setup paths, not radio interrupt
/// handlers. This leaf therefore remains flash-mapped so it does not consume
/// the interrupt-only SRAM reserve.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_ic_set_mac(interface: u32, address: *const u8) {
    if interface >= MAC_INTERFACE_COUNT || address.is_null() {
        core::arch::asm!("ebreak", options(noreturn));
    }

    let bytes = [
        address.read(),
        address.add(1).read(),
        address.add(2).read(),
        address.add(3).read(),
        address.add(4).read(),
        address.add(5).read(),
    ];
    let (low_word, high_word) = encode_mac_address(bytes);
    let (low_address, high_address) = mac_address_registers(interface);
    (low_address as *mut u32).write_volatile(low_word);

    let high = high_address as *mut u32;
    high.write_volatile(high_word & !MAC_ADDRESS_VALID_BIT);
    high.write_volatile(high.read_volatile() | MAC_ADDRESS_VALID_BIT);
}

/// Program the recovered RX frame/control/management policy for one queue.
///
/// References: pinned `libpp.a[if_hwctrl.o]::ic_set_rx_policy`, size `0x14`,
/// and `libpp.a[hal_mac.o]::hal_mac_rx_set_policy`, size `0xd2`.
/// The wrapper accepts queues 0..=2 and returns one after the finite MMIO
/// transaction. Register field names describe only the vendor arguments and
/// exact masks; broader MAC semantics are not assumed.
///
/// The evidenced callers configure scan/supplicant state from the radio
/// executor, not an interrupt. Keep this leaf flash-mapped.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_ic_set_rx_policy(
    queue: u32,
    mode: u32,
    control: u32,
    management: u32,
) -> u32 {
    if queue >= MAC_RX_POLICY_QUEUE_COUNT {
        return 1;
    }

    let frame_policy = mac_rx_frame_policy_address(queue) as *mut u32;
    frame_policy.write_volatile(with_mac_rx_mode(frame_policy.read_volatile(), mode));

    let address_policy = mac_rx_address_policy_address(queue) as *mut u32;
    if queue == 1 {
        // The pinned body sets bit 30 only for queue one before applying the
        // shared control-address policy below.
        address_policy.write_volatile(address_policy.read_volatile() | (1 << 30));
    } else {
        address_policy.write_volatile(address_policy.read_volatile() & !(1 << 30));
    }

    frame_policy.write_volatile(with_mac_rx_control_policy(
        frame_policy.read_volatile(),
        control,
    ));
    if control <= 1 {
        address_policy.write_volatile(with_mac_rx_control_address_policy(
            address_policy.read_volatile(),
            control,
        ));
    }

    let management_policy = mac_rx_management_policy_address(queue) as *mut u32;
    management_policy.write_volatile(with_mac_rx_management_policy(
        management_policy.read_volatile(),
        management,
    ));
    1
}

/// Enable or disable the recovered unique-BSSID checks for one RX queue.
///
/// References: pinned
/// `libpp.a[if_hwctrl.o]::ic_set_rx_policy_ubssid_check`, size `0x1e`, and
/// `libpp.a[hal_mac.o]::hal_mac_set_rxq_policy`, size `0x2c`. The vendor
/// wrapper admits queues 0..=3, returns zero outside that range, and otherwise
/// returns one after two ordered read/modify/write operations.
///
/// The evidenced caller is the same non-interrupt policy setup path, so this
/// leaf is flash-mapped.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_ic_set_rx_policy_ubssid_check(
    queue: u32,
    enabled: u32,
) -> u32 {
    if queue >= MAC_INTERFACE_COUNT {
        return 0;
    }

    let policy = mac_rx_frame_policy_address(queue) as *mut u32;
    if enabled == 0 {
        policy.write_volatile(policy.read_volatile() & !(1 << 8));
        policy.write_volatile(policy.read_volatile() & !(1 << 1));
    } else {
        policy.write_volatile(policy.read_volatile() | (1 << 8));
        policy.write_volatile(policy.read_volatile() | (1 << 1));
    }
    1
}

/// Select the two-bit MAC clear-channel-assessment mode.
///
/// Reference: pinned `libpp.a[hal_mac.o]::hal_mac_tx_set_cca`, size `0x18`.
/// The complete body replaces bits 31:30 of `0x2010_4c5c` and returns zero.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_mac_tx_set_cca(cca: u32) -> u32 {
    let control = TX_CCA_CONTROL_ADDRESS as *mut u32;
    control.write_volatile(with_tx_cca(control.read_volatile(), cca));
    0
}

/// Return the recovered TX queue valid bit.
///
/// Reference: pinned `libpp.a[hal_mac.o]::hal_mac_is_txq_valid`, size `0x14`.
/// Queue zero starts at `0x2010_4d70`; successive queue registers descend by
/// 16 bytes. The result is register bit 30 normalized to zero or one.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_mac_is_txq_valid(queue: u8) -> u32 {
    let control = tx_queue_control_address(queue) as *const u32;
    tx_queue_is_valid(control.read_volatile())
}

/// Clear only the recovered TX queue valid bit.
///
/// Reference: pinned `libpp.a[hal_mac.o]::hal_mac_set_txq_invalid`, size
/// `0x1c`. The body is one finite read/modify/write of register bit 30.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_mac_set_txq_invalid(queue: u8) {
    let control = tx_queue_control_address(queue) as *mut u32;
    control.write_volatile(without_tx_queue_valid(control.read_volatile()));
}

/// Clear both recovered TX queue control bits.
///
/// Reference: pinned `libpp.a[hal_mac_tx.o]::hal_mac_txq_disable`, size
/// `0x18`. The body is one finite read/modify/write of bits 31:30.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_mac_txq_disable(queue: u8) {
    let control = tx_queue_control_address(queue) as *mut u32;
    control.write_volatile(without_tx_queue_enable(control.read_volatile()));
}

/// Preserve the ESP32-S31 CSI bandwidth hook's explicit no-op contract.
///
/// The complete pinned `libpp.a[hal_mac_ctl.o]::hal_mac_set_csi_cbw` body is
/// one two-byte `ret`; it ignores its argument and owns no state.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_hal_mac_set_csi_cbw(_cbw: u32) {}

/// Restart the MAC for the strict `WIFI_PS_NONE` profile.
///
/// The complete pinned chain is
/// `libpp.a[if_hwctrl.o]::ic_mac_init` (40 bytes),
/// `libpp.a[hal_mac.o]::hal_mac_init` (48 bytes), and
/// `libpp.a[hal_pwr.o]::pwr_hal_select_wifimac_regdma_link` (32 bytes).
/// With power save disabled, `pm_get_tx_blocks_retention_mask` returns all
/// ones, so the first read/modify/write clears `0x00ff_1000` at
/// `0x2010_4cac`. The second selects evidenced REGDMA link four in bits
/// 20:17 of `0x2010_d83c`.
///
/// The vendor tail also writes one to
/// `g_wifimac_regdma_link_selected`. That byte is a cache for the vendor PM
/// getters; every strict PM hook is disabled under the read-back-verified
/// `WIFI_PS_NONE` invariant, so publishing it would retain hidden C state
/// without a strict consumer.
///
/// This finite leaf contains no call, loop, wait, allocation, or non-MMIO
/// state. The surrounding Rust channel state machine owns serialization.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub(crate) unsafe fn restart_mac_without_power_save() {
    let mac_control = MAC_CONTROL_ADDRESS as *mut u32;
    mac_control.write_volatile(without_mac_tx_retention(mac_control.read_volatile()));

    let regdma_control = WIFI_MAC_REGDMA_CONTROL_ADDRESS as *mut u32;
    regdma_control.write_volatile(with_wifi_mac_regdma_link(
        regdma_control.read_volatile(),
        WIFI_MAC_ACTIVE_REGDMA_LINK,
    ));
}

/// Close the recovered front-end and baseband clock gates.
///
/// Reference: the complete pinned
/// `libphy.a[phy_init.o]::phy_close_fe_bb_clk` body, size `0x20`. It writes
/// zero to `0x2010_0400`, clears bits 1:0 of `0x2010_0800`, then writes zero
/// to `0x2010_7c80`. The field names are retained from the vendor symbol; no
/// broader register meaning is assumed. There is no call, loop, wait,
/// allocation, or non-MMIO state access.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.radio_hal"]
pub unsafe extern "C" fn wifi_strict_phy_close_fe_bb_clk() {
    (phy_clock_oracle::FE_CLOCK_GATE_OPAQUE.address() as *mut u32).write_volatile(0);

    let control = phy_clock_oracle::FE_BB_CLOCK_CONTROL_OPAQUE.address() as *mut u32;
    control.write_volatile(without_fe_bb_clock_enable(control.read_volatile()));

    (phy_clock_oracle::BB_CLOCK_GATE_OPAQUE.address() as *mut u32).write_volatile(0);
}

/// Open the recovered front-end and baseband clock gates.
///
/// Reference: complete rev0 ROM `phy_open_fe_bb_clk` body at `0x2f82_3ec0`,
/// size `0x38`. The four writes retain their exact order and the two
/// read/modify/write operations use fresh volatile reads. PMU bit 22 is named
/// by the S31 PMU description; the low four PMU bits and the three PHY gate
/// registers remain explicitly opaque rather than borrowing neighboring-chip
/// names.
///
/// This cold-init leaf has no call, branch, loop, wait, allocation, callback,
/// or non-MMIO state access.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_phy_open_fe_bb_clk() {
    (phy_clock_oracle::FE_CLOCK_GATE_OPAQUE.address() as *mut u32).write_volatile(0x1e7);

    let control = phy_clock_oracle::FE_BB_CLOCK_CONTROL_OPAQUE.address() as *mut u32;
    control.write_volatile(control.read_volatile() | PHY_FE_BB_ENABLE_MASK);

    (phy_clock_oracle::BB_CLOCK_GATE_OPAQUE.address() as *mut u32).write_volatile(u32::MAX);

    let active_clock_power = pmu::HP_ACTIVE_HP_CK_POWER.address() as *mut u32;
    active_clock_power.write_volatile(active_clock_power.read_volatile() | ROM_OPEN_FE_BB_PMU_MASK);
}

/// Read the exact PBus field consumed by RX-DCO calibration.
///
/// The rev0 ROM chain
/// `phy_pbus_rd(1, 2) -> phy_pbus_rd_addr/phy_pbus_rd_shift` resolves to one
/// volatile read at `0x2010_0894`, shift zero, masked to nine bits. The jump
/// tables are present in `esp32s31_rev0_rom.elf` at `0x2f84_d910` and
/// `0x2f84_d924`. This Rust leaf has no call, branch, loop, wait, allocation,
/// callback, or non-MMIO state access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_pbus_rx_dco_value() -> u16 {
    phy_pbus_rx_dco_read_value((PHY_PBUS_RX_DCO_READ_ADDRESS as *const u32).read_volatile())
}

/// Sample the temperature-sensor code exactly once.
///
/// Readiness and PHY-I2C range selection belong to the caller-driven
/// temperature transition. This leaf has no loop, delay, or hidden state.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_temperature_code() -> u32 {
    (crate::phy_temperature::PHY_TEMPERATURE_CODE_ADDRESS as *const u32).read_volatile()
        & crate::phy_temperature::PHY_TEMPERATURE_CODE_MASK
}

/// Sample the free-running counter used by the ROM SDM-stability deadline.
///
/// Complete rev0 ROM `phy_wait_i2c_sdm_stable` at `0x2f82_3e76` samples
/// `0x2010_d800` before and after each independently completed PHY-I2C read.
/// This leaf performs exactly one volatile read; deadline ownership and
/// wraparound arithmetic stay in the Rust cold-init state machine.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_sdm_cycle_counter() -> u32 {
    (PHY_SDM_CYCLE_COUNTER_ADDRESS as *const u32).read_volatile()
}

/// Select the two recovered TX-clock enable bits.
///
/// Reference: complete rev0 ROM `phy_set_txclk_en` at `0x2f82_7cd2`, size
/// `0x24`. It performs one read/modify/write of bits 17:16 at
/// `0x2010_0890`. There is no call, loop, wait, allocation, callback, or
/// ROM-owned data access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_tx_clock(enabled: bool) {
    let control = PHY_CLOCK_CONTROL_ADDRESS as *mut u32;
    control.write_volatile(with_phy_tx_clock(control.read_volatile(), enabled));
}

/// Select the two recovered RX-clock enable bits.
///
/// Reference: complete rev0 ROM `phy_set_rxclk_en` at `0x2f82_7cf6`, size
/// `0x20`. It performs one read/modify/write of bits 15:14 at
/// `0x2010_0890`. There is no call, loop, wait, allocation, callback, or
/// ROM-owned data access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rx_clock(enabled: bool) {
    let control = PHY_CLOCK_CONTROL_ADDRESS as *mut u32;
    control.write_volatile(with_phy_rx_clock(control.read_volatile(), enabled));
}

/// Capture and clear one recovered PHY register field.
///
/// This is intentionally a narrow cold-calibration helper rather than a
/// general register API. Callers retain the exact address/mask pair in their
/// non-cloneable action token and restore it before publishing an outcome.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn capture_and_clear_phy_register_field(address: usize, field_mask: u32) -> u32 {
    debug_assert!(
        address == crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_ADDRESS
            && field_mask == crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_FIELD_MASK
    );
    let register = address as *mut u32;
    let current = register.read_volatile();
    register.write_volatile(current & !field_mask);
    current & field_mask
}

/// Restore a field captured by [`capture_and_clear_phy_register_field`].
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn restore_phy_register_field(address: usize, field_mask: u32, saved_field: u32) {
    debug_assert!(
        address == crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_ADDRESS
            && field_mask == crate::phy_rx_gain_cal::PHY_RX_DC_CONTROL_FIELD_MASK
    );
    let register = address as *mut u32;
    register.write_volatile((register.read_volatile() & !field_mask) | (saved_field & field_mask));
}

/// Apply the complete direct-register prefix/suffix of ROM
/// `phy_set_rx_gain_cal_dc`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rx_gain_dc_registers(enabled: bool) {
    if enabled {
        set_register_bits(
            phy_clock_oracle::FE_BB_CLOCK_CONTROL_OPAQUE.address(),
            PHY_CALIBRATION_CLOCK_MASK,
        );
        set_register_bits(PHY_TONE_SELECTOR_CONTROL_ADDRESS - 4, 0x60);
    } else {
        clear_register_bits(PHY_TONE_SELECTOR_CONTROL_ADDRESS - 4, 0x60);
    }
}

/// Read one nine-bit PBus field without invoking ROM.
///
/// Complete ROM `phy_pbus_rd` is pure address/shift selection followed by a
/// single volatile read. The tuple table below is recovered from its two ROM
/// jump tables at `0x2f84_d910` and `0x2f84_d924`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_pbus_field(selector: u8, path: u8) -> u16 {
    debug_assert!(selector <= 5, "unrecovered PBus selector");
    let address = phy_pbus_read_address(selector, path);
    let shift = phy_pbus_read_shift(selector, path);
    (((address as *const u32).read_volatile() >> shift) & 0x1ff) as u16
}

/// Program the complete crystal-duty calibration tone without `g_phyFuns`.
///
/// Primary reference: pinned
/// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new`, size `0xc2`, together
/// with its `g_phyFuns + 0x30` target
/// `phy_txgain_comp_pacfg_new`, size `0x54`.
///
/// The calibration caller supplies only the three nonzero-capable arguments;
/// the second path is zero in both evidenced calls. `enabled=true` reproduces
/// `(1, 0x80, 0, 0, 0, 0)`, while `enabled=false` reproduces
/// `(0, 0x80, 0x28, 0, 0, 0)`. Every fresh volatile read and intermediate
/// write is retained because the registers are hardware state. There is no
/// callback, loop, wait, allocation, or software-global access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_calibration_tone(enabled: bool, selector: u8, step: u8) {
    configure_phy_calibration_tone_wide(enabled, selector as u16, step);
}

/// Wide-selector form used by TX-DC calibration, whose evidenced selector is
/// 600 and therefore cannot be represented by the older `u8` child actions.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub(crate) unsafe fn configure_phy_calibration_tone_wide(enabled: bool, selector: u16, step: u8) {
    let compensation = PHY_TX_GAIN_COMPENSATION_CONTROL_ADDRESS as *mut u32;
    let compensation_aux = PHY_TX_GAIN_COMPENSATION_AUX_ADDRESS as *mut u32;

    // Exact `configure_tx_gain_compensation(0)` callback body.
    compensation.write_volatile(0);
    compensation_aux.write_volatile(0);

    let selectors = PHY_TONE_SELECTOR_CONTROL_ADDRESS as *mut u32;
    selectors.write_volatile(with_phy_tone_path0_selector(
        selectors.read_volatile(),
        i32::from(selector),
    ));
    selectors.write_volatile(with_phy_tone_path1_selector(selectors.read_volatile(), 0));

    let path0 = PHY_TONE_PATH0_CONTROL_ADDRESS as *mut u32;
    path0.write_volatile(with_phy_tone_path(
        path0.read_volatile(),
        enabled as i32,
        i32::from(selector),
        i32::from(step),
    ));

    let path1 = PHY_TONE_PATH1_CONTROL_ADDRESS as *mut u32;
    path1.write_volatile(with_phy_tone_path(path1.read_volatile(), 0, 0, 0));

    // Exact `configure_tx_gain_compensation(1)` callback body. Preserve the
    // four writes rather than collapsing their final value.
    compensation.write_volatile(without_phy_tx_gain_compensation_low_byte(
        compensation.read_volatile(),
    ));
    compensation.write_volatile(with_phy_tx_gain_compensation_byte1(
        compensation.read_volatile(),
    ));
    compensation.write_volatile(with_phy_tx_gain_compensation_byte2(
        compensation.read_volatile(),
    ));
    compensation.write_volatile(without_phy_tx_gain_compensation_high_byte(
        compensation.read_volatile(),
    ));
}

/// Enter or leave the TX-IQ coefficient calibration register mode.
///
/// Reference: complete ROM `phy_rfcal_txiq` prefix and suffix. Each branch is
/// one finite read/modify/write and owns no software state.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_txiq_correction(begin: bool) {
    let control = PHY_FE_CONTROL_0C0C_ADDRESS as *mut u32;
    let value = control.read_volatile();
    control.write_volatile(if begin {
        with_phy_txiq_calibration_enabled(value)
    } else {
        with_phy_txiq_calibration_complete(value)
    });
}

/// Capture the complete tone-control word saved by ROM `phy_rfcal_txiq`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_txiq_tone_control() -> u32 {
    (PHY_TONE_PATH0_CONTROL_ADDRESS as *const u32).read_volatile()
}

/// Restore the exact tone-control word after TX-IQ work-mode cleanup.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn restore_phy_txiq_tone_control(saved: u32) {
    (PHY_TONE_PATH0_CONTROL_ADDRESS as *mut u32).write_volatile(saved);
}

/// Configure one of the two mismatch-power polarities.
///
/// The first branch reproduces both writes at the head of
/// `phy_txiq_get_mis_pwr`; the second branch changes only bits 27:24 after
/// the first linear-power sample. The two-microsecond intervals remain
/// separate async actions in `phy_txiq`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_txiq_mis_power(
    first: bool,
    polarity: bool,
    attenuation: u8,
    selector: u16,
) {
    let tone = PHY_TONE_PATH0_CONTROL_ADDRESS as *mut u32;
    if first {
        tone.write_volatile(with_phy_txiq_first_polarity(
            tone.read_volatile(),
            polarity,
            attenuation,
            selector,
        ));
        let selectors = PHY_TONE_SELECTOR_CONTROL_ADDRESS as *mut u32;
        selectors.write_volatile(with_phy_tone_path0_selector(
            selectors.read_volatile(),
            i32::from(selector),
        ));
    } else {
        tone.write_volatile(with_phy_txiq_second_polarity(
            tone.read_volatile(),
            polarity,
        ));
    }
}

/// Publish one bounded TX-IQ gain or phase coefficient.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_txiq_coefficient(
    kind: crate::phy_txiq::PhyTxIqCoefficientKind,
    value: i8,
) {
    let control = PHY_FE_CONTROL_0C0C_ADDRESS as *mut u32;
    let current = control.read_volatile();
    control.write_volatile(match kind {
        crate::phy_txiq::PhyTxIqCoefficientKind::Gain => with_phy_txiq_gain(current, value),
        crate::phy_txiq::PhyTxIqCoefficientKind::Phase => with_phy_txiq_phase(current, value),
    });
}

/// Publish one bounded RX-IQ gain or phase coefficient.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rxiq_coefficient(
    kind: crate::phy_rxiq::PhyRxIqCoefficientKind,
    value: i8,
) {
    let control = PHY_IQ_CORRECTION_CONTROL_ADDRESS as *mut u32;
    let current = control.read_volatile();
    control.write_volatile(match kind {
        crate::phy_rxiq::PhyRxIqCoefficientKind::Gain => with_phy_rxiq_gain(current, value),
        crate::phy_rxiq::PhyRxIqCoefficientKind::Phase => with_phy_rxiq_phase(current, value),
    });
}

/// Select the finite correction path at entry to ROM `phy_rfcal_rxiq`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rxiq_calibration_mode() {
    let control = PHY_IQ_CORRECTION_CONTROL_ADDRESS as *mut u32;
    control.write_volatile(with_phy_rxiq_calibration_mode(control.read_volatile()));
}

/// Preserve the two fresh-read status publications at RXIQ root entry.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rxiq_root_status() {
    let status = PHY_PBUS_STATUS_ADDRESS as *mut u32;
    status.write_volatile(status.read_volatile() | 0x0000_4000);
    status.write_volatile(status.read_volatile() | 0x0000_8000);
}

/// Enter or leave the correction mode owned by archive `phy_rxiq_cal_init`.
///
/// The finish branch retains all four original fresh-read writes, including
/// clearing status bit 15 only after correction fields are restored.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rxiq_root_correction(begin: bool) {
    let correction = PHY_IQ_CORRECTION_CONTROL_ADDRESS as *mut u32;
    let auxiliary = PHY_IQ_CORRECTION_AUX_ADDRESS as *mut u32;
    if begin {
        correction.write_volatile(with_phy_rxiq_root_correction_begin(
            correction.read_volatile(),
        ));
        auxiliary.write_volatile(with_phy_rxiq_root_aux_begin(auxiliary.read_volatile()));
    } else {
        correction.write_volatile(correction.read_volatile() | 0x4000_0000);
        auxiliary.write_volatile(auxiliary.read_volatile() | 0x0000_4000);
        correction.write_volatile(correction.read_volatile() & 0xdfff_ffff);
        let status = PHY_PBUS_STATUS_ADDRESS as *mut u32;
        status.write_volatile(status.read_volatile() & 0xffff_7fff);
    }
}

/// Save and clear bits 23:22 around crystal-duty RX-DCO calibration.
///
/// Reference: pinned `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal` offsets
/// `0x3c..0xfc`. The returned value contains only the two owned field bits;
/// the leaf performs one finite read/modify/write and owns no software state.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn mask_phy_rx_dco_control_field() -> u32 {
    let control = PHY_RX_DCO_CONTROL_ADDRESS as *mut u32;
    let previous = control.read_volatile();
    control.write_volatile(without_phy_rx_dco_control_field(previous));
    previous & 0x00c0_0000
}

/// Restore the saved bits 23:22 without replacing concurrently unrelated
/// register fields.
///
/// Reference: pinned `phy_xtal_duty_cal` offsets `0x114..0x126`. There is no
/// loop, wait, allocation, callback, or mutable software state.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn restore_phy_rx_dco_control_field(saved_field: u32) {
    let control = PHY_RX_DCO_CONTROL_ADDRESS as *mut u32;
    control.write_volatile(with_restored_phy_rx_dco_control_field(
        control.read_volatile(),
        saved_field,
    ));
}

/// Apply the complete rev0 ROM `phy_i2c_clk_sel` register transform.
///
/// The pinned body at `0x2f82_9f1c`, size `0x68`, updates the high field and
/// then the low field separately in each of three consecutive registers.
/// Keeping the six writes preserves the observed intermediate hardware
/// states; this leaf contains no wait, branch, call, or mutable software
/// state.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_i2c_clock_selection(selection: u32) {
    unsafe fn configure_register(address: usize, selection: u32) {
        let register = address as *mut u32;
        register.write_volatile(with_phy_i2c_clock_selection_high(
            register.read_volatile(),
            selection,
        ));
        register.write_volatile(with_phy_i2c_clock_selection_low(
            register.read_volatile(),
            selection,
        ));
    }

    configure_register(PHY_I2C_CLOCK_SELECTION_0_ADDRESS, selection);
    configure_register(PHY_I2C_CLOCK_SELECTION_1_ADDRESS, selection);
    configure_register(PHY_I2C_CLOCK_SELECTION_2_ADDRESS, selection);
}

/// Apply the complete rev0 ROM `phy_fe_txrx_reset` pulse.
///
/// The pinned body at `0x2f82_788c`, size `0x24`, ignores its argument,
/// clears bits 25 and 26 at `0x2010_0440`, then sets both bits. There is no
/// delay or status observation between the two writes.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_fe_txrx_reset() {
    let register = PHY_FE_TXRX_RESET_ADDRESS as *mut u32;
    register.write_volatile(without_phy_fe_txrx_reset(register.read_volatile()));
    register.write_volatile(with_phy_fe_txrx_reset(register.read_volatile()));
}

/// Apply the finite MMIO suffix of rev0 ROM `phy_adc_rate_set`.
///
/// The complete parent action performs its masked PHY-I2C transaction first.
/// This leaf preserves the following two fresh-read writes to `0x2010_0448`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_adc_rate(rate: u32) {
    let register = PHY_ADC_RATE_ADDRESS as *mut u32;
    register.write_volatile(with_phy_adc_rate_high(register.read_volatile(), rate));
    register.write_volatile(with_phy_adc_rate_low(register.read_volatile(), rate));
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn set_register_bits(address: usize, bits: u32) {
    let register = address as *mut u32;
    register.write_volatile(with_register_bits(register.read_volatile(), bits));
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn clear_register_bits(address: usize, bits: u32) {
    let register = address as *mut u32;
    register.write_volatile(without_register_bits(register.read_volatile(), bits));
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn replace_register_field(address: usize, mask: u32, field: u32) {
    let register = address as *mut u32;
    register.write_volatile(with_register_field(register.read_volatile(), mask, field));
}

/// Apply complete rev0 ROM `phy_fe_reg_init`.
///
/// The pinned body at `0x2f82_7740`, size `0xf6`, is a finite sequence of
/// seventeen MMIO writes. Calls below are deliberately unrolled and retain
/// repeated fresh-read writes to the same register. There is no wait, delay,
/// loop, callback, or mutable software-state access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_front_end_registers(registers: &mut RadioRegisters) {
    set_register_bits(PHY_FE_CONTROL_0894_ADDRESS, 0x0010_0000);
    set_register_bits(PHY_FE_CONTROL_0C08_ADDRESS, 0x0200_0000);
    set_register_bits(PHY_FE_CONTROL_0C08_ADDRESS, 0x0400_0000);
    clear_register_bits(PHY_FE_CONTROL_0444_ADDRESS, 0x0000_0100);
    open_esp_radio_hal_esp32s31::phy_memory::configure_table_memory_base_index(registers, 0xa0);
    set_register_bits(PHY_FE_CONTROL_040C_ADDRESS, 0x0000_0004);
    set_register_bits(PHY_FE_CONTROL_0438_ADDRESS, 0x6000_0000);
    set_register_bits(PHY_FE_CONTROL_0C0C_ADDRESS, 0x0000_6000);
    clear_register_bits(PHY_FE_CONTROL_0444_ADDRESS, 0x0000_0800);
    set_register_bits(PHY_FE_CONTROL_0448_ADDRESS, 0x0000_0002);
    set_register_bits(PHY_FE_CONTROL_0448_ADDRESS, 0x0000_0001);
    replace_register_field(PHY_FE_CONTROL_086C_ADDRESS, 0x0000_00ff, 0x0000_0004);
    set_register_bits(PHY_FE_CONTROL_0448_ADDRESS, 0x0000_0001);
    set_register_bits(PHY_FE_CONTROL_0448_ADDRESS, 0x0000_0002);
    set_register_bits(PHY_FE_CONTROL_0438_ADDRESS, 0x8000_0000);
    set_register_bits(PHY_FE_CONTROL_0C0C_ADDRESS, 0x0000_8000);
    replace_register_field(PHY_FE_CONTROL_0C20_ADDRESS, 0x0000_00ff, 0x0000_0057);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
///
/// The 0x32-byte archive body used by `phy_rf_init` is smaller than the
/// similarly named ROM function: it performs exactly three fresh-read MMIO
/// updates and returns. In particular, this call site does not include the ROM
/// tail-call to `phy_dac_scale_set`. There is no loop, delay, callback, or
/// mutable software-state access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_front_end_update() {
    let front_end = PHY_FE_CONTROL_0C08_ADDRESS as *mut u32;
    front_end.write_volatile(with_phy_front_end_update_first(front_end.read_volatile()));
    front_end.write_volatile(with_phy_front_end_update_second(front_end.read_volatile()));

    let adc = PHY_FE_CONTROL_0448_ADDRESS as *mut u32;
    adc.write_volatile(with_phy_front_end_adc_update(adc.read_volatile()));
}

/// Apply complete vendor `phy_tsens_read_init` and its ROM tail leaf.
///
/// The pinned 0x36-byte archive body ignores both ABI arguments, performs
/// four MMIO writes, forces power argument one, and tail-calls the complete
/// 0x1c-byte ROM `phy_set_tsens_power_` body. Rust therefore needs no
/// `phy_param[0x16]` input.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_temperature_sensor_read() {
    set_register_bits(PHY_TEMPERATURE_SENSOR_CONTROL_ADDRESS, 0x0000_0001);
    set_register_bits(PHY_TEMPERATURE_SENSOR_SYSTEM_CONTROL_ADDRESS, 0x4000_0000);
    set_register_bits(PHY_TEMPERATURE_SENSOR_CONTROL_ADDRESS, 0x0080_0000);
    set_register_bits(PHY_TEMPERATURE_SENSOR_CONTROL_ADDRESS, 0x0000_0200);
    replace_register_field(
        PHY_TEMPERATURE_SENSOR_POWER_ADDRESS,
        0x0040_0000,
        0x0040_0000,
    );
}

/// Arm one PWDET tone sample before the async one-microsecond timer edge.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn arm_phy_power_detector_tone() {
    set_register_bits(PHY_TONE_PATH0_CONTROL_ADDRESS, 0x0004_0000);
}

/// Clear the temporary tone-arm bit selected by former `phy_param[0x1aa]`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn clear_phy_power_detector_tone_arm() {
    clear_register_bits(PHY_TONE_PATH0_CONTROL_ADDRESS, 0x0004_0000);
}

/// Stop the calibration tone exactly as `phy_stop_tx_tone(1)`.
///
/// This includes the two fresh-read `phy_dac_scale_set(1)` field writes. It
/// is an unconditional cleanup leaf with no wait, branch, callback, or
/// software-global access.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn stop_phy_power_detector_tone() {
    clear_register_bits(PHY_TONE_PATH0_CONTROL_ADDRESS, 0x0004_0000);
    clear_register_bits(PHY_TONE_PATH1_CONTROL_ADDRESS, 0x0004_0000);
    set_register_bits(PHY_TONE_STOP_CONTROL_ADDRESS, 0x0000_0003);
    replace_register_field(PHY_DAC_SCALE_CONTROL_ADDRESS, 0x00ff_0000, 0x00ff_0000);
    replace_register_field(PHY_DAC_SCALE_CONTROL_ADDRESS, 0x0000_ff00, 0x0000_ff00);
}

/// Trigger one TX-DC comparator measurement using the three fresh-read writes
/// at rev0 ROM `phy_txdc_cal+0x9c..=0xbe`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn trigger_phy_tx_dc_measurement() {
    set_register_bits(PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS, 0x0000_0002);
    clear_register_bits(PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS, 0x0000_0001);
    set_register_bits(PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS, 0x0000_0001);
}

/// Read one TX-DC readiness sample. Repetition remains an executor decision.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_tx_dc_ready_status() -> u32 {
    (PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS as *const u32).read_volatile()
}

/// Preserve the two independent post-ready comparator reads from the ROM.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn read_phy_tx_dc_comparator_status() -> [u32; 2] {
    let status = PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS as *const u32;
    [status.read_volatile(), status.read_volatile()]
}

/// Clear the TX-DC measurement controls as two fresh-read writes.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn clear_phy_tx_dc_measurement() {
    clear_register_bits(PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS, 0x0000_0002);
    clear_register_bits(PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS, 0x0000_0001);
}

/// Encode and publish a finite PHY transmit-gain table.
///
/// Reference: pinned
/// `libphy.a[phy_tx_gain.o]::phy_set_tx_gain_mem_new`, size `0x130`, plus the
/// complete rev0 ROM leaves `phy_txbbgain_to_index` at `0x2f826ac8` and
/// `phy_write_gain_mem` at `0x2f8274f0`.
///
/// The vendor body accepts 16 BT or 32 Wi-Fi entries. The open channel path
/// owns and publishes the exact 32-entry Wi-Fi image. Its historical
/// `seed_and_output_32` pointer treated the six seed words and eight packed
/// output words as one contiguous halfword view; Rust models that
/// concatenation explicitly instead of relying on struct layout or pointer
/// arithmetic.
///
/// Every iteration performs three ordinary input reads, selects four
/// halfwords from that contiguous layout, encodes three register words, then
/// publishes the three words through the owned `PHY_MEMORY` HAL. There is no
/// allocation, wait, indirect call, hidden state, raw pointer, or
/// hardware-dependent loop exit.
#[cfg(target_arch = "riscv32")]
pub(crate) fn publish_phy_tx_gain_memory(
    registers: &mut RadioRegisters,
    bank: bool,
    image: crate::phy_channel::PhyWifiTxGainImage,
) {
    let hardware_base =
        open_esp_radio_hal_esp32s31::phy_memory::read_table_memory_base_index(registers);
    let memory_base = hardware_base.wrapping_add(if bank { 32 } else { 0 });
    let mut entry = 0_u8;
    while entry != 32 {
        let entry_index = usize::from(entry);
        let gain_72 = packed_halfword(&image.output_72, entry_index);
        let gain_64 = packed_halfword(&image.output_64, entry_index);
        let gain_32 = packed_byte(&image.output_32, entry_index);
        let seed_index = tx_baseband_gain_index(gain_64) * 4;
        let (word_0, word_1, word_2) = encode_phy_gain_memory_words(
            gain_72,
            gain_64,
            gain_32,
            tx_gain_seed_halfword(&image, seed_index),
            tx_gain_seed_halfword(&image, seed_index + 1),
            tx_gain_seed_halfword(&image, seed_index + 2),
            tx_gain_seed_halfword(&image, seed_index + 3),
            image.config,
        );

        open_esp_radio_hal_esp32s31::phy_memory::program_gain_memory_entry(
            registers,
            [word_0, word_1, word_2],
            memory_base.wrapping_add(entry),
        );
        entry += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        encode_mac_address, encode_phy_gain_memory_words, join_rx_descriptor_address,
        mac_address_registers, mac_rx_address_policy_address, mac_rx_frame_policy_address,
        mac_rx_management_policy_address, packed_byte, packed_halfword, phy_pbus_is_busy,
        phy_pbus_read_address, phy_pbus_read_shift, phy_pbus_rx_dco_read_value, tsf_latch_mask,
        tx_baseband_gain_index, tx_gain_seed_halfword, tx_queue_control_address, tx_queue_is_valid,
        with_mac_rx_control_address_policy, with_mac_rx_control_policy,
        with_mac_rx_management_policy, with_mac_rx_mode, with_mac_rx_unique_bssid_policy,
        with_phy_adc_rate_high, with_phy_adc_rate_low, with_phy_fe_txrx_reset,
        with_phy_front_end_adc_update, with_phy_front_end_update_first,
        with_phy_front_end_update_second, with_phy_i2c_clock_selection_high,
        with_phy_i2c_clock_selection_low, with_phy_pbus_force_test, with_phy_rx_clock,
        with_phy_rxiq_calibration_mode, with_phy_rxiq_gain, with_phy_rxiq_phase,
        with_phy_rxiq_root_aux_begin, with_phy_rxiq_root_correction_begin, with_phy_tone_path,
        with_phy_tone_path0_selector, with_phy_tone_path1_selector, with_phy_tx_clock,
        with_phy_tx_gain_compensation_byte1, with_phy_tx_gain_compensation_byte2,
        with_phy_txiq_calibration_complete, with_phy_txiq_calibration_enabled,
        with_phy_txiq_first_polarity, with_phy_txiq_gain, with_phy_txiq_phase,
        with_phy_txiq_second_polarity, with_register_bits, with_register_field,
        with_restored_phy_rx_dco_control_field, with_tx_cca, with_wifi_mac_regdma_link,
        without_fe_bb_clock_enable, without_mac_tx_retention, without_phy_fe_txrx_reset,
        without_phy_rx_dco_control_field, without_phy_tx_gain_compensation_high_byte,
        without_phy_tx_gain_compensation_low_byte, without_register_bits, without_tx_queue_enable,
        without_tx_queue_valid, WIFI_MAC_ACTIVE_REGDMA_LINK,
    };

    #[test]
    fn selects_the_two_recovered_tsf_latch_bits() {
        assert_eq!(tsf_latch_mask(0), 1);
        assert_eq!(tsf_latch_mask(1), 2);
        assert_eq!(tsf_latch_mask(u32::MAX), 2);
    }

    #[test]
    fn joins_only_the_recovered_rx_descriptor_address_fields() {
        assert_eq!(
            join_rx_descriptor_address(0xabc5_4321, 0x123f_edcb),
            0x1235_4321
        );
        assert_eq!(join_rx_descriptor_address(u32::MAX, 0), 0x000f_ffff);
        assert_eq!(join_rx_descriptor_address(0, u32::MAX), 0xfff0_0000);
    }

    #[test]
    fn tx_queue_registers_descend_by_the_recovered_stride() {
        assert_eq!(tx_queue_control_address(0), 0x2010_4d70);
        assert_eq!(tx_queue_control_address(1), 0x2010_4d60);
        assert_eq!(tx_queue_control_address(3), 0x2010_4d40);
    }

    #[test]
    fn tx_queue_control_fields_match_the_pinned_leaves() {
        assert_eq!(with_tx_cca(0xffff_ffff, 0), 0x3fff_ffff);
        assert_eq!(with_tx_cca(0x0123_4567, 1), 0x4123_4567);
        assert_eq!(with_tx_cca(0x0123_4567, 2), 0x8123_4567);
        assert_eq!(with_tx_cca(0x0123_4567, 3), 0xc123_4567);
        assert_eq!(with_tx_cca(0, 7), 0xc000_0000);
        assert_eq!(tx_queue_is_valid(0x4000_0000), 1);
        assert_eq!(tx_queue_is_valid(0x8000_0000), 0);
        assert_eq!(without_tx_queue_valid(u32::MAX), 0xbfff_ffff);
        assert_eq!(without_tx_queue_enable(u32::MAX), 0x3fff_ffff);
    }

    #[test]
    fn mac_address_registers_and_encoding_match_the_pinned_leaf() {
        assert_eq!(mac_address_registers(0), (0x2010_405c, 0x2010_4060));
        assert_eq!(mac_address_registers(3), (0x2010_4074, 0x2010_4078));
        assert_eq!(
            encode_mac_address([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]),
            (0x3322_1102, 0x0001_5544)
        );
    }

    #[test]
    fn rx_policy_addresses_and_masks_match_the_pinned_leaf() {
        assert_eq!(mac_rx_frame_policy_address(0), 0x2010_40d8);
        assert_eq!(mac_rx_frame_policy_address(3), 0x2010_40e4);
        assert_eq!(mac_rx_address_policy_address(2), 0x2010_4014);
        assert_eq!(mac_rx_management_policy_address(2), 0x2010_4070);

        assert_eq!(with_mac_rx_mode(u32::MAX, 0), 0xffff_fbef);
        assert_eq!(with_mac_rx_mode(0, 2), 0x0000_0410);
        assert_eq!(with_mac_rx_control_policy(u32::MAX, 1), 0xffff_ffbf);
        assert_eq!(with_mac_rx_control_policy(0, 2), 0x0000_0040);
        assert_eq!(with_mac_rx_control_address_policy(u32::MAX, 0), 0x7fff_ffff);
        assert_eq!(with_mac_rx_control_address_policy(0, 1), 0x8000_0000);
        assert_eq!(
            with_mac_rx_control_address_policy(0x1234_5678, 2),
            0x1234_5678
        );
        assert_eq!(with_mac_rx_management_policy(u32::MAX, 0), 0xfffe_ffff);
        assert_eq!(with_mac_rx_management_policy(0, 1), 0x0001_0000);
        assert_eq!(with_mac_rx_unique_bssid_policy(u32::MAX, 0), 0xffff_fefd);
        assert_eq!(with_mac_rx_unique_bssid_policy(0, 1), 0x0000_0102);
    }

    #[test]
    fn no_power_save_mac_restart_matches_the_complete_pinned_chain() {
        assert_eq!(without_mac_tx_retention(u32::MAX), 0xff00_efff);
        assert_eq!(without_mac_tx_retention(0x12ff_3456), 0x1200_2456);

        assert_eq!(
            with_wifi_mac_regdma_link(0, WIFI_MAC_ACTIVE_REGDMA_LINK),
            0x0008_0000
        );
        assert_eq!(
            with_wifi_mac_regdma_link(u32::MAX, WIFI_MAC_ACTIVE_REGDMA_LINK),
            0xffe9_ffff
        );
        assert_eq!(with_wifi_mac_regdma_link(0x1234_5678, 0), 0x1220_5678);
    }

    #[test]
    fn phy_fe_bb_clock_mask_matches_the_pinned_leaf() {
        assert_eq!(without_fe_bb_clock_enable(u32::MAX), 0xffff_fffc);
        assert_eq!(without_fe_bb_clock_enable(0x1234_567b), 0x1234_5678);
    }

    #[test]
    fn phy_pbus_masks_and_command_encoding_match_complete_rom_leaves() {
        assert_eq!(with_phy_pbus_force_test(0, 4, 1, 0), 0x0000_8012);
        assert_eq!(with_phy_pbus_force_test(u32::MAX, 3, 2, 0x100), 0xffff_400f);
        assert!(!phy_pbus_is_busy(0x7fff_ffff));
        assert!(phy_pbus_is_busy(0x8000_0000));
        assert_eq!(phy_pbus_rx_dco_read_value(0xffff_ffff), 0x01ff);
        assert_eq!(phy_pbus_rx_dco_read_value(0x1234_0123), 0x0123);
        assert_eq!(phy_pbus_read_address(0, 0), 0x2010_08a4);
        assert_eq!(phy_pbus_read_address(1, 2), 0x2010_0894);
        assert_eq!(phy_pbus_read_address(2, 1), 0x2010_0898);
        assert_eq!(phy_pbus_read_address(2, 0), 0x2010_089c);
        assert_eq!(phy_pbus_read_address(3, 0), 0x2010_089c);
        assert_eq!(phy_pbus_read_address(4, 1), 0x2010_08a0);
        assert_eq!(phy_pbus_read_address(4, 0), 0x2010_08a4);
        assert_eq!(phy_pbus_read_address(5, 0), 0x2010_08a4);
        assert_eq!(phy_pbus_read_shift(0, 0), 9);
        assert_eq!(phy_pbus_read_shift(0, 1), 18);
        assert_eq!(phy_pbus_read_shift(1, 2), 0);
        assert_eq!(phy_pbus_read_shift(1, 1), 9);
        assert_eq!(phy_pbus_read_shift(2, 0), 18);
        assert_eq!(phy_pbus_read_shift(2, 1), 0);

        assert_eq!(without_phy_rx_dco_control_field(0x12ff_5678), 0x123f_5678);
        assert_eq!(
            with_restored_phy_rx_dco_control_field(0xffff_ffff, 0x0040_0000),
            0xff7f_ffff
        );
        assert_eq!(
            with_restored_phy_rx_dco_control_field(0x1234_5678, 0xffff_ffff),
            0x12f4_5678
        );
    }

    #[test]
    fn phy_tx_and_rx_clock_masks_match_both_rom_branches() {
        assert_eq!(with_phy_tx_clock(0, true), 0x0003_0000);
        assert_eq!(with_phy_tx_clock(u32::MAX, false), 0xfffc_ffff);
        assert_eq!(with_phy_tx_clock(0x1234_5678, true), 0x1237_5678);
        assert_eq!(with_phy_tx_clock(0x1234_5678, false), 0x1234_5678);

        assert_eq!(with_phy_rx_clock(0, true), 0x0000_c000);
        assert_eq!(with_phy_rx_clock(u32::MAX, false), 0xffff_3fff);
        assert_eq!(with_phy_rx_clock(0x1234_5678, true), 0x1234_d678);
        assert_eq!(with_phy_rx_clock(0x1234_5678, false), 0x1234_1678);
    }

    #[test]
    fn phy_calibration_tone_matches_both_evidenced_call_images() {
        assert_eq!(with_phy_tone_path0_selector(u32::MAX, 0x80), 0xffff_fffc);
        assert_eq!(with_phy_tone_path1_selector(u32::MAX, 0), 0xffff_fff3);

        assert_eq!(with_phy_tone_path(0xa000_0000, 1, 0x80, 0), 0xa004_0020);
        assert_eq!(with_phy_tone_path(0xa000_0000, 0, 0x80, 0x28), 0xa003_6020);
        assert_eq!(with_phy_tone_path(0xbfff_ffff, 0, 0, 0), 0xb000_0000);
    }

    #[test]
    fn phy_tone_gain_compensation_preserves_all_four_vendor_writes() {
        let first = without_phy_tx_gain_compensation_low_byte(0x1234_5678);
        let second = with_phy_tx_gain_compensation_byte1(first);
        let third = with_phy_tx_gain_compensation_byte2(second);
        let fourth = without_phy_tx_gain_compensation_high_byte(third);

        assert_eq!(first, 0x1234_5600);
        assert_eq!(second, 0x1234_fa00);
        assert_eq!(third, 0x12ff_fa00);
        assert_eq!(fourth, 0x00ff_fa00);
    }

    #[test]
    fn phy_txiq_register_transforms_match_complete_rom_leaves() {
        assert_eq!(with_phy_txiq_calibration_enabled(u32::MAX), 0xffff_bfff);
        assert_eq!(with_phy_txiq_calibration_enabled(0), 0x0000_2000);
        assert_eq!(with_phy_txiq_calibration_complete(0x0000_2000), 0x0000_6000);
        assert_eq!(
            with_phy_txiq_first_polarity(0xa000_0000, true, 0x50, 0x80),
            0xa42e_c020
        );
        assert_eq!(
            with_phy_txiq_second_polarity(0xa6ae_c020, true),
            0xa8ae_c020
        );
        assert_eq!(
            with_phy_txiq_second_polarity(0xa6ae_c020, false),
            0xa1ae_c020
        );
        assert_eq!(with_phy_txiq_gain(u32::MAX, -31), 0xffff_ffe1);
        assert_eq!(with_phy_txiq_phase(u32::MAX, -63), 0xffff_f07f);
    }

    #[test]
    fn phy_rxiq_register_transforms_match_complete_rom_leaves() {
        assert_eq!(with_phy_rxiq_gain(u32::MAX, -31), 0xffe1_ffff);
        assert_eq!(with_phy_rxiq_phase(u32::MAX, -63), 0xf07f_ffff);
        assert_eq!(with_phy_rxiq_calibration_mode(0), 0x2000_0000);
        assert_eq!(with_phy_rxiq_calibration_mode(u32::MAX), 0xbfff_ffff);
        assert_eq!(with_phy_rxiq_root_correction_begin(0), 0x2000_0000);
        assert_eq!(with_phy_rxiq_root_correction_begin(u32::MAX), 0xbfff_ffff);
        assert_eq!(with_phy_rxiq_root_aux_begin(0), 0x0000_2000);
        assert_eq!(with_phy_rxiq_root_aux_begin(u32::MAX), 0xffff_bfff);
    }

    #[test]
    fn phy_i2c_clock_selection_matches_all_rom_field_transforms() {
        assert_eq!(with_phy_i2c_clock_selection_high(0, 8), 0x0000_0080);
        assert_eq!(
            with_phy_i2c_clock_selection_low(with_phy_i2c_clock_selection_high(0, 8), 8,),
            0x0000_0084
        );
        assert_eq!(with_phy_i2c_clock_selection_high(u32::MAX, 8), 0xffff_f8bf);
        assert_eq!(with_phy_i2c_clock_selection_low(u32::MAX, 8), 0xffff_ffc4);
    }

    #[test]
    fn phy_fe_txrx_reset_matches_both_rom_writes() {
        assert_eq!(without_phy_fe_txrx_reset(u32::MAX), 0xf9ff_ffff);
        assert_eq!(with_phy_fe_txrx_reset(0), 0x0600_0000);
        assert_eq!(
            with_phy_fe_txrx_reset(without_phy_fe_txrx_reset(0xa5a5_5a5a)),
            0xa7a5_5a5a
        );
    }

    #[test]
    fn phy_adc_rate_mmio_suffix_matches_both_rom_fields() {
        assert_eq!(with_phy_adc_rate_high(0, 1), 0x0000_0002);
        assert_eq!(with_phy_adc_rate_low(0x0000_0002, 1), 0x0000_0003);
        assert_eq!(with_phy_adc_rate_high(u32::MAX, 0), 0xffff_fffd);
        assert_eq!(with_phy_adc_rate_low(u32::MAX, 0), 0xffff_fffe);
    }

    #[test]
    fn phy_front_end_register_transforms_preserve_exact_masks() {
        assert_eq!(with_register_bits(0x1234_5678, 0x0010_0000), 0x1234_5678);
        assert_eq!(with_register_bits(0x1234_5678, 0x8000_0000), 0x9234_5678);
        assert_eq!(without_register_bits(u32::MAX, 0x0000_0900), 0xffff_f6ff);
        assert_eq!(
            with_register_field(u32::MAX, 0xff00_0000, 0xa000_0000),
            0xa0ff_ffff
        );
        assert_eq!(
            with_register_field(0x1234_56ff, 0x0000_00ff, 0x0000_0057),
            0x1234_5657
        );
    }

    #[test]
    fn phy_front_end_update_preserves_archive_masks_and_fresh_read_order() {
        let initial = 0x8100_4000;
        let first = with_phy_front_end_update_first(initial);
        let second = with_phy_front_end_update_second(first);

        assert_eq!(first, 0x8300_4000);
        assert_eq!(second, 0x8700_4000);
        assert_eq!(with_phy_front_end_adc_update(0xa5a5_0100), 0xa5a5_0103);
    }

    #[test]
    fn phy_temperature_sensor_power_forces_the_instruction_proven_constant() {
        assert_eq!(
            with_register_field(0, 0x0040_0000, 0x0040_0000),
            0x0040_0000
        );
        assert_eq!(
            with_register_field(u32::MAX, 0x0040_0000, 0x0040_0000),
            u32::MAX
        );
    }

    #[test]
    fn phy_baseband_gain_indices_match_the_rom_leaf() {
        assert_eq!(tx_baseband_gain_index(0x0080), 1);
        assert_eq!(tx_baseband_gain_index(0x0100), 2);
        assert_eq!(tx_baseband_gain_index(0x0020), 3);
        assert_eq!(tx_baseband_gain_index(0x00a0), 4);
        assert_eq!(tx_baseband_gain_index(0), 0);
        assert_eq!(tx_baseband_gain_index(u16::MAX), 0);
    }

    #[test]
    fn phy_gain_words_match_the_complete_vendor_transform() {
        assert_eq!(
            encode_phy_gain_memory_words(0, 0, 0, 0, 0, 0, 0, 0),
            (0, 0x1000_0000, 0x0000_7f80)
        );
        assert_eq!(
            encode_phy_gain_memory_words(
                0x0007, 0x00bf, 0xa5, 0x1234, 0x5678, 0x9abc, 0xdef0, 0xffff,
            ),
            (0xbfde_1fff, 0x93f6_3f3c, 0x0052_ff83)
        );
    }

    #[test]
    fn tx_gain_seed_view_crosses_the_owned_field_boundary_explicitly() {
        let image = crate::phy_channel::PhyWifiTxGainImage {
            seed: [
                0x0100_0000,
                0x0302_0000,
                0x0504_0000,
                0x0706_0000,
                0x0908_0000,
                0x0b0a_0000,
            ],
            output_32: [
                0x0f0e_0d0c,
                0x1312_1110,
                0x1716_1514,
                0x1b1a_1918,
                0,
                0,
                0,
                0,
            ],
            output_64: [0; 16],
            output_72: [0; 18],
            config: 0,
        };

        assert_eq!(tx_gain_seed_halfword(&image, 10), 0);
        assert_eq!(tx_gain_seed_halfword(&image, 11), 0x0b0a);
        assert_eq!(tx_gain_seed_halfword(&image, 12), 0x0d0c);
        assert_eq!(tx_gain_seed_halfword(&image, 19), 0x1b1a);
        assert_eq!(packed_halfword(&image.output_32, 1), 0x0f0e);
        assert_eq!(packed_byte(&image.output_32, 3), 0x0f);
    }
}
