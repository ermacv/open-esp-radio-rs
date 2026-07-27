//! Finite cold-start of the ESP32-S31 Wi-Fi MAC receive path.
//!
//! The register sequence is recovered from `libpp.a[hal_mac.o]::hal_init`,
//! `mac_txrx_init`, `mac_rxbuf_init`, and `mac_last_rxbuf_init`. It deliberately
//! stops before publishing an RX descriptor base: ownership of the DMA ring
//! remains with [`crate::rx::publish_cold_ring`].

use open_esp_radio_pac_esp32s31::{
    mac::{self, init as registers},
    power::{hp_sys_clkrst, modem_lpcon, modem_syscon},
    Register32,
};

use crate::registers::Mmio;

const MAC_INIT_REQUEST: u32 = 1 << 1;
const MAC_INIT_READY: u32 = 1;
const RX_SNIFFER_REJECT_MASK: u32 = 0x0000_038f;
const RX_SNIFFER_ENABLE: u32 = 0x0002_0000;
const WIFI_CLOCKS: u32 = modem_syscon::clk_conf1::CLK_WIFIBB_22M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_40M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_44M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_80M_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_40X_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_80X_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_40X1_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_80X1_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIBB_160X1_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFIMAC_EN.mask()
    | modem_syscon::clk_conf1::CLK_WIFI_APB_EN.mask();
const COEX_CLOCK: u32 = modem_lpcon::clk_conf::CLK_COEX_EN.mask();
const HP_MODEM_CLOCK_CONFIGURATION: u32 = hp_sys_clkrst::modem_conf::MODEM_APB_CLK_EN.mask()
    | hp_sys_clkrst::modem_conf::MODEM_CLK_EN.mask()
    | hp_sys_clkrst::modem_conf::MODEM_CLK_SOURCE_SEL.mask()
    | hp_sys_clkrst::modem_conf::MODEM_PLL_CLK_EN.mask()
    | hp_sys_clkrst::modem_conf::MODEM_XTAL_CLK_EN.mask();
const MAC_COLD_RX_INTERRUPT_MASK: u32 = 0x19a8_79e0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacColdStartError {
    HandshakeTimedOut { samples: u32, observed: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdStartOutcome {
    pub handshake_samples: u32,
    pub handshake_value: u32,
}

#[inline]
fn modify<M: Mmio>(mmio: &mut M, register: Register32, mask: u32, value: u32) {
    let current = mmio.read32(register);
    mmio.write32(register, (current & !mask) | (value & mask));
}

fn program_interface_address<M: Mmio>(mmio: &mut M, interface: usize, address: [u8; 6]) {
    mmio.write32(
        registers::INTERFACE_ADDRESS_LOW[interface],
        u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
    );
    mmio.write32(
        registers::INTERFACE_ADDRESS_HIGH[interface],
        u32::from(address[4]) | (u32::from(address[5]) << 8) | (1 << 16),
    );
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

/// Direct bodies of `hal_attenna_init` and the receive-relevant default COEX
/// PTI setup at the tail of `hal_init`.
fn initialize_antenna_and_coex<M: Mmio>(mmio: &mut M) {
    for index in 0..registers::ANTENNA_CONTROL_COUNT {
        modify(
            mmio,
            registers::antenna_control(index).expect("bounded antenna index"),
            0x0000_003c,
            0x0000_0020,
        );
    }
    modify(mmio, registers::R_42B0, 0x0000_0004, 0);
    modify(mmio, registers::R_42B0, 0x0000_0020, 0x0000_0020);

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
pub fn initialize_promiscuous_receive<M: Mmio>(
    mmio: &mut M,
    handshake_sample_limit: u32,
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> Result<MacColdStartOutcome, MacColdStartError> {
    // The PHY owner has already pulsed and released both Wi-Fi reset lines.
    // Open the remaining Wi-Fi-MAC and coexistence gates without resetting the
    // freshly calibrated baseband.
    modify(mmio, modem_syscon::CLK_CONF1, WIFI_CLOCKS, WIFI_CLOCKS);
    modify(mmio, modem_lpcon::CLK_CONF, COEX_CLOCK, COEX_CLOCK);
    mmio.write32(hp_sys_clkrst::MODEM_CONF, HP_MODEM_CLOCK_CONFIGURATION);

    modify(
        mmio,
        registers::HANDSHAKE,
        MAC_INIT_REQUEST,
        MAC_INIT_REQUEST,
    );
    let mut handshake_samples = 0;
    let handshake_value = loop {
        let value = mmio.read32(registers::HANDSHAKE);
        if value & MAC_INIT_READY != 0 {
            break value;
        }
        handshake_samples += 1;
        if handshake_samples >= handshake_sample_limit {
            return Err(MacColdStartError::HandshakeTimedOut {
                samples: handshake_samples,
                observed: value,
            });
        }
    };

    mmio.write32(mac::INT_ENABLE, 0);
    mmio.write32(mac::INT_CLEAR, u32::MAX);

    // `mac_txrx_init`
    modify(mmio, registers::R_4C8C, 0x9080_b200, 0x9080_b200);
    modify(mmio, registers::R_4C98, 1 << 3, 0);
    for register in registers::RX_QUEUE_DEFAULT {
        modify(mmio, register, 0xffff_0000, 0);
    }
    for register in &registers::RX_QUEUE_DEFAULT[..2] {
        modify(mmio, *register, 0x0500_0000, 0x0500_0000);
    }
    modify(mmio, registers::R_4114, 0x11, 0x11);
    modify(mmio, registers::R_4118, 0x8ff0_0000, 0x81b0_0000);
    modify(mmio, registers::R_4CA0, 0x3, 0x3);
    modify(mmio, registers::R_4C1C, 0xc000_0000, 0xc000_0000);
    for register in [registers::R_4C20, registers::R_4C24] {
        modify(mmio, register, 0x0000_0fff, 0x0000_00f0);
    }
    modify(mmio, registers::R_4CA8, 0x0000_00f0, 0x0000_0040);
    modify(mmio, registers::R_4C60, 0x7fff_0000, 0x7fff_0000);
    // The following instruction in `mac_txrx_init` sets the remaining high
    // bit in a second read/modify/write. RX happened to operate without it,
    // but the complete common TX/RX gate is all ones in bits 16..=31.
    modify(mmio, registers::R_4C60, 0x8000_0000, 0x8000_0000);
    modify(mmio, registers::R_4308, 0x2, 0x2);
    modify(mmio, mac::RX_CONTROL, 1 << 31, 0);

    // Reset the four hardware RX queue policy words. The first three have
    // address/BSSID policy registers; queue three is intentionally policy-only.
    for queue in 0..registers::RX_FILTER.len() {
        let policy = registers::RX_FILTER[queue];
        modify(mmio, policy, 0x0000_26c5, 0x0000_0285);
        if queue < 3 {
            modify(mmio, policy, 0x0000_0450, 0);
            modify(
                mmio,
                registers::BSSID_HIGH[queue],
                0xc000_0000,
                if queue == 1 { 0x4000_0000 } else { 0 },
            );
            modify(
                mmio,
                registers::INTERFACE_ADDRESS_HIGH[queue],
                0x0000_ffff,
                0,
            );
        }
    }

    // `mac_rxbuf_init`, with descriptor publication left to the ring owner.
    modify(mmio, registers::R_4C68, 0x000f_ffff, 0x000f_ffff);
    modify(mmio, registers::R_4C6C, 0x000f_ffff, 0x0000_0004);
    modify(mmio, mac::RX_LAST_DESCRIPTOR_HIGH, 0xfff0_0000, 0x2f00_0000);
    modify(mmio, registers::R_407C, 0x0000_00ff, 0);

    initialize_he_receive(mmio);

    // `mac_last_rxbuf_init`
    for (register, value) in registers::LAST_RX_BUFFER.into_iter().zip([
        0x0002_3006,
        0x0000_0608,
        0x0000_ffff,
        0x0002_3006,
        0x0000_0808,
        0x0000_ffff,
        0x0002_3006,
        0x0000_8e88,
        0x0000_ffff,
        0x0002_301c,
        0x4400_4300,
        0xffff_ffff,
        0x0002_301c,
        0x4300_4400,
        0xffff_ffff,
        0x0002_3011,
        0x0000_0001,
        0x0000_00ff,
    ]) {
        mmio.write32(register, value);
    }
    modify(mmio, registers::R_4120, 0x0000_3f7e, 0x0000_3f7e);
    modify(mmio, registers::R_4098, 0x0800_0000, 0x0800_0000);

    // No-power-save timing defaults used by `mac_txrx_init`.
    modify(mmio, registers::R_4C58, 0x001f_fc00, 0x000e_e000);
    modify(mmio, registers::R_4C58, 0x0000_03ff, 0x0000_00f0);
    modify(mmio, registers::R_4C58, 0x7fe0_0000, 0x0bc0_0000);
    modify(mmio, registers::R_4C54, 0x7fe0_0000, 0x1d40_0000);
    modify(mmio, registers::R_4C54, 0x001f_fc00, 0x0009_d800);
    mmio.write32(registers::R_444C, 0x0009_0a0b);
    mmio.write32(registers::R_4458, 0x0009_0a0b);
    mmio.write32(registers::R_4450, 0x0005_0100);
    mmio.write32(registers::R_445C, 0x0005_0100);
    modify(mmio, registers::R_4C1C, 0x0000_0fff, 0x0000_000f);

    // `phy_disable_low_rate`, invoked by `hal_mac_disable_low_rate`.
    modify(mmio, registers::R_8060, 0x0000_0c00, 0);
    modify(mmio, registers::R_807C, 0x0000_0800, 0);

    // `hal_crypto_init`: even unencrypted promiscuous frames traverse the
    // common RX crypto bypass block.
    for (register, value) in
        registers::CRYPTO_BYPASS
            .into_iter()
            .zip([0x0003_0000, 0x0003_0000, 0, 0, 0])
    {
        mmio.write32(register, value);
    }

    initialize_antenna_and_coex(mmio);

    // `hal_enable_mac` starts the shared MAC timebase by clearing the four
    // disable bits before it publishes the interrupt mask. The register can
    // already read as zero after reset, but the ordered write is still the
    // hardware start edge consumed by the EDCA scheduler.
    modify(mmio, registers::R_4C00, 0x0000_00f0, 0);

    // Common receive configuration at the tail of `hal_init`. This exact
    // vendor cold-start mask is also a hardware RX gate on S31. Publishing it
    // does not route the peripheral interrupt to a CPU; the platform interrupt
    // owner remains responsible for installing that route and ISR.
    mmio.write32(mac::INT_ENABLE, MAC_COLD_RX_INTERRUPT_MASK);
    modify(mmio, registers::R_4098, 0x0000_ffff, 0x0000_0101);
    modify(mmio, mac::RX_CONTROL, 0x0800_0000, 0x0800_0000);

    // `wifi_set_rx_policy(0)` publishes both valid interface addresses after
    // `hal_init`; the address-valid bits are part of the S31 RX start gate even
    // when the sniffer subsequently disables address filtering.
    program_interface_address(mmio, 0, station_address);
    program_interface_address(mmio, 1, access_point_address);

    // Promiscuous mode clears the recovered class-reject bits and enables all
    // miscellaneous packet classes. A target oracle shows that the queue
    // policy words at 0x40fc/0x4100/0x4108 do not change when promiscuous mode
    // is enabled; they must retain the defaults installed by `mac_txrx_init`.
    mmio.write32(registers::CONTROL, 0);
    modify(
        mmio,
        registers::RX_SNIFFER_CONTROL,
        RX_SNIFFER_ENABLE | RX_SNIFFER_REJECT_MASK,
        RX_SNIFFER_ENABLE,
    );
    modify(mmio, registers::R_40F4, 0x0000_ff00, 0x0000_ff00);
    mmio.fence();

    Ok(MacColdStartOutcome {
        handshake_samples,
        handshake_value,
    })
}
