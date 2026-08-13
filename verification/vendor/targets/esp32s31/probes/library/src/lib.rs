#![no_std]

//! Link-time probes for compiled vendor/Rust MMIO comparison.
//!
//! These wrappers are test-harness artifacts, never driver entry points. Fat
//! LTO inlines the safe HAL leaf into each retained symbol so the Workbench
//! verifier can compare the resulting instruction-level MMIO transaction
//! sequence.

use core::{
    cell::Cell,
    future::{Future, ready},
    pin::Pin,
};

use open_esp_radio_esp32s31_hal::RadioRegisters;
use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal;
use open_esp_radio_esp32s31_wifi_dma::descriptor::{
    BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT,
};
use open_esp_radio_esp32s31_wifi_dma::rx_storage::RxDmaStorage;
use open_esp_radio_esp32s31_wifi_dma::tx_storage::TxDmaStorage;
use open_esp_radio_esp32s31_wifi_mac::{
    ap_tsf::{reset_and_start_access_point_tsf, stop_access_point_tsf},
    crypto::{install_ap_pairwise_ccmp, install_sta_pairwise_ccmp},
    irq::{
        HANDLED_MAC_MASK, IrqDisposition, IrqSink, IrqWork, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
        handle_mac_irq, next_irq_work,
    },
    rx_pool::RxStagePool,
    tx::{LegacyTxConfig, LegacyTxQueue, TxSlot, TxSlotState, legacy_q0_image},
};
use open_esp_radio_ieee80211::station::{
    StaAssociationAttempt, StaAssociationFailure, StaAuthenticationAttempt,
    StaAuthenticationFailure, StaSequenceCounter,
};
use open_esp_radio_wifi_sta::join::{
    StaJoinBackend, StaJoinError, StaJoinRunner, StaJoinRxObserver, StaJoinTimer,
};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_verification_rx")]
static mut OPEN_LIBPP_RX_STORAGE: RxDmaStorage<2, 16, 20> = RxDmaStorage::new();

#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_verification_tx")]
static mut OPEN_LIBPP_TX_STORAGE: TxDmaStorage<64> = TxDmaStorage::new();

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
pub extern "C" fn open_phy_trace_disable_agc(registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::set_enabled(registers, false);
}

/// Compiled driver adapter for the `phy_iq_est_enable` semantic boundary.
///
/// This is deliberately test-only. It executes the production HAL leaves and
/// exposes the async scheduling edge as an intercepted delay marker. The host
/// verifier independently drives the production typed transition; requiring
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
pub extern "C" fn open_libpp_trace_hal_mac_interrupt_ret_get_event() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_interrupt_get_event()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_trace_hal_mac_interrupt_ret_clr_event(events: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_interrupt_clr_event(events)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_wifi_sta_trace_hal_disable_sta_beacon_filter() {
    // SAFETY: these validation-only PAC capabilities are used only by this
    // isolated probe image. The called helper is the production transaction.
    open_esp_radio_esp32s31_pac::validation::hal_disable_sta_beacon_filter();
}

/// Compiled production-driver projection for the vendor
/// `wDev_Insert_KeyEntry` interface-role boundary.
///
/// The host adapter checks only the reviewed connection-context field. It
/// deliberately does not claim whole-transaction equivalence because the
/// closed PAC adds fail-closed occupancy checks and zeroization.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_wifi_key_role_trace_wdev_insert_key_entry(
    _cipher: u32,
    interface: u32,
    _peer: u32,
    _key_id: u32,
    _index: u32,
    _key_words: u32,
    _key_length: u32,
    _flags: u32,
) -> u32 {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let peer = [0_u8; 6];
    let temporal_key = [0_u8; 16];
    let outcome = match interface {
        0 => install_sta_pairwise_ccmp(&mut registers, peer, &temporal_key).map(|_| ()),
        // AID one is the smallest valid AP pairwise slot. The adapter reviews
        // only the connection-context field, so the concrete slot must remain
        // explicit rather than being inferred from the interface selector.
        1 => install_ap_pairwise_ccmp(&mut registers, peer, 1, &temporal_key).map(|_| ()),
        _ => return 0,
    };
    match outcome {
        Ok(()) => 1,
        Err(open_esp_radio_esp32s31_wifi_mac::crypto::CryptoKeyError::Occupied) => 2,
        Err(_) => 3,
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_irq_trace_hal_pwr_interrupt_get_event() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_pwr_interrupt_get_event()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event(events: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_pwr_interrupt_clr_event(events)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_disable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_pac::validation::hal_mac_rx_disable(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_enable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_pac::validation::hal_mac_rx_enable(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_read_rxdscrlast() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_read_rxdscrlast()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_read_rxdscrnext() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_read_rxdscrnext()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_set_base(address: u32) {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_pac::validation::hal_mac_rx_set_base(address);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_get_last_dscr() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_get_last_dscr()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_is_dscr_reload() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_rx_is_dscr_reload()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_hal_mac_rx_set_dscr_reload() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_pac::validation::hal_mac_rx_set_dscr_reload(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_enable(index: u32) {
    let Some(timer) = open_esp_radio_esp32s31_pac::CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.enable_coex_timer(timer);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_disable(index: u32) {
    let Some(timer) = open_esp_radio_esp32s31_pac::CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.disable_coex_timer(timer);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_force(index: u32) {
    let Some(timer) = open_esp_radio_esp32s31_pac::CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.force_coex_timer(timer);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_trace_coex_hw_timer_unforce(index: u32) {
    let Some(timer) = open_esp_radio_esp32s31_pac::CoexTimerRegister::new(index as u8) else {
        return;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.unforce_coex_timer(timer);
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
    use open_esp_radio_esp32s31_coex::{CoexClient, CoexPti, CoexTimerIndex, program_timer};

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
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut timer = open_esp_radio_esp32s31_hal::coex::CoexTimerHal::new(&mut registers);
    let mut clock = CoexProbeClock {
        is_real_chip: is_real_chip != 0,
    };
    let _ = program_timer(
        &mut timer, &mut clock, index, client, pti, latency, duration,
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
    use open_esp_radio_esp32s31_coex::{
        CoexClientRequest, CoexCore, CoexError, CoexEventId, CoexPtiTable,
    };

    let Ok(event) = CoexEventId::new(event as u8) else {
        return 0x102;
    };
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    core.enable();
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut timer = open_esp_radio_esp32s31_hal::coex::CoexTimerHal::new(&mut registers);
    let mut clock = CoexProbeClock {
        is_real_chip: is_real_chip != 0,
    };
    let request = CoexClientRequest {
        event,
        latency,
        duration,
    };
    let result = if client == 0 {
        core.request_bluetooth(&mut timer, &mut clock, request)
    } else {
        core.request_wifi(&mut timer, &mut clock, request)
    };
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
    use open_esp_radio_esp32s31_coex::{CoexCore, CoexError, CoexEventId, CoexPtiTable};

    let Ok(event) = CoexEventId::new(event as u8) else {
        return 0x102;
    };
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut timer = open_esp_radio_esp32s31_hal::coex::CoexTimerHal::new(&mut registers);
    match core.release(&mut timer, event) {
        Ok(_) => 0,
        Err(CoexError::InvalidEvent) => 0x102,
        Err(_) => u32::MAX,
    }
}

/// Compiled composition probe for the source-owned RX append transaction.
///
/// `scenario` selects an immediate settle (0), two pending observations (1),
/// a terminal-frontier base repair (2), or the exact reload timeout (3). The
/// host qualification supplies the corresponding peripheral responses and
/// checks every emitted MMIO and one-microsecond scheduling marker. Descriptor
/// and staging memory remain real production Rust types; no vendor `wDevCtrl`
/// layout is reproduced.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_rx_trace_wdev_append_rx_blocks(scenario: u32) -> u32 {
    const COUNT: usize = 2;
    const BUFFER_SIZE: u32 = 16;

    // SAFETY: the executor starts every probe scenario from a fresh image.
    // The production arena API then owns the only live borrow for the rest of
    // this call; the raw reference merely supplies the stable root required
    // by the target-side DMA typestate.
    let storage: &'static RxDmaStorage<COUNT, 16, 20> =
        unsafe { &*core::ptr::addr_of!(OPEN_LIBPP_RX_STORAGE) };
    let mut buffers = [0_u32; COUNT];
    let Ok(base) = storage.dma_layout(&mut buffers) else {
        return 0;
    };
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let stopped = match storage.prepare_ring(&mut registers, base, &buffers) {
        Ok(stopped) => stopped,
        Err(_) => return 0,
    };
    let mut ring = match stopped.start(&mut registers) {
        Ok(ring) => ring,
        Err(_) => return 0,
    };

    storage.descriptors()[0].write_word0(BUFFER_SIZE | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    let completed = match storage.take_completed_unit(&mut ring, 1) {
        Ok(Some(completed)) => completed,
        _ => return 0,
    };
    let pool = RxStagePool::<1, 16>::new();
    let mut pending = match pool.stage_dma_unit_recycle_bounded(completed, &mut registers, 16) {
        Ok(open_esp_radio_esp32s31_wifi_mac::rx_pool::RxDmaStageUnitOutcome::Staged(pending)) => {
            pending
        }
        _ => return 0,
    };
    if storage.lifecycle_state() != open_esp_radio_esp32s31_wifi_dma::rx_ring::RxDmaArenaState::Live
    {
        return 0;
    }
    let mut async_edges = 0_u32;
    let frame = loop {
        match pending.poll_reload(&mut registers, &mut ring) {
            Ok(Some(frame)) => break frame,
            Ok(None) => {
                ets_delay_us(1);
                async_edges = async_edges.saturating_add(1);
            }
            Err(_) if scenario == 3 => return 0x4000_0000 | async_edges,
            Err(_) => return 0,
        }
    };
    let segment = frame.segment();
    if segment.descriptor_address != base
        // Staging deliberately severs the DMA-list link: the independent
        // frame is not a second descriptor owner and must never retain the
        // hardware successor address.
        || segment.next_descriptor_address != 0
        || ring.accepted_tail() != 0
        || ring.reload_pending()
    {
        return 0;
    }
    if (scenario == 1) != (async_edges == 2) || (scenario != 1 && async_edges != 0) {
        return 0;
    }
    0x8000_0001 | async_edges << 8 | u32::from(DESCRIPTOR_BYTES == 12) << 1
}

fn management_response_header(frame_control: u8, local: [u8; 6], bssid: [u8; 6]) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0] = frame_control;
    frame[4..10].copy_from_slice(&local);
    frame[10..16].copy_from_slice(&bssid);
    frame[16..22].copy_from_slice(&bssid);
    frame
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaJoinProbePhase {
    Idle,
    Authentication,
    Association,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaJoinProbeError {
    ReceiveAlreadyStarted,
    ReceiveNotStarted,
}

struct StaJoinProbeBackend {
    scenario: u32,
    local: [u8; 6],
    bssid: [u8; 6],
    phase: StaJoinProbePhase,
    receive_live: bool,
    auth_polls: u32,
    association_polls: u32,
    auth_attempts: u16,
    association_attempts: u16,
    starts: u16,
    stops: u16,
    valid: bool,
}

impl StaJoinProbeBackend {
    const fn new(scenario: u32, local: [u8; 6], bssid: [u8; 6]) -> Self {
        Self {
            scenario,
            local,
            bssid,
            phase: StaJoinProbePhase::Idle,
            receive_live: false,
            auth_polls: 0,
            association_polls: 0,
            auth_attempts: 0,
            association_attempts: 0,
            starts: 0,
            stops: 0,
            valid: true,
        }
    }
}

impl StaJoinBackend for StaJoinProbeBackend {
    type Error = StaJoinProbeError;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            Err(StaJoinProbeError::ReceiveAlreadyStarted)
        } else {
            self.receive_live = true;
            self.starts += 1;
            Ok(())
        };
        ready(result)
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            self.receive_live = false;
            self.phase = StaJoinProbePhase::Idle;
            self.stops += 1;
            Ok(())
        } else {
            Err(StaJoinProbeError::ReceiveNotStarted)
        };
        ready(result)
    }

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        self.phase = StaJoinProbePhase::Authentication;
        self.auth_attempts += 1;
        self.valid &= attempt.ordinal == self.auth_attempts;
        ready(Ok(()))
    }

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        const EXPECTED_ELAPSED: [u32; 7] = [0, 160, 320, 480, 640, 800, 960];
        self.phase = StaJoinProbePhase::Association;
        let index = usize::from(self.association_attempts);
        self.association_attempts += 1;
        self.valid &= index < EXPECTED_ELAPSED.len()
            && attempt.ordinal == self.association_attempts
            && attempt.elapsed_ms == EXPECTED_ELAPSED[index];
        ready(Ok(()))
    }

    fn service_receive<'a, O>(
        &'a mut self,
        observer: &'a mut O,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        O: StaJoinRxObserver + 'a,
    {
        let result = if !self.receive_live {
            Err(StaJoinProbeError::ReceiveNotStarted)
        } else {
            match self.phase {
                StaJoinProbePhase::Authentication => {
                    self.auth_polls += 1;
                    if self.scenario == 0 && self.auth_polls == 1 {
                        let mut response = management_response_header(0xb0, self.local, self.bssid);
                        response[26..28].copy_from_slice(&2_u16.to_le_bytes());
                        let _ = observer.observe_completed(Some(&response));
                    }
                }
                StaJoinProbePhase::Association => {
                    self.association_polls += 1;
                    if self.scenario == 2 && self.association_polls == 1 {
                        let mut response = management_response_header(0x10, self.local, self.bssid);
                        response[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
                        response[28..30].copy_from_slice(&0xc123_u16.to_le_bytes());
                        let _ = observer.observe_completed(Some(&response));
                    }
                }
                StaJoinProbePhase::Idle => {}
            }
            Ok(())
        };
        ready(result)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StaJoinProbeTimer {
    now_micros: u64,
    waits: u32,
    valid: bool,
}

impl StaJoinTimer for StaJoinProbeTimer {
    fn now_micros(&self) -> u64 {
        self.now_micros
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.valid &= deadline_micros >= self.now_micros;
        self.now_micros = deadline_micros;
        self.waits += 1;
        ready(())
    }
}

/// Compiled production-executor probe for the ordinary infrastructure-STA
/// portion of vendor `ieee80211_sta_new_state`.
///
/// Scenarios cover first-attempt Open Authentication success (0), the exact
/// three-attempt source-owned timeout policy (1), Association success (2),
/// and the complete 1,000-ms Association deadline with seven scheduled MPDUs
/// (3). The probe runs the production `StaJoinRunner` with a finite test
/// PAC/DMA adapter and monotonic clock; it deliberately has no NVS, logging,
/// callback table, RTOS or vendor MAC/DMA layout.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libnet80211_trace_sta_join_state(scenario: u32) -> u32 {
    let local = core::hint::black_box([0x02, 0, 0, 0, 0, 1]);
    let bssid = core::hint::black_box([0x10, 0x11, 0x12, 0x13, 0x14, 0x15]);
    let backend = StaJoinProbeBackend::new(scenario, local, bssid);
    let timer = StaJoinProbeTimer {
        now_micros: 0,
        waits: 0,
        valid: true,
    };
    let mut runner = StaJoinRunner::new(backend, timer);
    let mut sequence = StaSequenceCounter::new(core::hint::black_box(0x123));

    match scenario {
        0 => {
            let result =
                embassy_futures::block_on(runner.authenticate(local, bssid, &mut sequence));
            let (backend, timer) = runner.into_parts();
            match result {
                Ok(success)
                    if success.attempt == 1
                        && success.total_received_frames == 1
                        && sequence.peek() == 0x124
                        && backend.valid
                        && !backend.receive_live
                        && backend.auth_attempts == 1
                        && backend.starts == 1
                        && backend.stops == 1
                        && timer.valid
                        && timer.now_micros == 1_000
                        && timer.waits == 1 =>
                {
                    0xa001_0124
                }
                _ => 0,
            }
        }
        1 => {
            let result =
                embassy_futures::block_on(runner.authenticate(local, bssid, &mut sequence));
            let (backend, timer) = runner.into_parts();
            match result {
                Err(StaJoinError::AuthenticationFailed {
                    attempts: 3,
                    failure: StaAuthenticationFailure::Timeout,
                    total_received_frames: 0,
                }) if sequence.peek() == 0x126
                    && backend.valid
                    && !backend.receive_live
                    && backend.auth_attempts == 3
                    && backend.starts == 3
                    && backend.stops == 3
                    && timer.valid
                    && timer.now_micros == 3_000_000
                    && timer.waits == 3_000 =>
                {
                    0xa103_0126
                }
                _ => 0,
            }
        }
        2 => {
            let result = embassy_futures::block_on(runner.associate(local, bssid, &mut sequence));
            let (backend, timer) = runner.into_parts();
            match result {
                Ok(success)
                    if success.response.status_code == 0
                        && success.response.association_id == 0x123
                        && success.total_received_frames == 1
                        && sequence.peek() == 0x124
                        && backend.valid
                        && backend.receive_live
                        && backend.association_attempts == 1
                        && backend.starts == 1
                        && backend.stops == 0
                        && timer.valid
                        && timer.now_micros == 1_000
                        && timer.waits == 1 =>
                {
                    0xb123_0124
                }
                _ => 0,
            }
        }
        3 => {
            let result = embassy_futures::block_on(runner.associate(local, bssid, &mut sequence));
            let (backend, timer) = runner.into_parts();
            match result {
                Err(StaJoinError::AssociationFailed {
                    failure: StaAssociationFailure::Timeout,
                    total_received_frames: 0,
                }) if sequence.peek() == 0x12a
                    && backend.valid
                    && !backend.receive_live
                    && backend.association_attempts == 7
                    && backend.starts == 1
                    && backend.stops == 1
                    && timer.valid
                    && timer.now_micros == 1_000_000
                    && timer.waits == 1_000 =>
                {
                    0xb107_03e8
                }
                _ => 0,
            }
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_tx_set_cca(value: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_tx_set_cca(value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state() -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_get_txq_in_trig_flow_state()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_is_txq_enabled(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_is_txq_enabled(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_is_txq_valid(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_is_txq_valid(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_set_txq_invalid(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_set_txq_invalid(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_txq_disable(queue: u32) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_txq_disable(queue)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_txq_publish_owned(queue: u32) -> u32 {
    let queue = match queue {
        0 => LegacyTxQueue::Voice,
        1 => LegacyTxQueue::Video,
        2 => LegacyTxQueue::BestEffort,
        3 => LegacyTxQueue::Background,
        _ => return 0,
    };
    // SAFETY: each executor scenario starts from a fresh probe image. The
    // production DMA owner consumes this unique static allocation and keeps
    // it pinned through the complete hardware-ownership interval.
    let storage = unsafe { &mut *core::ptr::addr_of_mut!(OPEN_LIBPP_TX_STORAGE) };
    let dma = match TxDmaStorage::pin_static(storage) {
        Ok(dma) => dma,
        Err(_) => return 0,
    };
    let mut slot = TxSlot::from_dma(dma);
    let mut slot = Pin::new(&mut slot);
    if slot.as_mut().buffer_mut().is_err() {
        return 0;
    }
    let cookie = match slot.as_mut().reserve(64, 32) {
        Ok(cookie) => cookie,
        Err(_) => return 0,
    };
    let Some(config) = LegacyTxConfig::management_1m_from_mpdu_length(32) else {
        return 0;
    };
    let descriptor_address = slot.as_ref().descriptor_address();
    let Some(image) = legacy_q0_image(descriptor_address, config) else {
        return 0;
    };
    if image.plcp0 & 0x000f_ffff != descriptor_address & 0x000f_ffff {
        return 0x1000_0000 | (image.plcp0 & 0x000f_ffff);
    }
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    if slot
        .as_mut()
        .submit_legacy(&mut registers, cookie, queue, config)
        .is_err()
        || slot.state() != TxSlotState::HardwareOwned
    {
        return 0;
    }
    slot.descriptor_word0()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_tx_trace_hal_mac_tx_config_edca(
    _vendor_config_address: u32,
    queue: u32,
    aifsn: u32,
    contention_window: u32,
    interface: open_esp_radio_esp32s31_pac::MacInterface,
) -> u32 {
    // The vendor side decodes these semantic arguments from its pointer-rich
    // ABI object. The Rust probe receives the reviewed projection directly;
    // every case keeps both representations and exact MMIO comparison proves
    // that the projection agrees with the vendor object.
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::hal_mac_tx_config_edca(
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
    let payload = open_esp_radio_esp32s31_pac::validation::hal_mac_tx_get_blockack(queue as u8)
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

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_interface_trace_hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
    open_esp_radio_esp32s31_pac::validation::hal_mac_set_addr(interface, address);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_interface_trace_hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
    open_esp_radio_esp32s31_pac::validation::hal_mac_set_bssid(interface, address);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_coex_trace_hal_set_rx_beacon_pti(beacon: u32, shared: u32) {
    open_esp_radio_esp32s31_pac::validation::hal_set_rx_beacon_pti(beacon, shared);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_coex_trace_hal_clear_rx_beacon_pti() {
    open_esp_radio_esp32s31_pac::validation::hal_clear_rx_beacon_pti();
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_coex_trace_hal_set_itwt_pti(control: u32, shared: u32) {
    open_esp_radio_esp32s31_pac::validation::hal_set_itwt_pti(control, shared);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_coex_trace_hal_clr_itwt_pti(index: u32) {
    open_esp_radio_esp32s31_pac::validation::hal_clr_itwt_pti(index);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_coex_trace_hal_set_tx_pti(
    queue: u32,
    scheduler_priority: u32,
    pti_2: u32,
    pti_1: u32,
    pti_0: u32,
    pti_3: u32,
    count: u32,
) {
    open_esp_radio_esp32s31_pac::validation::hal_set_tx_pti(
        queue,
        scheduler_priority,
        pti_2,
        pti_1,
        pti_0,
        pti_3,
        count,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_beacon_miss_timeout(
    value: u32,
) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_beacon_miss_timeout(value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_beacon_miss_limit(
    value: u32,
) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_beacon_miss_limit(value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable()
 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable(0,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_state_sleep_limit(
    value: u32,
) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_state_sleep_limit(value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable()
 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ = open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable(0,
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_state_wakeup_protect_enable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ =
        open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_state_wakeup_protect_enable(
            0,
        );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_state_wakeup_protect_early_time(
    value: u32,
) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_state_wakeup_protect_early_time(
        value,
    )
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_tbtt_auto_period_enable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ =
        open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_tbtt_auto_period_enable(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_tbtt_auto_period_disable() {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    let _ =
        open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_tbtt_auto_period_disable(0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_trace_pwr_hal_set_mac_modem_tbtt_auto_period_interval(
    value: u32,
) -> u32 {
    // SAFETY: this validation-only function is the sole user of the stolen
    // peripheral in its isolated probe image.
    open_esp_radio_esp32s31_pac::validation::pwr_hal_set_mac_modem_tbtt_auto_period_interval(value)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_power_tsf_trace_hal_set_sta_tsf_wakeup(enabled: u32) {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    registers.set_station_tsf_wakeup(enabled != 0);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_ap_tsf_trace_hal_disable_softap_tsf() {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut hardware = WifiMacHal::new(&mut registers);
    stop_access_point_tsf(&mut hardware);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset(selector: u32) {
    if selector == 0 {
        let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
        let mut hardware = WifiMacHal::new(&mut registers);
        reset_and_start_access_point_tsf(&mut hardware);
    }
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_btbb_coex_trace_coex_pti_v2() {
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    open_esp_radio_esp32s31_hal::coex::configure_bluetooth_pti(&mut registers);
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_scheduler_trace_interval_set(value: u32, output: *mut u32) -> u32 {
    let mut scheduler = open_esp_radio_esp32s31_coex::CoexScheduler::new();
    scheduler.set_interval(value);
    // SAFETY: executable profiles provide one aligned writable scratch word.
    unsafe { output.write(scheduler.interval()) };
    0
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_scheduler_trace_interval_get(value: u32) -> u32 {
    let mut scheduler = open_esp_radio_esp32s31_coex::CoexScheduler::new();
    scheduler.set_interval(value);
    scheduler.interval()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_scheduler_trace_current_period(active: u32, period: u32) -> u32 {
    use open_esp_radio_esp32s31_coex::{CoexPhase, CoexSchedule, CoexScheduler};

    let phases = [CoexPhase::from_reviewed_image([0; 4])];
    let schedule = CoexSchedule::new(period as u8, &phases);
    let mut scheduler = CoexScheduler::new();
    if active != 0 {
        scheduler.activate(&schedule);
    }
    u32::from(scheduler.current_period())
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_coex_scheduler_trace_current_phase(
    active: u32,
    phase_count: u32,
    phase_index: u32,
    vendor_layout_base: u32,
) -> u32 {
    use open_esp_radio_esp32s31_coex::{CoexPhase, CoexSchedule, CoexScheduler};

    let phases = [
        CoexPhase::from_reviewed_image([0x11; 4]),
        CoexPhase::from_reviewed_image([0x22; 4]),
    ];
    let phase_count = usize::try_from(phase_count)
        .unwrap_or(usize::MAX)
        .min(phases.len());
    let schedule = CoexSchedule::new(1, &phases[..phase_count]);
    let mut scheduler = CoexScheduler::new();
    if active != 0 {
        scheduler.activate(&schedule);
    }
    scheduler.set_phase_index(phase_index as u8);
    if scheduler.current_phase().is_some() {
        vendor_layout_base.wrapping_add(2 + phase_index.wrapping_mul(4))
    } else {
        0
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
    open_esp_radio_esp32s31_pac::validation::hal_get_sta_tsf(low, high);
}

struct ProbeIrqSink {
    posted: Cell<u32>,
    unhandled: Cell<u32>,
}

impl IrqSink for ProbeIrqSink {
    fn post(&self, mac_pending: u32) {
        self.posted.set(mac_pending);
    }

    fn record_unhandled(&self, bits: u32) {
        self.unhandled.set(bits);
    }
}

/// Compiled semantic probe for the minimal MAC slice of `wDev_ProcessFiq`.
///
/// Encoding: bits 0..1 disposition, bit 2 acknowledge-called, bit 7 exact
/// acknowledge value, bits 3..6 work count, bits 8..23 ordered work nibbles,
/// and bit 31 exact unhandled image. The host adapter compares this against
/// modeled vendor call boundaries. Unlike the leaf probes, this composed probe
/// uses the real production `MacInterruptRegisters` capability and therefore
/// exposes the complete STATUS -> CLEAR -> fence transaction as MMIO evidence.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_libpp_trace_wdev_process_fiq_mac_slice() -> u32 {
    let mut interrupt = open_esp_radio_esp32s31_pac::validation::mac_interrupt_registers();
    let sink = ProbeIrqSink {
        posted: Cell::new(0),
        unhandled: Cell::new(0),
    };
    let (disposition, snapshot) = handle_mac_irq(&mut interrupt, &sink);
    if snapshot.status != 0 {
        // Reproduce the bounded production hard-ISR drain loop for the pilot's
        // one-event scenarios. The host supplies zero for the second STATUS
        // read and fails closed if the transaction consumes another response.
        let (_, drained) = handle_mac_irq(&mut interrupt, &sink);
        if drained.status != 0 {
            return 0;
        }
    }
    let disposition = match disposition {
        IrqDisposition::Posted => 1,
        IrqDisposition::Spurious => 2,
        IrqDisposition::AcknowledgedOnly => 3,
    };
    let mut encoded = disposition;
    if snapshot.status != 0 {
        encoded |= 1 << 2;
    }
    // Exact acknowledgement is proved independently by the emitted CLEAR
    // value; retain the bit so the semantic encoding remains stable.
    encoded |= 1 << 7;
    if sink.unhandled.get()
        == snapshot.status & !(HANDLED_MAC_MASK | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK)
    {
        encoded |= 1 << 31;
    }
    let mut pending = sink.posted.get();
    let mut count = 0_u32;
    while let Some(work) = next_irq_work(pending) {
        let code = match work {
            IrqWork::RxSuccess => 1,
            IrqWork::TxComplete => 2,
            IrqWork::TxTimeout => 3,
            IrqWork::Collision => 4,
        };
        encoded |= code << (8 + count * 4);
        count += 1;
        pending &= !work.mac_bit();
    }
    encoded | count << 3
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
    registers: &mut RadioRegisters,
) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_hccfr(
        registers,
        enabled & 1 != 0,
        cfr_value_from_vendor_argument(value),
    );
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_trace_phy_iccfr_en(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_baseband::configure_iccfr_gate(registers, input != 0);
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
pub extern "C" fn open_phy_trace_ant_dft_cfg(input: u32, registers: &mut RadioRegisters) {
    open_esp_radio_esp32s31_hal::phy_agc::configure_antenna_diversity(registers, input & 1 != 0);
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
    open_esp_radio_esp32s31_hal::phy_agc::configure_forced_rx_gain(
        registers,
        enabled & 1 != 0,
        open_esp_radio_esp32s31_hal::ForcedRxGain::new(gain as u8),
    );
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
    core::hint::black_box(open_libpp_trace_hal_mac_interrupt_ret_get_event as *const ());
    core::hint::black_box(open_libpp_trace_hal_mac_interrupt_ret_clr_event as *const ());
    core::hint::black_box(open_wifi_sta_trace_hal_disable_sta_beacon_filter as *const ());
    core::hint::black_box(open_wifi_key_role_trace_wdev_insert_key_entry as *const ());
    core::hint::black_box(open_libpp_power_irq_trace_hal_pwr_interrupt_get_event as *const ());
    core::hint::black_box(open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event as *const ());
    core::hint::black_box(open_libpp_trace_wdev_process_fiq_mac_slice as *const ());
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
    core::hint::black_box(open_libpp_rx_trace_wdev_append_rx_blocks as *const ());
    core::hint::black_box(open_libnet80211_trace_sta_join_state as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_tx_set_cca as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_is_txq_enabled as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_is_txq_valid as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_set_txq_invalid as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_txq_disable as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_txq_publish_owned as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_tx_config_edca as *const ());
    core::hint::black_box(open_libpp_tx_trace_hal_mac_tx_get_blockack as *const ());
    core::hint::black_box(open_libpp_interface_trace_hal_mac_set_addr as *const ());
    core::hint::black_box(open_libpp_interface_trace_hal_mac_set_bssid as *const ());
    core::hint::black_box(open_libpp_coex_trace_hal_set_rx_beacon_pti as *const ());
    core::hint::black_box(open_libpp_coex_trace_hal_clear_rx_beacon_pti as *const ());
    core::hint::black_box(open_libpp_coex_trace_hal_set_itwt_pti as *const ());
    core::hint::black_box(open_libpp_coex_trace_hal_clr_itwt_pti as *const ());
    core::hint::black_box(open_libpp_coex_trace_hal_set_tx_pti as *const ());
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_beacon_miss_timeout as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_beacon_miss_limit as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_beacon_miss_limit_exceeded_wakeup_enable
            as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_state_sleep_limit as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_state_sleep_limit_exceeded_wakeup_enable
            as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_state_wakeup_protect_enable as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_state_wakeup_protect_early_time as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_tbtt_auto_period_enable as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_tbtt_auto_period_disable as *const (),
    );
    core::hint::black_box(
        open_libpp_power_trace_pwr_hal_set_mac_modem_tbtt_auto_period_interval as *const (),
    );
    core::hint::black_box(open_libpp_power_tsf_trace_hal_set_sta_tsf_wakeup as *const ());
    core::hint::black_box(open_libpp_ap_tsf_trace_hal_disable_softap_tsf as *const ());
    core::hint::black_box(open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset as *const ());
    core::hint::black_box(open_btbb_coex_trace_coex_pti_v2 as *const ());
    core::hint::black_box(open_coex_scheduler_trace_interval_set as *const ());
    core::hint::black_box(open_coex_scheduler_trace_interval_get as *const ());
    core::hint::black_box(open_coex_scheduler_trace_current_period as *const ());
    core::hint::black_box(open_coex_scheduler_trace_current_phase as *const ());
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
