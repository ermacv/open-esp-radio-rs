#![no_std]

//! Link-time probes for compiled vendor/Rust MMIO comparison.
//!
//! These wrappers are test-harness artifacts, never driver entry points. Fat
//! LTO inlines the safe HAL leaf into each retained symbol so Blobray
//! verifier can compare the resulting instruction-level MMIO transaction
//! sequence.

use oer_esp32s31_hal::owner::RadioRuntimeOwner;
// Leaf-level PHY comparison probes receive only a borrowed protocol-neutral
// PHY partition. They cannot acquire, release, or recover the complete Wi-Fi
// owner. Release-relevant production probes below acquire opaque HAL owners.

use oer_esp32s31_pac::RadioPhyRegisters;

use oer_esp32s31_ieee80211_mac::ap_tsf::{reset_and_start_access_point_tsf, stop_access_point_tsf};

mod calibration_leaves;
mod i2c;
mod production_trace;

/// Expose externally supplied PHY registers through the HAL shared-PHY port
/// with a fresh per-call restore slot.
fn shared_phy(
    registers: &mut RadioPhyRegisters,
) -> oer_esp32s31_hal::owner::ValidationSharedPhy<'_> {
    oer_esp32s31_hal::owner::ValidationSharedPhy::new(registers)
}

/// Borrow an isolated shared radio-PHY partition.
/// The closure cannot return a borrow of this temporary validation capability.
fn with_phy<R>(call: impl FnOnce(&mut RadioPhyRegisters) -> R) -> R {
    let mut registers = oer_esp32s31_pac::validation::shared_radio_registers();
    call(registers.radio_phy_mut())
}

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
#[allow(
    clippy::disallowed_methods,
    reason = "isolated verification image has no executor; panic is a terminal probe failure"
)]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Harness-only delay edge intercepted by the Blobray verifier before this body runs.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn ets_delay_us(micros: u32) {
    core::hint::black_box(micros);
}

oer_probe_macros::probe! {
    /// Harness delay adapter preserving integer inputs around the explicit event edge.
    /// The guest body executes these saves/restores; the inner model supplies only
    /// requested-time observation, never production completion or register values.
    #[unsafe(naked)]
    pub fn open_phy_trace_preserving_delay(_micros: u32) {
        core::arch::naked_asm!(
            "addi sp, sp, -64",
            "sw ra, 0(sp)",
            "sw t0, 4(sp)",
            "sw t1, 8(sp)",
            "sw t2, 12(sp)",
            "sw t3, 16(sp)",
            "sw t4, 20(sp)",
            "sw t5, 24(sp)",
            "sw t6, 28(sp)",
            "sw a0, 32(sp)",
            "sw a1, 36(sp)",
            "sw a2, 40(sp)",
            "sw a3, 44(sp)",
            "sw a4, 48(sp)",
            "sw a5, 52(sp)",
            "sw a6, 56(sp)",
            "sw a7, 60(sp)",
            "call open_phy_trace_delay_event",
            "lw ra, 0(sp)",
            "lw t0, 4(sp)",
            "lw t1, 8(sp)",
            "lw t2, 12(sp)",
            "lw t3, 16(sp)",
            "lw t4, 20(sp)",
            "lw t5, 24(sp)",
            "lw t6, 28(sp)",
            "lw a0, 32(sp)",
            "lw a1, 36(sp)",
            "lw a2, 40(sp)",
            "lw a3, 44(sp)",
            "lw a4, 48(sp)",
            "lw a5, 52(sp)",
            "lw a6, 56(sp)",
            "lw a7, 60(sp)",
            "addi sp, sp, 64",
            "ret",
        );
    }
}

oer_probe_macros::probe! {
    /// Modeled requested-delay observation, reached through the captured ABI adapter.
    pub fn open_phy_trace_delay_event(micros: u32) {
        core::hint::black_box(micros);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_bt_index_to_bb(index: u32) -> u32 =>
        oer_esp32s31_phy::calibration::bluetooth::bluetooth_gain_index_to_baseband(index);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_bt_bb_to_index(baseband: u32) -> u32 =>
        oer_esp32s31_phy::calibration::bluetooth::bluetooth_baseband_to_gain_index(baseband);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_disable_agc(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_enabled(&mut shared_phy(registers), false);
}

oer_probe_macros::probe! {
    pub fn open_libpp_trace_hal_mac_interrupt_ret_get_event() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_interrupt_get_event()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_trace_hal_mac_interrupt_ret_clr_event(events: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_interrupt_clr_event(events)
    }
}

oer_probe_macros::probe! {
    pub fn open_wifi_sta_trace_hal_disable_sta_beacon_filter() {
        // SAFETY: these validation-only PAC capabilities are used only by this
        // isolated probe image. The called helper is the production transaction.
        oer_esp32s31_hal::validation::hal_disable_sta_beacon_filter();
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_power_irq_trace_hal_pwr_interrupt_get_event() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_pwr_interrupt_get_event()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event(events: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_pwr_interrupt_clr_event(events)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_disable() {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_disable(0);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_enable() {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_enable(0);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_set_base(address: u32) {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_set_base(address);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_is_dscr_reload() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_rx_is_dscr_reload()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_set_dscr_reload() {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_set_dscr_reload(0);
    }
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_enable(index: u32) =>
        oer_esp32s31_coex::validation::enable_timer(index);
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_disable(index: u32) =>
        oer_esp32s31_coex::validation::disable_timer(index);
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_force(index: u32) =>
        oer_esp32s31_coex::validation::force_timer(index);
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_unforce(index: u32) =>
        oer_esp32s31_coex::validation::unforce_timer(index);
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for the complete `coex_core_pti_get`
    /// contract. The vendor ABI returns `0x102` for a null output pointer and
    /// otherwise copies one entry from its reviewed 48-byte priority table.
    ///
    /// # Safety
    /// A non-null `output` must point to one exclusively writable byte.
    pub unsafe fn open_coex_core_trace_pti_get(event: u32, output: *mut u8) -> u32 {
        if output.is_null() {
            return 0x102;
        }
        let Ok(event) = oer_esp32s31_coex::CoexEventId::new(event as u8) else {
            return 0x102;
        };
        let pti = oer_esp32s31_coex::CoexPtiTable::reviewed_vendor().pti(event);
        // SAFETY: verification profiles provide a writable caller-owned output
        // byte and compare its final state with the vendor execution.
        unsafe { output.write(pti.value()) };
        0
    }
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for `coex_core_event_duration_get`.
    /// The vendor leaf accepts only the five events backed by `g_coex_param`,
    /// clears a non-null output for every other event, and uses `u32::MAX` as its
    /// invalid-argument status.
    ///
    /// # Safety
    /// A non-null `output` must point to an aligned, exclusively writable word.
    pub unsafe fn open_coex_core_trace_event_duration_get(event: u32, output: *mut u32) -> u32 {
        if output.is_null() {
            return u32::MAX;
        }
        let duration = oer_esp32s31_coex::CoexEventId::new(event as u8)
            .ok()
            .and_then(|event| oer_esp32s31_coex::CoexEventDurations::reviewed_vendor().duration(event));
        // SAFETY: verification profiles provide a writable caller-owned output
        // word and compare its final state with the vendor execution.
        unsafe { output.write(duration.unwrap_or(0)) };
        if duration.is_some() { 0 } else { u32::MAX }
    }
}

oer_probe_macros::probe! {
    /// Compiled production-path projection of the complete vendor event-to-timer
    /// switch. `0xff` is the exact unmapped sentinel returned by the vendor leaf.
    pub fn open_coex_core_trace_timer_idx_get(event: u32) -> u32 {
        let Ok(event) = oer_esp32s31_coex::CoexEventId::new(event as u8) else {
            return 0xff;
        };
        event
            .timer_index()
            .map_or(0xff, |index| u32::from(index.value()))
    }
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for the complete `coex_hw_timer_set`
    /// transaction. The public vendor ABI is `(index, client, pti, latency,
    /// duration)`; notably, duration is written to the primary word before
    /// latency is written to the secondary word.
    pub fn open_coex_set_trace_coex_hw_timer_set(
        index: u32,
        client: u32,
        pti: u32,
        latency: u32,
        duration: u32,
        is_real_chip: u32,
    ) {
        use oer_esp32s31_coex::{CoexClient, CoexPti, CoexTimerIndex};

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
        let _ = oer_esp32s31_coex::validation::program_timer(
            is_real_chip != 0,
            index,
            client,
            pti,
            latency,
            duration,
        );
    }
}

oer_probe_macros::probe! {
    /// Complete register projection of `coex_core_request` under the reviewed
    /// Rust lifecycle precondition that the COEX owner is enabled.
    pub fn open_coex_core_trace_request(
        client: u32,
        event: u32,
        latency: u32,
        duration: u32,
        is_real_chip: u32,
    ) -> u32 {
        use oer_esp32s31_coex::{CoexClientRequest, CoexError, CoexEventId};

        let Ok(event) = CoexEventId::new(event as u8) else {
            return 0x102;
        };
        let request = CoexClientRequest {
            event,
            latency,
            duration,
        };
        let result =
            oer_esp32s31_coex::validation::core_request(is_real_chip != 0, client != 0, request);
        match result {
            Ok(_) => 0,
            Err(CoexError::InvalidEvent) => 0x102,
            Err(_) => u32::MAX,
        }
    }
}

oer_probe_macros::probe! {
    /// Complete register projection of `coex_core_release`. The vendor ABI keeps
    /// the client in `a0` but selects the timer exclusively from event `a1`.
    pub fn open_coex_core_trace_release(_client: u32, event: u32) -> u32 {
        use oer_esp32s31_coex::{CoexError, CoexEventId};

        let Ok(event) = CoexEventId::new(event as u8) else {
            return 0x102;
        };
        match oer_esp32s31_coex::validation::core_release(event) {
            Ok(_) => 0,
            Err(CoexError::InvalidEvent) => 0x102,
            Err(_) => u32::MAX,
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_tx_set_cca(value: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_tx_set_cca(value)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_get_txq_in_trig_flow_state()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_is_txq_enabled(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_is_txq_enabled(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_is_txq_valid(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_is_txq_valid(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_set_txq_invalid(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_set_txq_invalid(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_txq_disable(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_txq_disable(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_tx_protection_initialize_cts() =>
        oer_esp32s31_hal::validation::initialize_mac_software_cts();
}

oer_probe_macros::probe! {
    pub fn open_tx_protection_control_configure_rts(
        queue: u32,
        enabled: u32,
        _unused: u32,
        duration_threshold: u32,
        threshold_bytes: u32,
    ) {
        assert!(enabled <= 1);
        assert!(duration_threshold <= 1);
        oer_esp32s31_hal::validation::configure_mac_tx_rts(
            queue,
            enabled != 0,
            (duration_threshold != 0).then_some(threshold_bytes as u16),
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_tx_protection_control_disable_he_threshold() =>
        oer_esp32s31_hal::validation::disable_mac_he_rts_threshold();
}

oer_probe_macros::probe! {
    pub fn open_ordinary_tx_ownership_publish(queue: u8) =>
        oer_esp32s31_hal::validation::ordinary_tx_publish(queue);
}

oer_probe_macros::probe! {
    pub fn open_ordinary_tx_ownership_acknowledge(selector: u32, queue: u8) {
        assert_eq!(selector, 2);
        oer_esp32s31_hal::validation::ordinary_tx_acknowledge_completion(queue);
    }
}

oer_probe_macros::probe! {
    pub fn open_ordinary_tx_ownership_disable(queue: u32) -> u32 =>
        oer_esp32s31_hal::validation::hal_mac_txq_disable(queue);
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_tx_config_edca(
        _vendor_config_address: u32,
        queue: u32,
        aifsn: u32,
        contention_window: u32,
        interface: oer_esp32s31_hal::types::MacInterface,
    ) -> u32 {
        // The vendor side decodes these semantic arguments from its pointer-rich
        // ABI object. The Rust probe receives the reviewed projection directly;
        // every case keeps both representations and exact MMIO comparison proves
        // that the projection agrees with the vendor object.
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_tx_config_edca(
            queue,
            aifsn as u8,
            contention_window as u16,
            interface,
        )
    }
}

#[repr(C)]
struct CanonicalHtTxParameters {
    queue: u32,
    descriptor_head: u32,
    mcs: u32,
    guard_interval: u32,
    channel_width: u32,
    format: u32,
    length: u32,
    descriptor_count: u32,
    data_power_primary: u32,
    data_power_alternate: u32,
    rts_power_primary: u32,
    rts_power_alternate: u32,
    protection_spacing: u32,
    timeout: u32,
    scheduler_priority: u32,
    packet_priority: u32,
    priority_count: u32,
    aifsn: u32,
    contention_window: u32,
    interface: u32,
    hardware_key_selector: u32,
    txop: u32,
}

struct ValidationPreparedTxDma(u32);

// SAFETY: this verification-only authority is instantiated solely from the
// profile-owned stable descriptor memory used by the isolated instruction
// trace. It is never exposed to production code or real hardware.
unsafe impl oer_memory::PreparedTxDma for ValidationPreparedTxDma {
    fn descriptor_head(&self) -> u32 {
        self.0
    }
}

oer_probe_macros::probe! {
    /// Reviewed ABI projection around the exact compiled production HT queue
    /// transaction. The vendor side receives its pointer-rich PP context at `a0`;
    /// the Rust side receives semantic HT parameters at the same address. The
    /// PAC constructs every register word from those reviewed values.
    pub fn open_libpp_tx_trace_hal_mac_tx_set_ppdu(
        program_address: u32,
        _vendor_auxiliary: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::{
            MacHtChannelWidth, MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate,
            MacHtTxFormat, MacHtTxParameters, MacHtTxProgram, MacInterface,
        };

        let parameters = program_address as *const CanonicalHtTxParameters;
        // SAFETY: the verification profile supplies one initialized, aligned and
        // immutable CanonicalHtTxParameters for the duration of this call.
        let parameters = unsafe { &*parameters };
        let interface = match parameters.interface {
            0 => MacInterface::Station,
            1 => MacInterface::AccessPoint,
            2 => MacInterface::Context2,
            3 => MacInterface::Context3,
            _ => panic!("verification MAC interface is out of range"),
        };
        let mcs = match parameters.mcs {
            0 => MacHtMcs::Mcs0,
            1 => MacHtMcs::Mcs1,
            2 => MacHtMcs::Mcs2,
            3 => MacHtMcs::Mcs3,
            4 => MacHtMcs::Mcs4,
            5 => MacHtMcs::Mcs5,
            6 => MacHtMcs::Mcs6,
            7 => MacHtMcs::Mcs7,
            _ => panic!("verification HT MCS is out of range"),
        };
        let guard_interval = match parameters.guard_interval {
            0 => MacHtGuardInterval::Long800Ns,
            1 => MacHtGuardInterval::Short400Ns,
            _ => panic!("verification HT guard interval is out of range"),
        };
        let channel_width = match parameters.channel_width {
            0 => MacHtChannelWidth::Mhz20,
            1 => MacHtChannelWidth::Mhz40,
            _ => panic!("verification HT channel width is out of range"),
        };
        let format = match parameters.format {
            0 => MacHtTxFormat::SingleMpdu,
            1 => MacHtTxFormat::Ampdu,
            _ => panic!("verification HT format is out of range"),
        };
        let protection_spacing = match parameters.protection_spacing {
            0 => MacHtProtectionSpacing::Density0To4,
            1 => MacHtProtectionSpacing::Density5,
            2 => MacHtProtectionSpacing::Density6,
            3 => MacHtProtectionSpacing::Density7,
            _ => panic!("verification HT protection spacing is out of range"),
        };
        let dma = ValidationPreparedTxDma(parameters.descriptor_head);
        let program = MacHtTxProgram::new(
            &dma,
            MacHtTxParameters {
                protection: oer_esp32s31_pac::MacTxProtection::None,
                rate: MacHtRate {
                    mcs,
                    guard_interval,
                    channel_width,
                },
                format,
                length: parameters.length as u16,
                descriptor_count: parameters.descriptor_count as u8,
                data_power_primary: parameters.data_power_primary as u8,
                data_power_alternate: parameters.data_power_alternate as u8,
                rts_power_primary: parameters.rts_power_primary as u8,
                rts_power_alternate: parameters.rts_power_alternate as u8,
                protection_spacing,
                timeout: parameters.timeout as u16,
                scheduler_priority: parameters.scheduler_priority as u8,
                packet_priority: parameters.packet_priority as u8,
                priority_count: parameters.priority_count as u16,
                aifsn: parameters.aifsn as u8,
                contention_window: parameters.contention_window as u16,
                interface,
                hardware_key_selector: parameters.hardware_key_selector as u8,
                txop: parameters.txop != 0,
            },
        )
        .expect("verification HT parameters are in the reviewed PAC domain");
        oer_esp32s31_hal::validation::hal_mac_tx_set_ppdu(parameters.queue as u8, program)
    }
}

#[repr(C)]
pub struct CanonicalTxBlockAck {
    pub control: u8,
    pub reserved: u8,
    pub starting_sequence: u16,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_tx_get_blockack(queue: u32, output_address: u32) -> u32 {
        let payload = oer_esp32s31_hal::validation::hal_mac_tx_get_blockack(queue as u8)
            .expect("profile constrains ordinary TX queue 0..=3");
        let output = output_address as *mut CanonicalTxBlockAck;
        // SAFETY: the verification profile supplies one initialized, writable
        // twelve-byte output object for the duration of this call.
        unsafe {
            (*output).control = payload.control();
            (*output).starting_sequence = payload.starting_sequence();
            (*output).bitmap_low = payload.bitmap_low();
            (*output).bitmap_high = payload.bitmap_high();
        }
        0
    }
}

oer_probe_macros::probe! {
    /// ABI projection around the exact production normal-rate selector.
    ///
    /// The vendor entry receives a pointer-rich descriptor and stores the chosen
    /// rate at byte `0x0c`. The wrapper performs only that ABI projection; rate
    /// selection is owned by the compiled production function.
    pub fn open_libpp_tx_retry_trace_rc_get_rate(_rate_context: u32, descriptor_address: u32) {
        use oer_esp32s31_ieee80211_mac::tx::{
            LegacyRate, TxPhyRate,
            runtime::{OrdinaryRetryCounters, select_ordinary_retry_rate},
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
}

// These validation-only leaves make the result of the compiled production
// completion classifier observable without introducing a shadow numeric
// encoding. The non-pure inline assembly prevents LLVM from deleting the
// calls; comparison stops at the selected leaf before executing its body.
oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_ack_timeout(queue: u32) {
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
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_cts_timeout(queue: u32) {
        // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
        unsafe {
            core::arch::asm!(
                "addi zero, zero, 2",
                in("a0") queue,
                options(nomem, nostack)
            )
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_collision(queue: u32) {
        // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
        unsafe {
            core::arch::asm!(
                "addi zero, zero, 3",
                in("a0") queue,
                options(nomem, nostack)
            )
        };
    }
}

oer_probe_macros::probe! {
    /// ABI projection around the exact production status-four classifier.
    pub fn open_libpp_tx_retry_trace_lmac_process_tx_error(queue: u32, detail: u32, _selector: u32) {
        use oer_esp32s31_ieee80211_mac::tx::{TxCompletion, TxCompletionDisposition, TxCookie};

        let completion = TxCompletion::new_validation(TxCookie(0), 4, detail as u8);
        match completion.disposition() {
            TxCompletionDisposition::AckTimeout => open_libpp_tx_retry_ack_timeout(queue),
            TxCompletionDisposition::CtsTimeout => open_libpp_tx_retry_cts_timeout(queue),
            TxCompletionDisposition::Collision => open_libpp_tx_retry_collision(queue),
            TxCompletionDisposition::Success | TxCompletionDisposition::Terminal(_) => {}
        }
    }
}

#[repr(C)]
pub struct CanonicalOrdinaryRetryState {
    pub mpdu: u8,
    pub short: u8,
    pub long: u8,
    pub publications: u8,
    pub selected_rate: u8,
    pub decision: u8,
    pub retry_bit_mask: u8,
    pub contention_exponent: u8,
}

const ORDINARY_TX_PROBE_BUFFER_SIZE: usize = 256;

struct OrdinaryTxProbePower;

impl oer_esp32s31_ieee80211::tx::WifiTxPowerProfile for OrdinaryTxProbePower {
    fn power_pair(&self, _rate_code: u8) -> oer_esp32s31_ieee80211::tx::WifiTxPowerPair {
        oer_esp32s31_ieee80211::tx::WifiTxPowerPair {
            primary: 1,
            alternate: 1,
        }
    }
}

struct OrdinaryTxProbeEntropy;

impl oer_esp32s31_ieee80211::tx::WifiTxEntropy for OrdinaryTxProbeEntropy {
    fn next_u32(&mut self) -> u32 {
        0
    }
}

struct OrdinaryTxProbeTimer;

impl oer_esp32s31_ieee80211::tx::WifiTxTimer for OrdinaryTxProbeTimer {
    fn now_micros(&self) -> u64 {
        1
    }

    fn wait_until(&mut self, _deadline_micros: u64) -> impl core::future::Future<Output = ()> + '_ {
        core::future::ready(())
    }

    fn after_micros(&mut self, _micros: u64) -> impl core::future::Future<Output = ()> + '_ {
        core::future::ready(())
    }
}

oer_probe_macros::probe! {
    /// Reviewed probe edge emitted by every real production publication.
    pub fn open_libpp_tx_retry_publication(queue: u32) {
        // SAFETY: this instruction is a machine-visible optimizer barrier only.
        unsafe { core::arch::asm!("addi zero, zero, 4", in("a0") queue, options(nomem, nostack)) };
    }
}

struct OrdinaryTxProbeHardware {
    publications: u8,
}

impl oer_esp32s31_ieee80211_mac::tx::TxHardware for OrdinaryTxProbeHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn oer_esp32s31_ieee80211_mac::tx::PreparedTxDma,
        _queue: u8,
        _program: oer_esp32s31_hal::types::MacLegacyTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_legacy_tx(
        &mut self,
        _dma: &dyn oer_esp32s31_ieee80211_mac::tx::HardwareOwnedTxDma,
        queue: u8,
    ) {
        self.publications = self.publications.saturating_add(1);
        if self.publications > 1 {
            open_libpp_tx_retry_publication(u32::from(queue));
        }
    }

    fn take_tx_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacTxCompletionObservation> {
        Some(oer_esp32s31_hal::types::MacTxCompletionObservation::new_validation(5, 0))
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: oer_esp32s31_hal::types::MacTxDetachReason,
        detached: impl for<'detached> FnOnce(
            oer_esp32s31_hal::types::MacTxQueueDetached<'detached>,
        ) -> R,
    ) -> oer_esp32s31_hal::types::MacTxDetachOutcome<R> {
        use oer_esp32s31_hal::types::{MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached};

        match reason {
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_validation(expected_descriptor_head),
            )),
            MacTxDetachReason::Timeout | MacTxDetachReason::Collision => {
                MacTxDetachOutcome::NoEvent
            }
        }
    }
}

type OrdinaryTxProbeOwner = oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxOwner<
    'static,
    OrdinaryTxProbePower,
    OrdinaryTxProbeEntropy,
    OrdinaryTxProbeTimer,
    ORDINARY_TX_PROBE_BUFFER_SIZE,
>;

struct OrdinaryTxProbeState {
    owner: OrdinaryTxProbeOwner,
    hardware: OrdinaryTxProbeHardware,
}

struct OrdinaryTxProbeCell<T>(core::cell::UnsafeCell<T>);

// SAFETY: Blobray executes this probe image on one thread and invokes
// its exported stateful entry serially.
unsafe impl<T> Sync for OrdinaryTxProbeCell<T> {}

#[unsafe(link_section = ".dma.bss.ordinary_tx")]
static ORDINARY_TX_DMA: OrdinaryTxProbeCell<
    oer_esp32s31_ieee80211_dma::tx_storage::TxDmaStorage<ORDINARY_TX_PROBE_BUFFER_SIZE>,
> = OrdinaryTxProbeCell(core::cell::UnsafeCell::new(
    oer_esp32s31_ieee80211_dma::tx_storage::TxDmaStorage::new(),
));
static ORDINARY_TX_SLOT: OrdinaryTxProbeCell<
    core::mem::MaybeUninit<oer_esp32s31_ieee80211_mac::tx::TxSlot<ORDINARY_TX_PROBE_BUFFER_SIZE>>,
> = OrdinaryTxProbeCell(core::cell::UnsafeCell::new(core::mem::MaybeUninit::uninit()));
static ORDINARY_TX_PROBE: OrdinaryTxProbeCell<(
    bool,
    core::mem::MaybeUninit<OrdinaryTxProbeState>,
)> = OrdinaryTxProbeCell(core::cell::UnsafeCell::new((
    false,
    core::mem::MaybeUninit::uninit(),
)));

fn initialize_ordinary_tx_probe() -> Result<OrdinaryTxProbeState, u32> {
    use oer_esp32s31_ieee80211::{
        ordinary_tx::{OrdinaryTxInterface, OrdinaryTxOwner, OrdinaryTxPlan},
        tx::{WifiTxProgress, WifiTxResources},
    };

    use oer_esp32s31_ieee80211_mac::tx::{
        LegacyRate, LegacyTxQueue, TxPhyRate, TxSlot, runtime::WifiTxRuntimePolicy,
    };

    use oer_ieee80211_softmac::MacTxPlan;

    // SAFETY: initialization runs once in the single-threaded probe image;
    // the resulting DMA owner permanently consumes this static allocation.
    let dma = oer_esp32s31_ieee80211_dma::tx_storage::TxDmaStorage::pin_static(unsafe {
        &mut *ORDINARY_TX_DMA.0.get()
    })
    .map_err(|_| 10_u32)?;
    // SAFETY: this storage is initialized once and then borrowed exclusively
    // by the retained `OrdinaryTxOwner` for the rest of the image lifetime.
    let slot = unsafe {
        let slot = &mut *ORDINARY_TX_SLOT.0.get();
        slot.write(TxSlot::from_dma(dma));
        core::pin::Pin::new_unchecked(slot.assume_init_mut())
    };
    let mut owner = OrdinaryTxOwner::new(WifiTxResources {
        slot,
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: OrdinaryTxProbePower,
        entropy: OrdinaryTxProbeEntropy,
        timer: OrdinaryTxProbeTimer,
    });
    owner.buffer_mut().map_err(|_| 11_u32)?[..32].fill(0);
    let mut hardware = OrdinaryTxProbeHardware { publications: 0 };
    let progress = owner
        .start(
            &mut hardware,
            OrdinaryTxPlan {
                frame_length: 24,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::BestEffort.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    publication_limit: 0x20,
                    publication_timeout_micros: 1_000,
                },
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                interface: OrdinaryTxInterface::Station,
                scheduler_priority: 1,
                packet_priority: 1,
            },
        )
        .map_err(|_| 12_u32)?;
    if progress != WifiTxProgress::Pending {
        return Err(13_u32);
    }
    Ok(OrdinaryTxProbeState { owner, hardware })
}

/// Drive one ACK-timeout edge through the exact compiled production TX owner.
#[inline(never)]
fn ordinary_tx_ack_timeout_state(output_address: u32) -> u32 {
    use oer_esp32s31_ieee80211::tx::{WifiTxProgress, WifiTxWake};

    if output_address == 0 {
        return 4;
    }
    // SAFETY: this is the sole accessor in the single-threaded probe image.
    let storage = unsafe { &mut *ORDINARY_TX_PROBE.0.get() };
    if !storage.0 {
        let Ok(state) = initialize_ordinary_tx_probe() else {
            return 5;
        };
        storage.1.write(state);
        storage.0 = true;
    }
    // SAFETY: the branch above initializes the state before every read.
    let state = unsafe { storage.1.assume_init_mut() };
    // Route the hardware completion through the exact production Embassy
    // interrupt handoff. This keeps the compiled comparison from bypassing
    // the adapter boundary by constructing `WifiTxWake` directly.
    let irq = oer_esp32s31_ieee80211_runtime::datapath::irq::EmbassyMacIrqRuntime::<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
    >::new();
    irq.publish(oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE);
    let Some(events) = irq.try_take_tx() else {
        return 8;
    };
    let progress = state
        .owner
        .service(&mut state.hardware, WifiTxWake::Interrupt { events });
    if progress != Ok(WifiTxProgress::Pending) {
        return 6;
    }
    let Ok(Some(snapshot)) = state.owner.active_snapshot() else {
        return 7;
    };
    let output = output_address as *mut CanonicalOrdinaryRetryState;
    // SAFETY: the comparison case supplies this writable eight-byte object.
    unsafe {
        output.write(CanonicalOrdinaryRetryState {
            mpdu: snapshot.counters.mpdu,
            short: snapshot.counters.short,
            long: snapshot.counters.long,
            publications: snapshot.publications,
            selected_rate: snapshot.current_rate.code(),
            decision: 1,
            retry_bit_mask: u8::from(snapshot.retry_bit_set) * 8,
            contention_exponent: state
                .owner
                .policy()
                .contention_exponent(oer_esp32s31_ieee80211_mac::tx::LegacyTxQueue::BestEffort),
        })
    };
    0
}

oer_probe_macros::probe! {
    /// Vendor-ABI-shaped entry for the stateful ACK-timeout short-frame profile.
    /// The fixed output address belongs to the comparison scenario; all retry
    /// decisions and counters come from the exact production state above.
    pub fn open_libpp_tx_retry_trace_ack_timeout_state(_queue: u32, _selector: u32) -> u32 =>
        ordinary_tx_ack_timeout_state(0x3fff_5000);
}

oer_probe_macros::probe! {
    pub fn open_libpp_interface_trace_hal_mac_set_addr(interface: u32, address: &[u8; 6]) =>
        oer_esp32s31_hal::validation::hal_mac_set_addr(interface, address);
}

oer_probe_macros::probe! {
    pub fn open_libpp_interface_trace_hal_mac_set_bssid(interface: u32, address: &[u8; 6]) =>
        oer_esp32s31_hal::validation::hal_mac_set_bssid(interface, address);
}

oer_probe_macros::probe! {
    pub fn open_libpp_ap_tsf_trace_hal_disable_softap_tsf() {
        let mut owner = RadioRuntimeOwner::claim_for_validation();
        let mut hardware = owner.wifi_mac_hal();
        stop_access_point_tsf(&mut hardware);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset(selector: u32) {
        if selector == 0 {
            let mut owner = RadioRuntimeOwner::claim_for_validation();
            let mut hardware = owner.wifi_mac_hal();
            reset_and_start_access_point_tsf(&mut hardware);
        }
    }
}

oer_probe_macros::probe! {
    /// Read STA TSF into the caller's optional output words.
    ///
    /// # Safety
    /// Non-null outputs must be aligned, exclusively writable, non-overlapping words.
    pub unsafe fn open_rom_power_tsf_trace_hal_get_sta_tsf(low: *mut u32, high: *mut u32) {
        // SAFETY: the executable profile supplies either null or one aligned,
        // writable scratch word for each pointer, matching the ROM ABI.
        let low = unsafe { low.as_mut() };
        // SAFETY: same closed profile contract as `low`.
        let high = unsafe { high.as_mut() };
        oer_esp32s31_hal::validation::hal_get_sta_tsf(low, high);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_enable_agc(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_enabled(&mut shared_phy(registers), true);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_vht_support(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_vht_support(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_csidump_force_lltf_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_csi_dump_force_lltf(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_hemu_ru26_good_res(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_he_ru26_good_response(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_band_reg_set(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_frequency_band(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_fe_reg_init(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::initialize_front_end(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_fe_reg_update(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::update_front_end(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bbtx_outfilter(
        input_0: u32,
        input_1: u32,
        input_2: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_tx_output_filter(
            &mut shared_phy(registers), input_0, input_1, input_2,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdt_rst_enable(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_watchdog_reset_enabled(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdt_int_enable(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_watchdog_interrupt_enabled(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdt_timeout_clear(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::clear_watchdog_timeout(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_bb_wdt_get_status(registers: &mut RadioPhyRegisters) -> u32 =>
        oer_esp32s31_hal::phy::baseband::watchdog_status(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_lltf_mask_en(input_0: u32, input_1: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_lltf_mask(&mut shared_phy(registers), input_0, input_1);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ant_init(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_antenna(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdg_cfg(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_watchdog(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bt_filter_reg(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::configure_bt_filter(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_module_resetn(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::reset_module(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_en_hw_set_freq(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::set_hardware_control(&mut shared_phy(registers), true);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_dis_hw_set_freq(registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::frequency::set_hardware_control(&mut shared_phy(registers), false);
        ets_delay_us(2);
        // Keep the delay as a non-tail edge so the executor sees the named
        // harness symbol independently of linker tail-call relaxation.
        core::hint::black_box(registers);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_reg_init(
        _vendor_parameter_0: u32,
        _vendor_parameter_1: u32,
        parameter_override: u32,
        registers: &mut RadioPhyRegisters,
    ) =>
        oer_esp32s31_hal::phy::frequency::initialize_registers(&mut shared_phy(registers), parameter_override != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_iq_corr_enable(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::enable_iq_correction(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_noise_floor_auto_set(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_noise_floor_auto(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_read_hw_noisefloor(registers: &mut RadioPhyRegisters) -> u32 =>
        oer_esp32s31_hal::phy::baseband::read_hardware_noise_floor(&mut shared_phy(registers)) as u32;
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_tx_paon_set(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_tx_pa_on(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_wifi_agc_sat_gain(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_saturation_gain(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_enable_low_rate(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_low_rate_enabled(&mut shared_phy(registers), true);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_disable_low_rate(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_low_rate_enabled(&mut shared_phy(registers), false);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_is_low_rate_enabled(registers: &mut RadioPhyRegisters) -> u32 =>
        u32::from(oer_esp32s31_hal::phy::agc::low_rate_enabled(&mut shared_phy(registers)));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_dcmem_clr(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::clear_dc_memory(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_rx_11b_opt(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rx_11b_optimization(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_rfrx_sat_rst(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rf_rx_saturation(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_set_rxclk_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::pbus::configure_rx_clock(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_set_txclk_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_pbus_debugmode(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_i2c_txrate_init(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_i2c_tx_rate(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_nrx_freq_set(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::configure_nrx_frequency(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_cbw_chan_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::configure_channel_cbw(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_agc_reg_init(
        parameter_121: u32,
        parameter_120: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::agc::initialize_registers(
            &mut shared_phy(registers),
            parameter_121 as u8,
            parameter_120 as u8,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_set_rx_comp_new(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rx_compensation(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_bb_txpwr_track(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_tx_power_tracking(&mut shared_phy(registers), input & 1 != 0);
}

oer_probe_macros::probe! {
    /// Current vendor TX-gain restore boundary, compiled from production HAL/PAC.
    pub fn open_phy_calibration_trace_restore_tx_gain_compensation(
        _enabled: u32,
        registers: &mut RadioPhyRegisters,
    ) =>
        oer_esp32s31_hal::phy::baseband::restore_tx_gain_compensation(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_calibration_trace_force_digital_gain(
        enabled: u32,
        gain_0: u32,
        gain_1: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_forced_digital_gain(
            &mut shared_phy(registers),
            enabled & 1 != 0,
            gain_0 as i8,
            gain_1 as i8,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_calibration_trace_temperature_to_power(
        current: u32,
        reference: u32,
        bluetooth: u32,
    ) -> i32 {
        let class = if bluetooth == 0 {
            oer_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass::Wifi
        } else {
            oer_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154
        };
        i32::from(
            oer_esp32s31_phy::tracking::parameters::temperature_to_tracking_power(
                current as i16,
                reference as i16,
                class,
            ),
        )
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_calibration_trace_post_init_agc(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::update_post_initialization(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_reg_update_new(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::update_post_initialization(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_dc_mem_clr(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::clear_dc_memory(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_set_ftm_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_ftm_enabled_from_vendor_argument(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_stop_tx_tone_new(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::stop_tx_tone(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_close_fe_bb_clk(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::clock::close_frontend_baseband(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_config_hccfr(
        enabled: u32,
        value: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_hccfr_from_vendor_arguments(
            &mut shared_phy(registers), enabled, value,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_iccfr_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_iccfr_gate(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_force_iccfr(
        mode: u32,
        enabled: u32,
        value: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_forced_iccfr_from_vendor_arguments(
            &mut shared_phy(registers), mode, enabled, value,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_pwdet_always_en() =>
        oer_esp32s31_phy::tx::power_detector::phy_pwdet_always_en();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_pwdet_onetime_en() =>
        oer_esp32s31_phy::tx::power_detector::phy_pwdet_onetime_en();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_11p_set(
        enabled: u32,
        configuration: u32,
        output: &mut CanonicalDot11pState,
    ) {
        let initial = *output;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_dot11p_configuration(initial.enabled, initial.configuration);
        state.set_dot11p_configuration(enabled as u8, configuration as u8);
        let projected = state.dot11p_configuration();
        *output = CanonicalDot11pState {
            enabled: projected.enabled,
            configuration: projected.configuration,
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_current_level_set(value: u32, output: &mut CanonicalCurrentLevelState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_current_level(initial);
        state.set_current_level(value as u8);
        output.value = state.current_level();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_bt_power_track(value: u32, output: &mut CanonicalBtPowerTrackingState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_bt_power_tracking(initial);
        state.set_bt_power_tracking(value as u8);
        output.value = state.bt_power_tracking();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_ble_set_chan_base(value: u32, output: &mut CanonicalBleChannelBaseState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_ble_channel_base(initial);
        state.set_ble_channel_base(value as u8);
        output.value = state.ble_channel_base();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_init_param_set(
        value: u32,
        output: &mut CanonicalInitializationParameterState,
    ) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_initialization_parameter(u32::from(initial != 0));
        state.set_initialization_parameter(value);
        output.value = u8::from(state.initialization_parameter());
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_track_temp_debug(
        first: u32,
        second: u32,
        output: &mut CanonicalTemperatureTrackingState,
    ) {
        let initial = *output;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_temperature_tracking_debug(initial.first, initial.second);
        state.set_temperature_tracking_debug(first as u8, second as u8);
        let projected = state.temperature_tracking_debug();
        *output = CanonicalTemperatureTrackingState {
            first: projected.first,
            second: projected.second,
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_noise_check_loop() =>
        oer_esp32s31_phy::rx::signal_power::noise_check_loop();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_bbpll_en_usb() =>
        oer_esp32s31_phy::analog::rfpll::phy_bbpll_en_usb();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_freq_mem_backup() =>
        oer_esp32s31_phy::analog::frequency::phy_freq_mem_backup();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_freq_offset_set() =>
        oer_esp32s31_phy::analog::frequency::phy_freq_offset_set();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_get_i2c_data() =>
        oer_esp32s31_phy::analog::i2c::phy_get_i2c_data();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_archive_set_bb_wdg() =>
        oer_esp32s31_phy::calibration::baseband::set_bb_wdg();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_phy_get_rf_cal_version() -> u32 =>
        oer_esp32s31_phy::analog::rfpll::phy_get_rf_cal_version();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_phy_get_rfdata_num() -> u32 =>
        oer_esp32s31_phy::calibration::cold::phy_get_rfdata_num();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_get_bias_ref_code() -> u32 =>
        oer_esp32s31_phy::tx::calibration::get_bias_ref_code();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_phy_internal_delay() -> u32 =>
        oer_esp32s31_phy::calibration::cold::phy_internal_delay();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_enter_critical() =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_enter_critical();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_exit_critical() =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_exit_critical();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_get_dc_value(output: &mut [u16; 2], value: u32) =>
        oer_esp32s31_phy::calibration::estimator::get_dc_value(output, value);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_master_mem_cfg(configuration: &mut [u8; 6]) =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_master_mem_cfg(configuration);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_master_command_mem_cfg(configuration: &mut [u8; 8], mode: &mut u32) =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_master_command_mem_cfg(configuration, mode);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_tx_atten_comp(values: &mut [u8; 3]) =>
        oer_esp32s31_phy::tx::calibration::phy_tx_atten_comp(values);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ant_dft_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_antenna_diversity(&mut shared_phy(registers), input & 1 != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_btbb_wifi_bb_cfg2(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_bt_wifi_baseband(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_chan_dump_cfg(
        value: u32,
        enabled: u32,
        mode: u32,
        registers: &mut RadioPhyRegisters,
    ) =>
        oer_esp32s31_hal::phy::baseband::configure_channel_dump(&mut shared_phy(registers), value, enabled, mode);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_dac_rate_set(rate: u32, registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::baseband::configure_dac_rate(
            &mut shared_phy(registers),
            oer_esp32s31_hal::types::PhyAdcRate::from_vendor_rate(rate),
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_force_pwr_index(enabled: bool, index: u8, registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::memory::configure_forced_power_index(
            &mut shared_phy(registers),
            enabled,
            oer_esp32s31_hal::phy::memory::PhyForcedPowerIndex::new(u32::from(index))
                .expect("probe input must fit the reviewed forced-power index"),
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_force_rx_gain(enabled: u32, gain: u32, registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::agc::configure_forced_rx_gain_from_vendor_arguments(
            &mut shared_phy(registers), enabled, gain,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_rx11blr_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rx_11b_low_rate(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_enable_cca(registers: &mut RadioPhyRegisters) {
        let _ = registers;
        oer_esp32s31_hal::ieee80211::mac::validation_set_cca_enabled(true);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_disable_cca(registers: &mut RadioPhyRegisters) {
        let _ = registers;
        oer_esp32s31_hal::ieee80211::mac::validation_set_cca_enabled(false);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_sifs_reg_init(registers: &mut RadioPhyRegisters) {
        let _ = registers;
        oer_esp32s31_hal::ieee80211::mac::validation_initialize_sifs();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_abs_temp(input: u32) -> u32 =>
        oer_esp32s31_phy::calibration::math::absolute_temperature(input as i32);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_get_freq_mem_addr(
        base: u32,
        stride: u32,
        index: u32,
        offset: u32,
    ) -> u32 {
        u32::from(oer_esp32s31_phy::analog::frequency::phy_get_freq_mem_addr(
            base, stride, index, offset,
        ))
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_txpwr_track_slow(value: u32, output: &mut CanonicalSlowTxPowerTrackingState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_tx_power_tracking_slow(initial);
        state.set_tx_power_tracking_slow(value as u8);
        output.value = state.tx_power_tracking_slow();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_i2c_mem_write(
        address: u32,
        value: u32,
        mode: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::frequency::write_memory(
            &mut shared_phy(registers),
            (address & 0x07ff) as u16,
            value & 0x00ff_ffff,
            mode as u8,
        );
    }
}

oer_probe_macros::probe! {
    /// Execute the shipping command-memory transaction with explicit parameter bytes.
    /// The isolated image constructs its own owner; no private owner layout is part
    /// of the guest ABI. Encoding and all MMIO remain in the production HAL/PAC.
    pub fn open_phy_trace_command_memory(parameters: &[u8; 6]) {
        with_phy(|registers| {
            let inputs = oer_esp32s31_hal::phy::i2c::PhyI2cCommandMemoryInputs::new(
                parameters[0],
                parameters[1],
                parameters[2],
                parameters[3],
                parameters[4],
                parameters[5],
            );
            oer_esp32s31_hal::phy::i2c::configure_command_memory(&mut shared_phy(registers), inputs);
        })
    }
}

oer_probe_macros::probe! {
    /// Initialize captured mutable parameter memory during an explicit setup phase.
    /// This harness copy supplies scenario inputs; it performs no PHY computation.
    pub fn open_phy_trace_initialize_parameters(destination: &mut [u8; 400], source: &[u8; 400]) =>
        destination.copy_from_slice(source);
}

oer_probe_macros::probe! {
    /// Exercise the production software-frequency transition with an owned PHY
    /// partition. This also exposes the preserved ordinary delay-call boundary.
    pub fn open_phy_trace_owned_software_frequency_control() =>
        with_phy(|registers| open_phy_trace_dis_hw_set_freq(registers));
}
