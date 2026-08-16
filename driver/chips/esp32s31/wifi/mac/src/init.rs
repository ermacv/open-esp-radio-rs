//! Finite cold-start of the ESP32-S31 Wi-Fi MAC receive path.
//!
//! The register sequence is recovered from `libpp.a[hal_mac.o]::hal_init`,
//! `mac_txrx_init`, `mac_rxbuf_init`, and `mac_last_rxbuf_init`. It deliberately
//! stops before publishing an RX descriptor base: ownership of the DMA ring
//! remains with [`crate::rx::publish_cold_ring`].

pub use crate::cold_antenna::MacColdAntennaHardware;
pub use crate::cold_coex::{
    MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacColdCoexHardware, MacColdCoexPti,
};
pub use crate::cold_crypto::MacColdCryptoHardware;
pub use crate::cold_enable::MacColdEnableHardware;
pub use crate::cold_hal_tail::{MacColdHalTailHardware, MacSlowClockCalibration};
pub use crate::cold_handshake::{MacColdHandshakeHardware, MacColdStartError, MacColdStartOutcome};
pub use crate::cold_he::{
    MacColdHeHardware, MacTxPowerSource, query_tx_power_table, run_tx_power_diagnostic_queries,
};
pub use crate::cold_last_rx_buffer::MacColdLastRxBufferHardware;
pub use crate::cold_rx_buffer::MacColdRxBufferHardware;
pub use crate::cold_rx_policy::MacColdRxPolicyHardware;
pub use crate::cold_txrx::{MacColdTxRxHardware, MacDelaySlot};
pub use crate::interface_address::MacInterfaceAddressHardware;
use crate::interface_address::program_cold_receive_addresses;
pub use crate::low_rate::MacLowRateHardware;
pub use crate::sniffer::MacSnifferHardware;
pub use crate::sta_link_policy::{
    StaLinkRxPolicyHardware, StaNoiseFloorHardware, configure_sta_link_receive_policy,
};
pub use open_esp_radio_esp32s31_hal::types::{MacInterruptMask, MacTxPowerPair, MacTxPowerTable};

/// Complete event mask published by the recovered cold receive initializer.
///
/// Applications activate the disjoint ISR register capability only after
/// installing their final handler storage, but the bit policy itself remains
/// part of the MAC lifecycle recovered from `libpp.a[hal_mac.o]::hal_init`.
pub const MAC_COLD_RX_INTERRUPT_MASK: MacInterruptMask = MacInterruptMask::COLD_RX;

/// Official chip-platform capability required before touching MAC-local MMIO.
///
/// The MAC crate owns the lifecycle order while the integration implements
/// these operations with its official chip PAC singleton tokens.
pub trait MacClockControl {
    fn enable_wifi_mac_clocks(&mut self);
    fn enable_coexistence_clock(&mut self);
    fn configure_modem_source_clocks(&mut self);
    fn set_wifi_mac_reset(&mut self, asserted: bool);
}

/// Platform entropy used by the on-chip branch of `hal_he_set_mac_delay`.
///
/// The vendor OS adapter obtains this from its `_random` callback. Keeping it
/// as a narrow trait prevents the MAC crate from borrowing a chip RNG
/// peripheral or depending on the vendor C ABI.
pub trait MacDelayEntropy {
    fn mac_delay_random(&mut self) -> u32;
}

/// Platform slow-clock calibration used by `hal_timer_update_by_rtc`.
///
/// This is the Rust ownership boundary corresponding to the vendor
/// `_slowclk_cal_get` callback. It keeps the open MAC independent of the
/// vendor function table and of any future platform clock peripheral.
pub trait MacSlowClockCalibrationSource {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration;
}

/// Inputs for the role-neutral Wi-Fi MAC cold transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdStartConfig {
    pub handshake_sample_limit: u32,
    pub station_address: [u8; 6],
    pub access_point_address: [u8; 6],
}

/// Establish the role-neutral reset-state MAC register configuration.
///
/// The modem clock fields come from the pinned S31 register descriptions and
/// reproduce `esp-radio`'s S31 `enable_wifi(true)` operation. The remaining
/// ordered MAC writes come from the complete pinned `libpp.a` bodies named in
/// this module's documentation.
///
/// This function owns no descriptor storage, enables neither the RX walker nor
/// a CPU interrupt route, and does not select a station/AP/monitor receive
/// policy. A role builder must apply that policy before publishing its ring.
pub fn initialize_wifi_mac<
    M: MacColdAntennaHardware
        + MacColdCoexHardware
        + MacColdCryptoHardware
        + MacColdEnableHardware
        + MacColdHalTailHardware
        + MacColdHeHardware
        + MacColdHandshakeHardware
        + MacColdLastRxBufferHardware
        + MacColdRxBufferHardware
        + MacColdRxPolicyHardware
        + MacColdTxRxHardware
        + MacInterfaceAddressHardware
        + MacLowRateHardware,
    P: MacClockControl
        + MacCoexPtiSource
        + MacDelayEntropy
        + MacSlowClockCalibrationSource
        + MacTxPowerSource,
>(
    platform: &mut P,
    mmio: &mut M,
    config: MacColdStartConfig,
) -> Result<MacColdStartOutcome, MacColdStartError> {
    // Match the vendor lifecycle's `wifi_clock_enable()` followed by
    // `wifi_reset_mac()`. The earlier PHY-owner reset can occur while the MAC
    // functional clock is still gated, so it is not sufficient to establish
    // a cold MAC register state after a warm SoC reset. Reset only WIFIMAC
    // here; the calibrated Wi-Fi baseband remains live.
    platform.enable_wifi_mac_clocks();
    platform.enable_coexistence_clock();
    platform.configure_modem_source_clocks();
    platform.set_wifi_mac_reset(true);
    platform.set_wifi_mac_reset(false);

    let outcome = mmio.begin_cold_handshake(config.handshake_sample_limit)?;

    // Direct prefix, all three exact on-chip HE callback paths, then suffix.
    mmio.initialize_txrx_prefix();
    let delay_slot = MacDelaySlot::from_random(platform.mac_delay_random());
    mmio.initialize_txrx_callbacks(delay_slot);
    mmio.initialize_txrx_suffix();

    // Complete four-queue direct/leaf transaction from `hal_init`.
    mmio.initialize_cold_receive_policy();

    // `mac_rxbuf_init`, with descriptor publication left to the ring owner.
    mmio.initialize_rx_buffer_prefix();

    mmio.initialize_he_prefix();
    // Complete hal_init_tx_pwr queries all 43 calibrated PHY pairs before its
    // first MAC table write. The fixed Rust value owns that snapshot; no MAC
    // code retains a pointer into PHY or vendor global state.
    let tx_power = query_tx_power_table(platform);
    mmio.initialize_tx_power(&tx_power);
    // Complete dbg_read_tx_power is not semantically needed for logging, but
    // its 25 discarded ROM queries have 50 hardware-visible PHY RMW edges.
    run_tx_power_diagnostic_queries(platform);
    mmio.initialize_he_suffix();

    // Complete `mac_last_rxbuf_init`, including its three separate enable RMWs.
    mmio.initialize_last_rx_buffer_table();

    // `phy_disable_low_rate`, invoked by `hal_mac_disable_low_rate`.
    mmio.disable_phy_low_rate();

    // `hal_crypto_init`: even unencrypted promiscuous frames traverse the
    // common RX crypto bypass block.
    mmio.initialize_crypto_bypass();

    // Complete `hal_attenna_init`: 34 RMW edges, including both reverse
    // traversals of the eight queue/vector-bank words.
    mmio.initialize_mac_antenna();

    // Complete direct hal_init tail before its first COEX operation. The OSI
    // callback result retains whether the platform actually supplied a
    // calibration. The hardware mapping remains explicit in the HAL adapter.
    let slow_clock_calibration = platform.mac_slow_clock_calibration();
    mmio.initialize_hal_tail(MAC_COLD_RX_INTERRUPT_MASK, slow_clock_calibration);

    // Complete seventeen-edge COEX/PTI tail. Query values in the blob's exact
    // callback order before handing the finite program to its PAC owner.
    let coex_pti = MacColdCoexPti::query(platform);
    mmio.initialize_cold_coex(coex_pti);

    // Complete `hal_enable_mac`: clear the four common disable gates, then
    // publish the interrupt mask. This does not route the peripheral interrupt
    // to a CPU; the platform interrupt owner still installs that route and ISR.
    mmio.enable_mac_interrupts(MAC_COLD_RX_INTERRUPT_MASK);

    // `wifi_set_rx_policy(0)` publishes both valid interface addresses after
    // `hal_init`; the address-valid bits are part of the S31 RX start gate even
    // when the sniffer subsequently disables address filtering.
    program_cold_receive_addresses(mmio, config.station_address, config.access_point_address);

    Ok(outcome)
}

/// Select the standalone/scan promiscuous receive policy after common init.
///
/// The ownership-bound transaction keeps the HIL-qualified open policy,
/// complete vendor sniffer leaf, misc-class update and device fence together.
/// It deliberately leaves queue 0..2 defaults intact.
pub fn activate_promiscuous_receive<M: MacSnifferHardware>(mmio: &mut M) {
    mmio.configure_open_promiscuous_receive();
}
