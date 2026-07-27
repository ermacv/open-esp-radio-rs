//! Narrow ESP32-S31 radio-register leaves.
//!
//! These functions reproduce complete, finite ROM bodies whose only state is
//! documented MMIO. They are temporary runtime-local HAL boundaries until the
//! register layer moves into the ESP32-S31 radio HAL crate.

#[cfg(target_arch = "riscv32")]
use open_esp_radio_hal_esp32s31::RadioRegisters;

const PHY_TONE_PATH0_CONTROL_ADDRESS: usize = 0x2010_041c;
const PHY_TONE_PATH1_CONTROL_ADDRESS: usize = 0x2010_0420;
const PHY_TONE_STOP_CONTROL_ADDRESS: usize = 0x2010_040c;
const PHY_TONE_SELECTOR_CONTROL_ADDRESS: usize = 0x2010_0428;
const PHY_TX_GAIN_COMPENSATION_CONTROL_ADDRESS: usize = 0x2010_0410;
const PHY_TX_GAIN_COMPENSATION_AUX_ADDRESS: usize = 0x2010_0414;
const PHY_TX_DC_MEASUREMENT_CONTROL_ADDRESS: usize = 0x2010_0418;
const PHY_DAC_SCALE_CONTROL_ADDRESS: usize = 0x2010_0c04;
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
const PHY_IQ_CORRECTION_CONTROL_ADDRESS: usize = 0x2010_0438;
const PHY_IQ_CORRECTION_AUX_ADDRESS: usize = 0x2010_0c0c;
const PHY_PBUS_TRANSACTION_BIT: u32 = 1 << 1;
const PHY_PBUS_BUSY_BIT: u32 = 1 << 31;

/// Gate the calibration region around `phy_rf_init` and `phy_bb_init`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_phy_register_calibration_clock(registers: &mut RadioRegisters, enabled: bool) {
    registers.set_phy_calibration_clock(enabled);
}

/// Complete rev0 ROM `phy_bb_agc_reg_update`, size `0xa6`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_bb_agc_register_update(
    platform: &mut impl open_esp_radio_hal_esp32s31::wifi_bb::PhyWifiBbControl,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_hal_esp32s31::phy_agc::update_baseband_registers(platform, registers);
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
    platform: &mut impl open_esp_radio_hal_esp32s31::wifi_bb::PhyWifiBbControl,
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
    open_esp_radio_hal_esp32s31::phy_baseband::initialize_baseband(platform, registers);
    open_esp_radio_hal_esp32s31::phy_baseband::configure_watchdog(registers);
    open_esp_radio_hal_esp32s31::phy_baseband::configure_tx_pa_on(registers);
    configure_phy_rx_11b_optimization(registers, true);
    open_esp_radio_hal_esp32s31::phy_power_detector::configure_background(registers);
    open_esp_radio_hal_esp32s31::phy_baseband::configure_noise_floor_auto(registers);
    open_esp_radio_hal_esp32s31::phy_agc::configure_antenna(registers);
    open_esp_radio_hal_esp32s31::phy_frequency::configure_bt_filter(registers);
    open_esp_radio_hal_esp32s31::phy_frequency::enable_mac_baseband(platform, registers);
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
    platform: &mut impl open_esp_radio_hal_esp32s31::wifi_bb::PhyWifiBbControl,
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
        platform,
        registers,
        crate::phy_bb::PhyRegisterInitParameters {
            parameter_121: parameters.parameter_121,
            parameter_120: crate::phy_bb::PHY_RX_TABLE_ENTRY_COUNT,
        },
    );
    configure_phy_bb_agc_register_update(platform, registers);
    enable_phy_agc(registers);
}

const fn with_phy_pbus_force_test(value: u32, selector: u8, path: u8, test_value: u16) -> u32 {
    let command = ((test_value as u32) << 6) | ((selector as u32) << 2) | ((path as u32) << 15);
    (value & 0xfffe_0001) | (command & 0x0001_fffc) | PHY_PBUS_TRANSACTION_BIT
}

const fn phy_pbus_is_busy(value: u32) -> bool {
    value & PHY_PBUS_BUSY_BIT != 0
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

/// Apply the complete direct-register prefix/suffix of ROM
/// `phy_set_rx_gain_cal_dc`.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_rx_gain_dc_registers(
    registers: &mut RadioRegisters,
    enabled: bool,
) {
    if enabled {
        registers.set_phy_calibration_clock(true);
        set_register_bits(PHY_TONE_SELECTOR_CONTROL_ADDRESS - 4, 0x60);
    } else {
        clear_register_bits(PHY_TONE_SELECTOR_CONTROL_ADDRESS - 4, 0x60);
    }
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

/// Program the enabled path of rev0 ROM `phy_start_tx_tone_step`.
///
/// Unlike the archive's `_new` replacement below, the ROM leaf first disables
/// the DAC scale and TX-gain compensation, and leaves both disabled while the
/// power-control loop measures the tone.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn configure_phy_power_control_tone(selector: u16, step: u8) {
    clear_register_bits(PHY_TONE_STOP_CONTROL_ADDRESS, 0x0000_0003);
    replace_register_field(PHY_DAC_SCALE_CONTROL_ADDRESS, 0x00ff_0000, 0);
    replace_register_field(PHY_DAC_SCALE_CONTROL_ADDRESS, 0x0000_ff00, 0);

    let compensation = PHY_TX_GAIN_COMPENSATION_CONTROL_ADDRESS as *mut u32;
    let compensation_aux = PHY_TX_GAIN_COMPENSATION_AUX_ADDRESS as *mut u32;
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
        1,
        i32::from(selector),
        i32::from(step),
    ));

    let path1 = PHY_TONE_PATH1_CONTROL_ADDRESS as *mut u32;
    path1.write_volatile(with_phy_tone_path(path1.read_volatile(), 0, 0, 0));
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
        encode_phy_gain_memory_words, packed_byte, packed_halfword, phy_pbus_is_busy,
        tx_baseband_gain_index, tx_gain_seed_halfword, with_phy_adc_rate_high,
        with_phy_adc_rate_low, with_phy_fe_txrx_reset, with_phy_front_end_adc_update,
        with_phy_front_end_update_first, with_phy_front_end_update_second,
        with_phy_i2c_clock_selection_high, with_phy_i2c_clock_selection_low,
        with_phy_pbus_force_test, with_phy_rxiq_calibration_mode, with_phy_rxiq_gain,
        with_phy_rxiq_phase, with_phy_tone_path, with_phy_tone_path0_selector,
        with_phy_tone_path1_selector, with_phy_tx_gain_compensation_byte1,
        with_phy_tx_gain_compensation_byte2, with_phy_txiq_calibration_complete,
        with_phy_txiq_calibration_enabled, with_phy_txiq_first_polarity, with_phy_txiq_gain,
        with_phy_txiq_phase, with_phy_txiq_second_polarity, with_register_bits,
        with_register_field, without_phy_fe_txrx_reset, without_phy_tx_gain_compensation_high_byte,
        without_phy_tx_gain_compensation_low_byte, without_register_bits,
    };

    #[test]
    fn phy_pbus_masks_and_command_encoding_match_complete_rom_leaves() {
        assert_eq!(with_phy_pbus_force_test(0, 4, 1, 0), 0x0000_8012);
        assert_eq!(with_phy_pbus_force_test(u32::MAX, 3, 2, 0x100), 0xffff_400f);
        assert!(!phy_pbus_is_busy(0x7fff_ffff));
        assert!(phy_pbus_is_busy(0x8000_0000));
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
