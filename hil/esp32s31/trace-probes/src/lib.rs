#![no_std]

//! Link-time probes for compiled vendor/Rust MMIO comparison.
//!
//! These wrappers are test-harness artifacts, never driver entry points. Fat
//! LTO inlines the safe HAL leaf into each retained symbol so the vendor-code
//! validator can compare the resulting instruction-level MMIO transaction
//! sequence.

use open_esp_radio_esp32s31_hal::RadioRegisters;

// Stable test-only projection protocol. Production PHY types keep their Rust
// layout private; these small C-layout records are the explicit binary
// boundary consumed by the host parity verifier.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CanonicalDot11pState {
    pub enabled: u8,
    pub configuration: u8,
}

macro_rules! canonical_byte_state {
    ($name:ident) => {
        #[repr(transparent)]
        #[derive(Clone, Copy)]
        pub struct $name {
            pub value: u8,
        }
    };
}

canonical_byte_state!(CanonicalCurrentLevelState);
canonical_byte_state!(CanonicalBtPowerTrackingState);
canonical_byte_state!(CanonicalBleChannelBaseState);
canonical_byte_state!(CanonicalInitializationParameterState);
canonical_byte_state!(CanonicalSlowTxPowerTrackingState);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CanonicalTemperatureTrackingState {
    pub first: u8,
    pub second: u8,
}

/// Harness projection of the Rust-owned replacement for
/// `phy_param_rom + 0x1ac` plus the independent async safety counter.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CanonicalIqEstimatorState {
    pub readiness_activity_edges: u16,
    pub readiness_samples: u16,
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Harness-only delay edge intercepted by the vendor-code validator before this body runs.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn ets_delay_us(micros: u32) {
    core::hint::black_box(micros);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_bt_index_to_bb(index: u32) -> u32 {
    open_esp_radio_esp32s31_phy::phy_bluetooth::bluetooth_gain_index_to_baseband(index)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_bt_bb_to_index(baseband: u32) -> u32 {
    open_esp_radio_esp32s31_phy::phy_bluetooth::bluetooth_baseband_to_gain_index(baseband)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_disable_agc(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_enabled(registers, false);
}

/// Compiled driver adapter for the `phy_iq_est_enable` semantic boundary.
///
/// This is deliberately test-only. It executes the production HAL leaves and
/// exposes the async scheduling edge as an intercepted delay marker. The host
/// validator independently drives the production typed transition; requiring
/// both views avoids depending on optimized Rust enum stack padding here.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_iq_est_enable(
    control: u32,
    state: &mut CanonicalIqEstimatorState,
    registers: &mut RadioRegisters,
) {
    state.readiness_activity_edges = 0;
    state.readiness_samples = 0;

    open_esp_radio_esp32s31_hal::phy_iq_estimator::configure(registers, control as u16);
    open_esp_radio_esp32s31_hal::phy_iq_estimator::set_start_enabled(registers, true);
    ets_delay_us(1);
    open_esp_radio_esp32s31_hal::phy_iq_estimator::set_measurement_enabled(registers, true);

    while state.readiness_samples < open_esp_radio_esp32s31_phy::HARDWARE_EDGE_LIMIT {
        // Harness marker for the Embassy timer/yield owned by the production
        // target completer before every live sample.
        ets_delay_us(1);
        let snapshot = open_esp_radio_esp32s31_hal::phy_iq_estimator::sample_readiness(registers);
        state.readiness_samples = state.readiness_samples.saturating_add(1);
        if snapshot.ready {
            return;
        }
        if snapshot.activity {
            state.readiness_activity_edges = state.readiness_activity_edges.wrapping_add(1);
        }
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_enable_agc(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_enabled(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_vht_support(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_vht_support(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_csidump_force_lltf_cfg(
    input: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_csi_dump_force_lltf(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_hemu_ru26_good_res(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_he_ru26_good_response(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_freq_band_reg_set(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_frequency_band(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_fe_reg_init(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::initialize_front_end(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_fe_reg_update(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::update_front_end(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bbtx_outfilter(
    input_0: u32,
    input_1: u32,
    input_2: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_tx_output_filter(
        registers, input_0, input_1, input_2,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdt_rst_enable(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_watchdog_reset_enabled(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdt_int_enable(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_watchdog_interrupt_enabled(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdt_timeout_clear(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::clear_watchdog_timeout(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_bb_wdt_get_status(registers: &mut RadioRegisters) -> u32 {
    open_esp_radio_esp32s31_hal::phy_baseband::watchdog_status(registers)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_lltf_mask_en(
    input_0: u32,
    input_1: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_lltf_mask(registers, input_0, input_1);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ant_init(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_antenna(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdg_cfg(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_watchdog(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bt_filter_reg(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::configure_bt_filter(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_freq_module_resetn(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::reset_module(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_en_hw_set_freq(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::set_hardware_control(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_dis_hw_set_freq(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::set_hardware_control(registers, false);
    ets_delay_us(2);
    // Keep the delay as a non-tail edge so the executor sees the named
    // harness symbol independently of linker tail-call relaxation.
    core::hint::black_box(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_freq_reg_init(
    _vendor_parameter_0: u32,
    _vendor_parameter_1: u32,
    parameter_override: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_frequency::initialize_registers(
        registers,
        parameter_override != 0,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_iq_corr_enable(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::enable_iq_correction(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_noise_floor_auto_set(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_noise_floor_auto(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_read_hw_noisefloor(registers: &RadioRegisters) -> u32 {
    open_esp_radio_esp32s31_hal::phy_baseband::read_hardware_noise_floor(registers) as u32
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_tx_paon_set(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_tx_pa_on(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_wifi_agc_sat_gain(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_saturation_gain(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_enable_low_rate(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_low_rate_enabled(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_disable_low_rate(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_low_rate_enabled(registers, false);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_is_low_rate_enabled(registers: &RadioRegisters) -> u32 {
    u32::from(open_esp_radio_esp32s31_hal::phy_agc::low_rate_enabled(
        registers,
    ))
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_dcmem_clr(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::clear_dc_memory(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_rx_11b_opt(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rx_11b_optimization(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_rfrx_sat_rst(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rf_rx_saturation(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_set_rxclk_en(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_set_txclk_en(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_pbus_debugmode(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_i2c_txrate_init(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_i2c_tx_rate(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_nrx_freq_set(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::configure_nrx_frequency(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_cbw_chan_cfg(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::configure_channel_cbw(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_agc_reg_init(
    parameter_121: u32,
    parameter_120: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_agc::initialize_registers(
        registers,
        parameter_121 as u8,
        parameter_120 as u8,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_set_rx_comp_new(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rx_compensation(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_bb_txpwr_track(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_tx_power_tracking(
        registers,
        input & 1 != 0,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_reg_update_new(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::update_post_initialization(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_dc_mem_clr(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::clear_dc_memory(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_set_ftm_en(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_ftm_enabled(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_stop_tx_tone_new(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::stop_tx_tone(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_close_fe_bb_clk(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_clock::close_frontend_baseband(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_config_hccfr(
    enabled: u32,
    value: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_hccfr(registers, enabled, value);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_iccfr_en(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_iccfr_gate(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_force_iccfr(
    mode: u32,
    enabled: u32,
    value: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_forced_iccfr(
        registers, mode, enabled, value,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_pwdet_always_en() {
    open_esp_radio_esp32s31_phy::phy_pwdet::phy_pwdet_always_en();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_pwdet_onetime_en() {
    open_esp_radio_esp32s31_phy::phy_pwdet::phy_pwdet_onetime_en();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_11p_set(
    enabled: u32,
    configuration: u32,
    output: &mut CanonicalDot11pState,
) {
    let initial = *output;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_dot11p_configuration(initial.enabled, initial.configuration);
    state.set_dot11p_configuration(enabled as u8, configuration as u8);
    let projected = state.dot11p_configuration();
    *output = CanonicalDot11pState {
        enabled: projected.enabled,
        configuration: projected.configuration,
    };
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_current_level_set(
    value: u32,
    output: &mut CanonicalCurrentLevelState,
) {
    let initial = output.value;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_current_level(initial);
    state.set_current_level(value as u8);
    output.value = state.current_level();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_bt_power_track(
    value: u32,
    output: &mut CanonicalBtPowerTrackingState,
) {
    let initial = output.value;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_bt_power_tracking(initial);
    state.set_bt_power_tracking(value as u8);
    output.value = state.bt_power_tracking();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_ble_set_chan_base(
    value: u32,
    output: &mut CanonicalBleChannelBaseState,
) {
    let initial = output.value;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_ble_channel_base(initial);
    state.set_ble_channel_base(value as u8);
    output.value = state.ble_channel_base();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_init_param_set(
    value: u32,
    output: &mut CanonicalInitializationParameterState,
) {
    let initial = output.value;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_initialization_parameter(u32::from(initial != 0));
    state.set_initialization_parameter(value);
    output.value = u8::from(state.initialization_parameter());
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_track_temp_debug(
    first: u32,
    second: u32,
    output: &mut CanonicalTemperatureTrackingState,
) {
    let initial = *output;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_temperature_tracking_debug(initial.first, initial.second);
    state.set_temperature_tracking_debug(first as u8, second as u8);
    let projected = state.temperature_tracking_debug();
    *output = CanonicalTemperatureTrackingState {
        first: projected.first,
        second: projected.second,
    };
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_noise_check_loop() {
    open_esp_radio_esp32s31_phy::phy_signal_power::noise_check_loop();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_bbpll_en_usb() {
    open_esp_radio_esp32s31_phy::phy_rfpll::phy_bbpll_en_usb();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_freq_mem_backup() {
    open_esp_radio_esp32s31_phy::phy_frequency::phy_freq_mem_backup();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_freq_offset_set() {
    open_esp_radio_esp32s31_phy::phy_frequency::phy_freq_offset_set();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_get_i2c_data() {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_get_i2c_data();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_archive_set_bb_wdg() {
    open_esp_radio_esp32s31_phy::phy_bb::set_bb_wdg();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_phy_get_rf_cal_version() -> u32 {
    open_esp_radio_esp32s31_phy::phy_rfpll::phy_get_rf_cal_version()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_phy_get_rfdata_num() -> u32 {
    open_esp_radio_esp32s31_phy::phy_cold::phy_get_rfdata_num()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_get_bias_ref_code() -> u32 {
    open_esp_radio_esp32s31_phy::phy_tx_cal::get_bias_ref_code()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_phy_internal_delay() -> u32 {
    open_esp_radio_esp32s31_phy::phy_cold::phy_internal_delay()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_i2c_enter_critical() {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_i2c_enter_critical();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_i2c_exit_critical() {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_i2c_exit_critical();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_get_dc_value(output: &mut [u16; 2], value: u32) {
    open_esp_radio_esp32s31_phy::phy_dc_iq::get_dc_value(output, value);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_i2c_master_mem_cfg(configuration: &mut [u8; 6]) {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_i2c_master_mem_cfg(configuration);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_i2c_master_command_mem_cfg(
    configuration: &mut [u8; 8],
    mode: &mut u32,
) {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_i2c_master_command_mem_cfg(configuration, mode);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_tx_atten_comp(values: &mut [u8; 3]) {
    open_esp_radio_esp32s31_phy::phy_tx_cal::phy_tx_atten_comp(values);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ant_dft_cfg(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_antenna_diversity(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_btbb_wifi_bb_cfg2(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_bt_wifi_baseband(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_chan_dump_cfg(
    value: u32,
    enabled: u32,
    mode: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_channel_dump(
        registers, value, enabled, mode,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_dac_rate_set(rate: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_dac_rate(registers, rate);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_force_pwr_index(
    enabled: u32,
    index: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_memory::configure_forced_power_index(
        registers, enabled, index,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_force_rx_gain(
    enabled: u32,
    gain: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_forced_rx_gain(registers, enabled, gain);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_rx11blr_cfg(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rx_11b_low_rate(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_enable_cca(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::wifi_mac::set_cca_enabled(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_disable_cca(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::wifi_mac::set_cca_enabled(registers, false);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_sifs_reg_init(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::wifi_mac::initialize_sifs(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_save_pbus_reg(output: &mut [u32; 6], registers: &RadioRegisters) {
    *output = open_esp_radio_esp32s31_hal::phy_memory::capture_pbus_memory_boundaries(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_abs_temp(input: u32) -> u32 {
    open_esp_radio_esp32s31_phy::phy_rxiq::phy_abs_temp(input as i32)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_encode_i2c_master(
    block: u32,
    register: u32,
    value: u32,
) -> u32 {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_encode_i2c_master(block, register, value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_get_freq_mem_addr(
    base: u32,
    stride: u32,
    index: u32,
    offset: u32,
) -> u32 {
    u32::from(
        open_esp_radio_esp32s31_phy::phy_frequency::phy_get_freq_mem_addr(
            base, stride, index, offset,
        ),
    )
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_byte_to_word(bytes: &[u8; 4]) -> u32 {
    open_esp_radio_esp32s31_phy::phy_i2c::phy_byte_to_word(bytes)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_txpwr_track_slow(
    value: u32,
    output: &mut CanonicalSlowTxPowerTrackingState,
) {
    let initial = output.value;
    let mut state = open_esp_radio_esp32s31_phy::phy_cold::PhyColdState::new();
    state.set_tx_power_tracking_slow(initial);
    state.set_tx_power_tracking_slow(value as u8);
    output.value = state.tx_power_tracking_slow();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_freq_i2c_mem_write(
    address: u32,
    value: u32,
    mode: u32,
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_frequency::write_memory(
        registers,
        (address & 0x07ff) as u16,
        value & 0x00ff_ffff,
        mode as u8,
    );
}

/// Retain every exported probe when this crate is linked into the executable
/// comparison image. This function is harness-only and is never executed by
/// the PHY driver.
#[inline(never)]
pub fn retain_all_probes() {
    core::hint::black_box(open_phy_trace_disable_agc as *const ());
    core::hint::black_box(open_phy_trace_iq_est_enable as *const ());
    core::hint::black_box(open_phy_trace_enable_agc as *const ());
    core::hint::black_box(open_phy_trace_vht_support as *const ());
    core::hint::black_box(open_phy_trace_csidump_force_lltf_cfg as *const ());
    core::hint::black_box(open_phy_trace_hemu_ru26_good_res as *const ());
    core::hint::black_box(open_phy_trace_freq_band_reg_set as *const ());
    core::hint::black_box(open_phy_trace_fe_reg_init as *const ());
    core::hint::black_box(open_phy_trace_phy_fe_reg_update as *const ());
    core::hint::black_box(open_phy_trace_bbtx_outfilter as *const ());
    core::hint::black_box(open_phy_trace_bb_wdt_rst_enable as *const ());
    core::hint::black_box(open_phy_trace_bb_wdt_int_enable as *const ());
    core::hint::black_box(open_phy_trace_bb_wdt_timeout_clear as *const ());
    core::hint::black_box(open_phy_trace_ret_bb_wdt_get_status as *const ());
    core::hint::black_box(open_phy_trace_lltf_mask_en as *const ());
    core::hint::black_box(open_phy_trace_ant_init as *const ());
    core::hint::black_box(open_phy_trace_bb_wdg_cfg as *const ());
    core::hint::black_box(open_phy_trace_bt_filter_reg as *const ());
    core::hint::black_box(open_phy_trace_freq_module_resetn as *const ());
    core::hint::black_box(open_phy_trace_en_hw_set_freq as *const ());
    core::hint::black_box(open_phy_trace_dis_hw_set_freq as *const ());
    core::hint::black_box(open_phy_trace_freq_reg_init as *const ());
    core::hint::black_box(open_phy_trace_iq_corr_enable as *const ());
    core::hint::black_box(open_phy_trace_noise_floor_auto_set as *const ());
    core::hint::black_box(open_phy_trace_ret_read_hw_noisefloor as *const ());
    core::hint::black_box(open_phy_trace_tx_paon_set as *const ());
    core::hint::black_box(open_phy_trace_wifi_agc_sat_gain as *const ());
    core::hint::black_box(open_phy_trace_enable_low_rate as *const ());
    core::hint::black_box(open_phy_trace_disable_low_rate as *const ());
    core::hint::black_box(open_phy_trace_ret_is_low_rate_enabled as *const ());
    core::hint::black_box(open_phy_trace_bb_dcmem_clr as *const ());
    core::hint::black_box(open_phy_trace_rx_11b_opt as *const ());
    core::hint::black_box(open_phy_trace_rfrx_sat_rst as *const ());
    core::hint::black_box(open_phy_trace_set_rxclk_en as *const ());
    core::hint::black_box(open_phy_trace_set_txclk_en as *const ());
    core::hint::black_box(open_phy_trace_pbus_debugmode as *const ());
    core::hint::black_box(open_phy_trace_i2c_txrate_init as *const ());
    core::hint::black_box(open_phy_trace_nrx_freq_set as *const ());
    core::hint::black_box(open_phy_trace_bb_cbw_chan_cfg as *const ());
    core::hint::black_box(open_phy_trace_agc_reg_init as *const ());
    core::hint::black_box(open_phy_trace_phy_set_rx_comp_new as *const ());
    core::hint::black_box(open_phy_trace_phy_bb_txpwr_track as *const ());
    core::hint::black_box(open_phy_trace_phy_reg_update_new as *const ());
    core::hint::black_box(open_phy_trace_phy_dc_mem_clr as *const ());
    core::hint::black_box(open_phy_trace_phy_set_ftm_en as *const ());
    core::hint::black_box(open_phy_trace_phy_stop_tx_tone_new as *const ());
    core::hint::black_box(open_phy_trace_phy_close_fe_bb_clk as *const ());
    core::hint::black_box(open_phy_trace_phy_config_hccfr as *const ());
    core::hint::black_box(open_phy_trace_phy_iccfr_en as *const ());
    core::hint::black_box(open_phy_trace_phy_force_iccfr as *const ());
    core::hint::black_box(open_phy_trace_phy_pwdet_always_en as *const ());
    core::hint::black_box(open_phy_trace_phy_pwdet_onetime_en as *const ());
    core::hint::black_box(open_phy_trace_phy_11p_set as *const ());
    core::hint::black_box(open_phy_trace_phy_current_level_set as *const ());
    core::hint::black_box(open_phy_trace_phy_bt_power_track as *const ());
    core::hint::black_box(open_phy_trace_phy_ble_set_chan_base as *const ());
    core::hint::black_box(open_phy_trace_phy_init_param_set as *const ());
    core::hint::black_box(open_phy_trace_phy_track_temp_debug as *const ());
    core::hint::black_box(open_phy_trace_noise_check_loop as *const ());
    core::hint::black_box(open_phy_trace_phy_bbpll_en_usb as *const ());
    core::hint::black_box(open_phy_trace_phy_freq_mem_backup as *const ());
    core::hint::black_box(open_phy_trace_phy_freq_offset_set as *const ());
    core::hint::black_box(open_phy_trace_phy_get_i2c_data as *const ());
    core::hint::black_box(open_phy_trace_archive_set_bb_wdg as *const ());
    core::hint::black_box(open_phy_trace_ret_phy_get_rf_cal_version as *const ());
    core::hint::black_box(open_phy_trace_ret_phy_get_rfdata_num as *const ());
    core::hint::black_box(open_phy_trace_ret_get_bias_ref_code as *const ());
    core::hint::black_box(open_phy_trace_ret_phy_internal_delay as *const ());
    core::hint::black_box(open_phy_trace_phy_i2c_enter_critical as *const ());
    core::hint::black_box(open_phy_trace_phy_i2c_exit_critical as *const ());
    core::hint::black_box(open_phy_trace_get_dc_value as *const ());
    core::hint::black_box(open_phy_trace_phy_i2c_master_mem_cfg as *const ());
    core::hint::black_box(open_phy_trace_phy_i2c_master_command_mem_cfg as *const ());
    core::hint::black_box(open_phy_trace_phy_tx_atten_comp as *const ());
    core::hint::black_box(open_phy_trace_ant_dft_cfg as *const ());
    core::hint::black_box(open_phy_trace_btbb_wifi_bb_cfg2 as *const ());
    core::hint::black_box(open_phy_trace_chan_dump_cfg as *const ());
    core::hint::black_box(open_phy_trace_dac_rate_set as *const ());
    core::hint::black_box(open_phy_trace_force_pwr_index as *const ());
    core::hint::black_box(open_phy_trace_force_rx_gain as *const ());
    core::hint::black_box(open_phy_trace_rx11blr_cfg as *const ());
    core::hint::black_box(open_phy_trace_enable_cca as *const ());
    core::hint::black_box(open_phy_trace_disable_cca as *const ());
    core::hint::black_box(open_phy_trace_sifs_reg_init as *const ());
    core::hint::black_box(open_phy_trace_save_pbus_reg as *const ());
    core::hint::black_box(open_phy_trace_ret_abs_temp as *const ());
    core::hint::black_box(open_phy_trace_ret_encode_i2c_master as *const ());
    core::hint::black_box(open_phy_trace_ret_get_freq_mem_addr as *const ());
    core::hint::black_box(open_phy_trace_ret_byte_to_word as *const ());
    core::hint::black_box(open_phy_trace_txpwr_track_slow as *const ());
    core::hint::black_box(open_phy_trace_freq_i2c_mem_write as *const ());
}
