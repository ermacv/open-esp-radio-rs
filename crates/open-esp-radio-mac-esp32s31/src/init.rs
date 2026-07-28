//! Finite cold-start of the ESP32-S31 Wi-Fi MAC receive path.
//!
//! The register sequence is recovered from `libpp.a[hal_mac.o]::hal_init`,
//! `mac_txrx_init`, `mac_rxbuf_init`, and `mac_last_rxbuf_init`. It deliberately
//! stops before publishing an RX descriptor base: ownership of the DMA ring
//! remains with [`crate::rx::publish_cold_ring`].

use open_esp_radio_pac_esp32s31::{mac::init as registers, Register32};

pub use crate::cold_antenna::MacColdAntennaHardware;
pub use crate::cold_crypto::MacColdCryptoHardware;
pub use crate::cold_enable::MacColdEnableHardware;
pub use crate::cold_hal_tail::{MacColdHalTailHardware, MacSlowClockCalibration};
pub use crate::cold_handshake::{MacColdHandshakeHardware, MacColdStartError, MacColdStartOutcome};
pub use crate::cold_last_rx_buffer::MacColdLastRxBufferHardware;
pub use crate::cold_rx_buffer::MacColdRxBufferHardware;
pub use crate::cold_rx_policy::MacColdRxPolicyHardware;
pub use crate::cold_txrx::{MacColdTxRxHardware, MacDelaySlot};
pub use crate::interface_address::MacInterfaceAddressHardware;
pub use crate::low_rate::MacLowRateHardware;
pub use crate::sniffer::MacSnifferHardware;
pub use crate::sta_link_policy::{configure_sta_link_receive_policy, StaLinkRxPolicyHardware};
use crate::{interface_address::program_cold_receive_addresses, registers::Mmio};

const MAC_COLD_RX_INTERRUPT_MASK: u32 = 0x19a8_79e0;

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
    fn mac_slow_clock_calibration(&mut self) -> u32;
}

#[inline]
fn modify<M: Mmio>(mmio: &mut M, register: Register32, mask: u32, value: u32) {
    let current = mmio.read32(register);
    mmio.write32(register, (current & !mask) | (value & mask));
}

/// Direct register portion of `hal_init_bf` and `hal_he_init`.
///
/// The operations below cover the common legacy TX/RX fields plus the receive
/// parser, multi-BSSID and baseband-hang configuration used by `hal_init`.
/// Trigger-based HE transmit and debug output remain outside this cold path.
fn initialize_he_receive<M: Mmio>(mmio: &mut M) {
    // `hal_init_bf`: establish the common PHY/MAC receive timing fields.
    modify(mmio, registers::R_4C78, 0x0020_0000, 0);
    modify(mmio, registers::R_4C78, 0x0080_0000, 0);
    modify(mmio, registers::R_4C78, 0x0008_0000, 0x0008_0000);
    modify(mmio, registers::R_4C78, 0x0000_ff00, 0x0000_7100);
    modify(mmio, registers::R_4C78, 0x0000_00fe, 0x0000_0020);
    modify(mmio, registers::R_4C78, 0x0007_0000, 0x0005_0000);
    modify(mmio, registers::R_4C78, 0x0200_0000, 0x0200_0000);
    modify(mmio, registers::R_4480, 0x8000_0000, 0x8000_0000);
    modify(mmio, registers::R_447C, 0x00ff_f000, 0x0080_1000);
    modify(mmio, registers::R_4DE4, 0xfff0_0000, 0x6900_0000);
    modify(mmio, registers::R_409C, 0x0000_000c, 0x0000_0008);

    // Receive-relevant direct body of `hal_he_init`.
    modify(mmio, registers::R_4C80, 0xc000_0000, 0);
    modify(mmio, registers::R_4110, 0x000c_0000, 0x0008_0000);
    modify(mmio, registers::R_4048, 0x0000_01f8, 0x0000_01e0);
    modify(mmio, registers::R_4C2C, 0x0000_1000, 0x0000_1000);
    // `hal_init_tb_tx`: keep both trigger-based transmit request modes off.
    modify(mmio, registers::R_4E04, 0x0000_c000, 0);
    // `hal_he_set_ersu(0)` and its `hal_he_set_ersu_ack_rate(0)` child.
    // Although ER-SU is not used by the legacy probe request, these are common
    // transmit defaults established by the parent before any queue can run.
    modify(mmio, registers::R_4C7C, 0x0000_0400, 0x0000_0400);
    mmio.write32(registers::R_4404, 0x8080_8080);
    modify(mmio, registers::R_4C80, 0x0000_0ff8, 0x0000_0be0);

    // HE scratch table cleared by the vendor parent.
    for index in 0..registers::HE_SCRATCH_COUNT {
        mmio.write32(
            registers::he_scratch(index).expect("bounded HE scratch index"),
            0,
        );
    }

    for register in registers::HE_PROTECTION {
        modify(mmio, register, 0xc000_0000, 0);
    }
    modify(mmio, registers::R_4C98, 0x0000_0004, 0x0000_0004);
    modify(mmio, registers::R_4CC0, 0x8000_0000, 0x8000_0000);
    modify(mmio, registers::R_4C88, 0x0000_0003, 0x0000_0003);
    modify(mmio, registers::R_42B8, 0xc000_0000, 0x4000_0000);
    modify(mmio, registers::R_4400, 0x0002_0000, 0x0002_0000);
    // `hal_set_tx_min_pwr(-11)`: the signed six-bit minimum-power field is
    // shared by legacy management TX and the later HE transmit paths.
    modify(mmio, registers::R_4400, 0x0000_03f0, 0x0000_0350);
    modify(mmio, registers::R_410C, 0x0000_0001, 0);
    modify(mmio, registers::R_4C7C, 0x0040_0000, 0);

    // `hal_he_clr_multi_bssid` followed by
    // `hal_he_set_co_hosted_bss(0, 0)`.
    modify(mmio, registers::R_4020, 0x0002_0100, 0);
    modify(mmio, registers::R_4020, 0x0001_fe00, 0x0001_fe00);
    modify(mmio, registers::R_4020, 0x0000_00ff, 0);
    modify(mmio, registers::R_4028, 0x00ff_0000, 0);
    for register in registers::HE_QUEUE_CONTROL {
        modify(mmio, register, 0x0000_0004, 0);
    }
}

/// Receive-relevant default COEX PTI setup at the tail of `hal_init`.
fn initialize_coex<M: Mmio>(mmio: &mut M) {
    // `hal_coex_pti_init`, `hal_coex_enable_default_pti(1)` and the cold
    // default returned by the OSI PTI query: RX-active/ACK zero, Wi-Fi one.
    modify(mmio, registers::R_4DDC, 0x0000_003f, 0x0000_0031);
    modify(mmio, registers::R_42FC, 0x0000_00ff, 0);
}

/// Establish the reset-state MAC register configuration and accept all RX
/// frame classes.
///
/// The modem clock fields come from the pinned S31 register descriptions and
/// reproduce `esp-radio`'s S31 `enable_wifi(true)` operation. The remaining
/// ordered MAC writes come from the complete pinned `libpp.a` bodies named in
/// this module's documentation.
///
/// This function owns no descriptor storage and enables neither the RX walker
/// nor MAC interrupts. The caller must publish its ring after this returns.
pub fn initialize_promiscuous_receive<
    M: Mmio
        + MacColdAntennaHardware
        + MacColdCryptoHardware
        + MacColdEnableHardware
        + MacColdHalTailHardware
        + MacColdHandshakeHardware
        + MacColdLastRxBufferHardware
        + MacColdRxBufferHardware
        + MacColdRxPolicyHardware
        + MacColdTxRxHardware
        + MacInterfaceAddressHardware
        + MacLowRateHardware
        + MacSnifferHardware,
    P: MacClockControl + MacDelayEntropy + MacSlowClockCalibrationSource,
>(
    platform: &mut P,
    mmio: &mut M,
    handshake_sample_limit: u32,
    station_address: [u8; 6],
    access_point_address: [u8; 6],
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

    let outcome = mmio.begin_cold_handshake(handshake_sample_limit)?;

    // Direct prefix, all three exact on-chip HE callback paths, then suffix.
    mmio.initialize_txrx_prefix();
    let delay_slot = MacDelaySlot::from_random(platform.mac_delay_random());
    mmio.initialize_txrx_callbacks(delay_slot);
    mmio.initialize_txrx_suffix();

    // Complete four-queue direct/leaf transaction from `hal_init`.
    mmio.initialize_cold_receive_policy();

    // `mac_rxbuf_init`, with descriptor publication left to the ring owner.
    mmio.initialize_rx_buffer_prefix();

    initialize_he_receive(mmio);

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
    // callback's u32 is reduced exactly as the complete RTC-update leaf does.
    let slow_clock_calibration =
        MacSlowClockCalibration::from_osi_value(platform.mac_slow_clock_calibration());
    mmio.initialize_hal_tail(MAC_COLD_RX_INTERRUPT_MASK, slow_clock_calibration);
    initialize_coex(mmio);

    // Complete `hal_enable_mac`: clear the four common disable gates, then
    // publish the interrupt mask. This does not route the peripheral interrupt
    // to a CPU; the platform interrupt owner still installs that route and ISR.
    mmio.enable_mac_interrupts(MAC_COLD_RX_INTERRUPT_MASK);

    // `wifi_set_rx_policy(0)` publishes both valid interface addresses after
    // `hal_init`; the address-valid bits are part of the S31 RX start gate even
    // when the sniffer subsequently disables address filtering.
    program_cold_receive_addresses(mmio, station_address, access_point_address);

    // Promiscuous mode clears the recovered class-reject bits and enables all
    // miscellaneous packet classes. A target oracle shows that the queue
    // policy words at 0x40fc/0x4100/0x4108 do not change when promiscuous mode
    // is enabled; they must retain the defaults installed by `mac_txrx_init`.
    mmio.write32(registers::CONTROL, 0);
    mmio.enable_promiscuous_sniffer();
    modify(mmio, registers::R_40F4, 0x0000_ff00, 0x0000_ff00);
    mmio.fence();

    Ok(outcome)
}
