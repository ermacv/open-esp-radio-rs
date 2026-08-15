#![no_std]

//! Link-time probes for compiled vendor/Rust MMIO comparison.
//!
//! These wrappers are test-harness artifacts, never driver entry points. Fat
//! LTO inlines the safe HAL leaf into each retained symbol so the Workbench
//! verifier can compare the resulting instruction-level MMIO transaction
//! sequence.

use open_esp_radio_esp32s31_hal::RadioRuntimeOwner;
// Supporting PHY semantic projections still receive an explicit PAC token.
// Release-relevant production probes below acquire only opaque HAL owners.
use open_esp_radio_esp32s31_pac::RadioRegisters as SemanticRadioRegisters;
use open_esp_radio_esp32s31_wifi_mac::ap_tsf::{
    reset_and_start_access_point_tsf, stop_access_point_tsf,
};

mod production_trace;

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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Harness-only delay edge intercepted by the Workbench verifier before this body runs.
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
pub extern "C" fn open_phy_trace_disable_agc(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_enabled(registers, false);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_trace_hal_mac_interrupt_ret_get_event() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_interrupt_get_event()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_trace_hal_mac_interrupt_ret_clr_event(events: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_interrupt_clr_event(events)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_wifi_sta_trace_hal_disable_sta_beacon_filter() {
    // SAFETY: these validation-only PAC capabilities are used only by this
    // isolated probe image. The called helper is the production transaction.
    open_esp_radio_esp32s31_hal::validation::hal_disable_sta_beacon_filter();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_irq_trace_hal_pwr_interrupt_get_event() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_pwr_interrupt_get_event()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event(events: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_pwr_interrupt_clr_event(events)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_disable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_hal::validation::hal_mac_rx_disable(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_enable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_hal::validation::hal_mac_rx_enable(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_read_rxdscrlast() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_rx_read_rxdscrlast()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_read_rxdscrnext() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_rx_read_rxdscrnext()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_set_base(address: u32) {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_hal::validation::hal_mac_rx_set_base(address);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_get_last_dscr() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_rx_get_last_dscr()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_is_dscr_reload() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_rx_is_dscr_reload()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_set_dscr_reload() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_hal::validation::hal_mac_rx_set_dscr_reload(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_enable(index: u32) {
    open_esp_radio_esp32s31_hal::coex::validation_enable_timer(index);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_disable(index: u32) {
    open_esp_radio_esp32s31_hal::coex::validation_disable_timer(index);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_force(index: u32) {
    open_esp_radio_esp32s31_hal::coex::validation_force_timer(index);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_unforce(index: u32) {
    open_esp_radio_esp32s31_hal::coex::validation_unforce_timer(index);
}

/// Compiled production-path probe for the complete `coex_core_pti_get`
/// contract. The vendor ABI returns `0x102` for a null output pointer and
/// otherwise copies one entry from its reviewed 48-byte priority table.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_core_trace_pti_get(event: u32, output: *mut u8) -> u32 {
    if output.is_null() {
        return 0x102;
    }
    let Ok(event) = open_esp_radio_esp32s31_coex::CoexEventId::new(event as u8) else {
        return 0x102;
    };
    let pti = open_esp_radio_esp32s31_coex::CoexPtiTable::reviewed_vendor().pti(event);
    // SAFETY: verification profiles provide a writable caller-owned output
    // byte and compare its final state with the vendor execution.
    unsafe { output.write(pti.value()) };
    0
}

/// Compiled production-path probe for `coex_core_event_duration_get`.
/// The vendor leaf accepts only the five events backed by `g_coex_param`,
/// clears a non-null output for every other event, and uses `u32::MAX` as its
/// invalid-argument status.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_core_trace_event_duration_get(event: u32, output: *mut u32) -> u32 {
    if output.is_null() {
        return u32::MAX;
    }
    let duration = open_esp_radio_esp32s31_coex::CoexEventId::new(event as u8)
        .ok()
        .and_then(|event| {
            open_esp_radio_esp32s31_coex::CoexEventDurations::reviewed_vendor().duration(event)
        });
    // SAFETY: verification profiles provide a writable caller-owned output
    // word and compare its final state with the vendor execution.
    unsafe { output.write(duration.unwrap_or(0)) };
    if duration.is_some() { 0 } else { u32::MAX }
}

/// Compiled production-path projection of the complete vendor event-to-timer
/// switch. `0xff` is the exact unmapped sentinel returned by the vendor leaf.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_core_trace_timer_idx_get(event: u32) -> u32 {
    let Ok(event) = open_esp_radio_esp32s31_coex::CoexEventId::new(event as u8) else {
        return 0xff;
    };
    event
        .timer_index()
        .map_or(0xff, |index| u32::from(index.value()))
}

struct CoexProbeClock {
    is_real_chip: bool,
}

impl open_esp_radio_esp32s31_coex::CoexClockHardware for CoexProbeClock {
    fn sample(
        &mut self,
    ) -> Result<open_esp_radio_esp32s31_coex::CoexTimerClock, open_esp_radio_esp32s31_coex::CoexError>
    {
        const COEX_LP_CLK_CONF: *const u32 = 0x2010_f008 as *const u32;
        // SAFETY: this validation-only implementation runs under the
        // workbench MMIO executor. Each volatile read intentionally mirrors
        // one independent read in `coex_hw_timer_tick_get`.
        let selector = unsafe { core::ptr::read_volatile(COEX_LP_CLK_CONF) };
        // SAFETY: same validated MMIO word, deliberately sampled again.
        let divider = unsafe { core::ptr::read_volatile(COEX_LP_CLK_CONF) };
        open_esp_radio_esp32s31_coex::CoexTimerClock::from_register_images(
            selector,
            divider,
            40,
            self.is_real_chip,
        )
    }
}

/// Compiled production-path probe for the complete `coex_hw_timer_set`
/// transaction. The public vendor ABI is `(index, client, pti, latency,
/// duration)`; notably, duration is written to the primary word before
/// latency is written to the secondary word.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_set_trace_coex_hw_timer_set(
    index: u32,
    client: u32,
    pti: u32,
    latency: u32,
    duration: u32,
    is_real_chip: u32,
) {
    use open_esp_radio_esp32s31_coex::{CoexClient, CoexPti, CoexTimerIndex};

    let Ok(index) = CoexTimerIndex::new(index as u8) else {
        return;
    };
    let Ok(pti) = CoexPti::new(pti as u8) else {
        return;
    };
    let client = if client == 0 {
        CoexClient::Bluetooth
    } else {
        CoexClient::Wifi
    };
    let mut clock = CoexProbeClock {
        is_real_chip: is_real_chip != 0,
    };
    let _ = open_esp_radio_esp32s31_hal::coex::validation_program_timer(
        &mut clock, index, client, pti, latency, duration,
    );
}

/// Complete register projection of `coex_core_request` under the reviewed
/// Rust lifecycle precondition that the COEX owner is enabled.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_core_trace_request(
    client: u32,
    event: u32,
    latency: u32,
    duration: u32,
    is_real_chip: u32,
) -> u32 {
    use open_esp_radio_esp32s31_coex::{CoexClientRequest, CoexError, CoexEventId};

    let Ok(event) = CoexEventId::new(event as u8) else {
        return 0x102;
    };
    let mut clock = CoexProbeClock {
        is_real_chip: is_real_chip != 0,
    };
    let request = CoexClientRequest {
        event,
        latency,
        duration,
    };
    let result = open_esp_radio_esp32s31_hal::coex::validation_core_request(
        &mut clock,
        client != 0,
        request,
    );
    match result {
        Ok(_) => 0,
        Err(CoexError::InvalidEvent) => 0x102,
        Err(_) => u32::MAX,
    }
}

/// Complete register projection of `coex_core_release`. The vendor ABI keeps
/// the client in `a0` but selects the timer exclusively from event `a1`.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_core_trace_release(_client: u32, event: u32) -> u32 {
    use open_esp_radio_esp32s31_coex::{CoexError, CoexEventId};

    let Ok(event) = CoexEventId::new(event as u8) else {
        return 0x102;
    };
    match open_esp_radio_esp32s31_hal::coex::validation_core_release(event) {
        Ok(_) => 0,
        Err(CoexError::InvalidEvent) => 0x102,
        Err(_) => u32::MAX,
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_tx_set_cca(value: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_tx_set_cca(value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_get_txq_in_trig_flow_state()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_is_txq_enabled(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_is_txq_enabled(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_is_txq_valid(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_is_txq_valid(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_set_txq_invalid(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_set_txq_invalid(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_txq_disable(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_txq_disable(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_tx_config_edca(
    _vendor_config_address: u32,
    queue: u32,
    aifsn: u32,
    contention_window: u32,
    interface: open_esp_radio_esp32s31_hal::types::MacInterface,
) -> u32 {
    // The vendor side decodes these semantic arguments from its pointer-rich
    // ABI object. The Rust probe receives the reviewed projection directly;
    // every case keeps both representations and exact MMIO comparison proves
    // that the projection agrees with the vendor object.
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_hal::validation::hal_mac_tx_config_edca(
        queue,
        aifsn as u8,
        contention_window as u16,
        interface,
    )
}

#[repr(C)]
pub struct CanonicalTxBlockAck {
    pub control: u8,
    pub reserved: u8,
    pub starting_sequence: u16,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_tx_get_blockack(
    queue: u32,
    output_address: u32,
) -> u32 {
    let payload = open_esp_radio_esp32s31_hal::validation::hal_mac_tx_get_blockack(queue as u8)
        .expect("profile constrains ordinary TX queue 0..=3");
    let output = output_address as *mut CanonicalTxBlockAck;
    // SAFETY: the verification profile supplies one initialized, writable
    // twelve-byte output object for the duration of this call.
    unsafe {
        (*output).control = ((payload.control_and_sequence >> 16) & 0x0f) as u8;
        (*output).starting_sequence = ((payload.control_and_sequence >> 4) & 0x0fff) as u16;
        (*output).bitmap_low = payload.bitmap_low;
        (*output).bitmap_high = payload.bitmap_high;
    }
    0
}

/// ABI projection around the exact production normal-rate selector.
///
/// The vendor entry receives a pointer-rich descriptor and stores the chosen
/// rate at byte `0x0c`. The wrapper performs only that ABI projection; rate
/// selection is owned by the compiled production function.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_retry_trace_rc_get_rate(
    _rate_context: u32,
    descriptor_address: u32,
) {
    use open_esp_radio_esp32s31_wifi_mac::{
        tx::{LegacyRate, TxPhyRate},
        tx_runtime::{OrdinaryRetryCounters, select_ordinary_retry_rate},
    };

    let descriptor = descriptor_address as *mut u8;
    // SAFETY: every comparison case supplies a writable descriptor object
    // covering the vendor counter and selected-rate bytes.
    let counters = unsafe {
        OrdinaryRetryCounters {
            mpdu: descriptor.add(5).read(),
            short: descriptor.add(6).read(),
            long: descriptor.add(7).read(),
        }
    };
    let selected = select_ordinary_retry_rate(TxPhyRate::Legacy(LegacyRate::Ofdm54M), counters)
        .expect("reviewed rcGetRate cases remain inside the 54M schedule");
    // SAFETY: the same case-owned descriptor covers byte 0x0c.
    unsafe { descriptor.add(0x0c).write(selected.code()) };
}

// These validation-only leaves make the result of the compiled production
// completion classifier observable without introducing a shadow numeric
// encoding. The non-pure inline assembly prevents LLVM from deleting the
// calls; comparison stops at the selected leaf before executing its body.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_retry_ack_timeout(queue: u32) {
    // SAFETY: an empty volatile assembly block has no machine-visible inputs
    // or outputs and exists only as an optimizer barrier in the probe image.
    unsafe {
        core::arch::asm!(
            "addi zero, zero, 1",
            in("a0") queue,
            options(nomem, nostack)
        )
    };
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_retry_cts_timeout(queue: u32) {
    // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
    unsafe {
        core::arch::asm!(
            "addi zero, zero, 2",
            in("a0") queue,
            options(nomem, nostack)
        )
    };
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_retry_collision(queue: u32) {
    // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
    unsafe {
        core::arch::asm!(
            "addi zero, zero, 3",
            in("a0") queue,
            options(nomem, nostack)
        )
    };
}

/// ABI projection around the exact production status-four classifier.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_retry_trace_lmac_process_tx_error(
    queue: u32,
    detail: u32,
    _selector: u32,
) {
    use open_esp_radio_esp32s31_wifi_mac::tx::{
        TxCompletion, TxCompletionDisposition, TxCookie,
    };

    let completion = TxCompletion {
        cookie: TxCookie(0),
        status: 4,
        trigger_flow: false,
        used_alternate: false,
        auxiliary_a_word: 0,
        auxiliary_b_word: 0,
        auxiliary_c_word: 0,
        primary_word: detail,
        alternate_word: 0,
    };
    match completion.disposition() {
        TxCompletionDisposition::AckTimeout => open_libpp_tx_retry_ack_timeout(queue),
        TxCompletionDisposition::CtsTimeout => open_libpp_tx_retry_cts_timeout(queue),
        TxCompletionDisposition::Collision => open_libpp_tx_retry_collision(queue),
        TxCompletionDisposition::Success | TxCompletionDisposition::Terminal(_) => {}
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_interface_trace_hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
    open_esp_radio_esp32s31_hal::validation::hal_mac_set_addr(interface, address);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_interface_trace_hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
    open_esp_radio_esp32s31_hal::validation::hal_mac_set_bssid(interface, address);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_ap_tsf_trace_hal_disable_softap_tsf() {
    let mut owner = RadioRuntimeOwner::claim_for_validation();
    let mut hardware = owner.wifi_mac_hal();
    stop_access_point_tsf(&mut hardware);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset(selector: u32) {
    if selector == 0 {
        let mut owner = RadioRuntimeOwner::claim_for_validation();
        let mut hardware = owner.wifi_mac_hal();
        reset_and_start_access_point_tsf(&mut hardware);
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_rom_power_tsf_trace_hal_get_sta_tsf(low: *mut u32, high: *mut u32) {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    // SAFETY: the executable profile supplies either null or one aligned,
    // writable scratch word for each pointer, matching the ROM ABI.
    let low = unsafe { low.as_mut() };
    // SAFETY: same closed profile contract as `low`.
    let high = unsafe { high.as_mut() };
    open_esp_radio_esp32s31_hal::validation::hal_get_sta_tsf(low, high);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_enable_agc(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_enabled(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_vht_support(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_vht_support(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_csidump_force_lltf_cfg(
    input: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_csi_dump_force_lltf(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_hemu_ru26_good_res(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_he_ru26_good_response(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_freq_band_reg_set(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_frequency_band(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_fe_reg_init(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::initialize_front_end(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_fe_reg_update(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::update_front_end(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bbtx_outfilter(
    input_0: u32,
    input_1: u32,
    input_2: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_tx_output_filter(
        registers, input_0, input_1, input_2,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdt_rst_enable(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_watchdog_reset_enabled(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdt_int_enable(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::set_watchdog_interrupt_enabled(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdt_timeout_clear(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::clear_watchdog_timeout(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_bb_wdt_get_status(registers: &mut SemanticRadioRegisters) -> u32 {
    open_esp_radio_esp32s31_hal::phy_baseband::watchdog_status(registers)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_lltf_mask_en(
    input_0: u32,
    input_1: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_lltf_mask(registers, input_0, input_1);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ant_init(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_antenna(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_wdg_cfg(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_watchdog(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bt_filter_reg(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::configure_bt_filter(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_freq_module_resetn(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::reset_module(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_en_hw_set_freq(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::set_hardware_control(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_dis_hw_set_freq(registers: &mut SemanticRadioRegisters) {
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
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_frequency::initialize_registers(
        registers,
        parameter_override != 0,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_iq_corr_enable(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::enable_iq_correction(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_noise_floor_auto_set(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_noise_floor_auto(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_read_hw_noisefloor(registers: &SemanticRadioRegisters) -> u32 {
    open_esp_radio_esp32s31_hal::phy_baseband::read_hardware_noise_floor(registers) as u32
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_tx_paon_set(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_tx_pa_on(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_wifi_agc_sat_gain(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_saturation_gain(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_enable_low_rate(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_low_rate_enabled(registers, true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_disable_low_rate(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_low_rate_enabled(registers, false);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_ret_is_low_rate_enabled(registers: &SemanticRadioRegisters) -> u32 {
    u32::from(open_esp_radio_esp32s31_hal::phy_agc::low_rate_enabled(
        registers,
    ))
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_dcmem_clr(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::clear_dc_memory(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_rx_11b_opt(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rx_11b_optimization(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_rfrx_sat_rst(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rf_rx_saturation(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_set_rxclk_en(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::pbus::configure_rx_clock(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_set_txclk_en(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::pbus::configure_tx_clock(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_pbus_debugmode(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::pbus::configure_debug_mode(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_i2c_txrate_init(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_i2c_tx_rate(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_nrx_freq_set(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::configure_nrx_frequency(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_bb_cbw_chan_cfg(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_frequency::configure_channel_cbw(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_agc_reg_init(
    parameter_121: u32,
    parameter_120: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_agc::initialize_registers(
        registers,
        parameter_121 as u8,
        parameter_120 as u8,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_set_rx_comp_new(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rx_compensation(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_bb_txpwr_track(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_tx_power_tracking(
        registers,
        input & 1 != 0,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_reg_update_new(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::update_post_initialization(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_dc_mem_clr(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::clear_dc_memory(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_set_ftm_en(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_ftm_enabled(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_stop_tx_tone_new(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::stop_tx_tone(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_close_fe_bb_clk(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_clock::close_frontend_baseband(registers);
}

const fn cfr_value_from_vendor_argument(value: u32) -> open_esp_radio_esp32s31_hal::CfrValue {
    match open_esp_radio_esp32s31_hal::CfrValue::new(
        value as u16 & open_esp_radio_esp32s31_hal::CfrValue::MAX,
    ) {
        Some(value) => value,
        None => unreachable!(),
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_config_hccfr(
    enabled: u32,
    value: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_hccfr(
        registers,
        enabled & 1 != 0,
        cfr_value_from_vendor_argument(value),
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_iccfr_en(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_iccfr_gate(registers, input != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_force_iccfr(
    mode: u32,
    enabled: u32,
    value: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_forced_iccfr(
        registers,
        mode & 1 != 0,
        enabled & 1 != 0,
        cfr_value_from_vendor_argument(value),
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
pub extern "C" fn open_phy_trace_ant_dft_cfg(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_antenna_diversity(registers, input & 1 != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_btbb_wifi_bb_cfg2(registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_bt_wifi_baseband(registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_chan_dump_cfg(
    value: u32,
    enabled: u32,
    mode: u32,
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_channel_dump(
        registers, value, enabled, mode,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_dac_rate_set(rate: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_dac_rate(registers, rate);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_force_pwr_index(
    enabled: u32,
    index: u32,
    registers: &mut SemanticRadioRegisters,
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
    registers: &mut SemanticRadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_forced_rx_gain(
        registers,
        enabled & 1 != 0,
        open_esp_radio_esp32s31_hal::ForcedRxGain::new(gain as u8),
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_rx11blr_cfg(input: u32, registers: &mut SemanticRadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_rx_11b_low_rate(registers, input);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_enable_cca(registers: &mut SemanticRadioRegisters) {
    let _ = registers;
    open_esp_radio_esp32s31_hal::wifi_mac::validation_set_cca_enabled(true);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_disable_cca(registers: &mut SemanticRadioRegisters) {
    let _ = registers;
    open_esp_radio_esp32s31_hal::wifi_mac::validation_set_cca_enabled(false);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_sifs_reg_init(registers: &mut SemanticRadioRegisters) {
    let _ = registers;
    open_esp_radio_esp32s31_hal::wifi_mac::validation_initialize_sifs();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_save_pbus_reg(output: &mut [u32; 6], registers: &SemanticRadioRegisters) {
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
    let mut state = open_esp_radio_esp32s31_phy::phy_state::PhyState::default();
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
    registers: &mut SemanticRadioRegisters,
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
    core::hint::black_box(
        production_trace::open_phy_production_trace_phy_chip_set_chan as *const (),
    );
    core::hint::black_box(open_phy_trace_disable_agc as *const ());
    core::hint::black_box(open_libpp_trace_hal_mac_interrupt_ret_get_event as *const ());
    core::hint::black_box(open_libpp_trace_hal_mac_interrupt_ret_clr_event as *const ());
    core::hint::black_box(open_wifi_sta_trace_hal_disable_sta_beacon_filter as *const ());
    core::hint::black_box(open_libpp_power_irq_trace_hal_pwr_interrupt_get_event as *const ());
    core::hint::black_box(open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_disable as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_enable as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_read_rxdscrlast as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_read_rxdscrnext as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_set_base as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_get_last_dscr as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_is_dscr_reload as *const ());
    core::hint::black_box(open_libpp_rx_trace_hal_mac_rx_set_dscr_reload as *const ());
    core::hint::black_box(open_coex_trace_coex_hw_timer_enable as *const ());
    core::hint::black_box(open_coex_trace_coex_hw_timer_disable as *const ());
    core::hint::black_box(open_coex_trace_coex_hw_timer_force as *const ());
    core::hint::black_box(open_coex_trace_coex_hw_timer_unforce as *const ());
    core::hint::black_box(open_coex_set_trace_coex_hw_timer_set as *const ());
    core::hint::black_box(open_coex_core_trace_pti_get as *const ());
    core::hint::black_box(open_coex_core_trace_event_duration_get as *const ());
    core::hint::black_box(open_coex_core_trace_timer_idx_get as *const ());
    core::hint::black_box(open_coex_core_trace_request as *const ());
    core::hint::black_box(open_coex_core_trace_release as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_tx_set_cca as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_is_txq_enabled as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_is_txq_valid as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_set_txq_invalid as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_txq_disable as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_tx_config_edca as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_tx_get_blockack as *const ());
    core::hint::black_box(open_libpp_tx_retry_trace_rc_get_rate as *const ());
    core::hint::black_box(open_libpp_tx_retry_trace_lmac_process_tx_error as *const ());
    core::hint::black_box(open_libpp_tx_retry_ack_timeout as *const ());
    core::hint::black_box(open_libpp_tx_retry_cts_timeout as *const ());
    core::hint::black_box(open_libpp_tx_retry_collision as *const ());
    core::hint::black_box(open_libpp_interface_trace_hal_mac_set_addr as *const ());
    core::hint::black_box(open_libpp_interface_trace_hal_mac_set_bssid as *const ());
    core::hint::black_box(open_libpp_ap_tsf_trace_hal_disable_softap_tsf as *const ());
    core::hint::black_box(open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset as *const ());
    core::hint::black_box(open_rom_power_tsf_trace_hal_get_sta_tsf as *const ());
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
