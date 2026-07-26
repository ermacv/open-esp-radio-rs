//! Finite cold-start of the ESP32-S31 Wi-Fi MAC receive path.
//!
//! The register sequence is recovered from `libpp.a[hal_mac.o]::hal_init`,
//! `mac_txrx_init`, `mac_rxbuf_init`, and `mac_last_rxbuf_init`. It deliberately
//! stops before publishing an RX descriptor base: ownership of the DMA ring
//! remains with [`crate::rx::publish_cold_ring`].

use crate::registers::Mmio;

const MAC_INIT_HANDSHAKE: u32 = 0x2010_4de0;
const MODEM_SYSCON_CLK_CONF1: u32 = 0x2010_9c14;
const MODEM_LPCON_CLK_CONF: u32 = 0x2010_f018;
const HP_MODEM_CONF: u32 = 0x2058_71e0;
const MAC_INTERRUPT_ENABLE: u32 = 0x2010_4c40;
const MAC_INTERRUPT_CLEAR: u32 = 0x2010_4c4c;
const MAC_CONTROL: u32 = 0x2010_4cac;
const RX_FILTER_BASE: u32 = 0x2010_40d8;
const RX_SNIFFER_CONTROL: u32 = 0x2010_40e4;

const MAC_INIT_REQUEST: u32 = 1 << 1;
const MAC_INIT_READY: u32 = 1;
const RX_SNIFFER_REJECT_MASK: u32 = 0x0000_038f;
const RX_SNIFFER_ENABLE: u32 = 0x0002_0000;
const WIFI_CLOCKS: u32 = 0x0000_07ff;
const COEX_CLOCK: u32 = 1 << 1;
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
fn modify<M: Mmio>(mmio: &M, address: u32, mask: u32, value: u32) {
    let current = mmio.read32(address);
    mmio.write32(address, (current & !mask) | (value & mask));
}

fn program_interface_address<M: Mmio>(mmio: &M, interface: u32, address: [u8; 6]) {
    let stride = interface * 8;
    mmio.write32(
        0x2010_405c + stride,
        u32::from_le_bytes([address[0], address[1], address[2], address[3]]),
    );
    mmio.write32(
        0x2010_4060 + stride,
        u32::from(address[4]) | (u32::from(address[5]) << 8) | (1 << 16),
    );
}

/// Direct register portion of `hal_init_bf` and `hal_he_init`.
///
/// The omitted vendor children only configure transmit power, trigger-based
/// transmit and debug output. The operations below are the complete receive
/// parser, multi-BSSID and baseband-hang configuration used by `hal_init`.
fn initialize_he_receive<M: Mmio>(mmio: &M) {
    // `hal_init_bf`: establish the common PHY/MAC receive timing fields.
    modify(mmio, 0x2010_4c78, 0x0020_0000, 0);
    modify(mmio, 0x2010_4c78, 0x0080_0000, 0);
    modify(mmio, 0x2010_4c78, 0x0008_0000, 0x0008_0000);
    modify(mmio, 0x2010_4c78, 0x0000_ff00, 0x0000_7100);
    modify(mmio, 0x2010_4c78, 0x0000_00fe, 0x0000_0020);
    modify(mmio, 0x2010_4c78, 0x0007_0000, 0x0005_0000);
    modify(mmio, 0x2010_4c78, 0x0200_0000, 0x0200_0000);
    modify(mmio, 0x2010_4480, 0x8000_0000, 0x8000_0000);
    modify(mmio, 0x2010_447c, 0x00ff_f000, 0x0080_1000);
    modify(mmio, 0x2010_4de4, 0xfff0_0000, 0x6900_0000);
    modify(mmio, 0x2010_409c, 0x0000_000c, 0x0000_0008);

    // Receive-relevant direct body of `hal_he_init`.
    modify(mmio, 0x2010_4c80, 0xc000_0000, 0);
    modify(mmio, 0x2010_4110, 0x000c_0000, 0x0008_0000);
    modify(mmio, 0x2010_4048, 0x0000_01f8, 0x0000_01e0);
    modify(mmio, 0x2010_4c2c, 0x0000_1000, 0x0000_1000);
    modify(mmio, 0x2010_4c80, 0x0000_0ff8, 0x0000_0be0);

    // HE scratch table cleared by the vendor parent.
    let mut address = 0x2010_55f0;
    while address != 0x2010_57d0 {
        mmio.write32(address, 0);
        address += 4;
    }

    for address in [0x2010_4d64, 0x2010_4d54, 0x2010_4d44, 0x2010_4d34] {
        modify(mmio, address, 0xc000_0000, 0);
    }
    modify(mmio, 0x2010_4c98, 0x0000_0004, 0x0000_0004);
    modify(mmio, 0x2010_4cc0, 0x8000_0000, 0x8000_0000);
    modify(mmio, 0x2010_4c88, 0x0000_0003, 0x0000_0003);
    modify(mmio, 0x2010_42b8, 0xc000_0000, 0x4000_0000);
    modify(mmio, 0x2010_4400, 0x0002_0000, 0x0002_0000);
    modify(mmio, 0x2010_410c, 0x0000_0001, 0);
    modify(mmio, 0x2010_4c7c, 0x0040_0000, 0);

    // `hal_he_clr_multi_bssid` followed by
    // `hal_he_set_co_hosted_bss(0, 0)`.
    modify(mmio, 0x2010_4020, 0x0002_0100, 0);
    modify(mmio, 0x2010_4020, 0x0001_fe00, 0x0001_fe00);
    modify(mmio, 0x2010_4020, 0x0000_00ff, 0);
    modify(mmio, 0x2010_4028, 0x00ff_0000, 0);
    for address in [
        0x2010_4d68,
        0x2010_4d58,
        0x2010_4d48,
        0x2010_4d38,
        0x2010_4d28,
        0x2010_4d18,
        0x2010_4d08,
        0x2010_4cf8,
    ] {
        modify(mmio, address, 0x0000_0004, 0);
    }
}

/// Direct bodies of `hal_attenna_init` and the receive-relevant default COEX
/// PTI setup at the tail of `hal_init`.
fn initialize_antenna_and_coex<M: Mmio>(mmio: &M) {
    let mut address = 0x2010_5510;
    while address != 0x2010_5130 {
        modify(mmio, address, 0x0000_003c, 0x0000_0020);
        address -= 0x7c;
    }
    modify(mmio, 0x2010_42b0, 0x0000_0004, 0);
    modify(mmio, 0x2010_42b0, 0x0000_0020, 0x0000_0020);

    // `hal_coex_pti_init`, `hal_coex_enable_default_pti(1)` and the cold
    // default returned by the OSI PTI query: RX-active/ACK zero, Wi-Fi one.
    modify(mmio, 0x2010_4ddc, 0x0000_003f, 0x0000_0031);
    modify(mmio, 0x2010_42fc, 0x0000_00ff, 0);
}

/// Establish the reset-state MAC register configuration and accept all RX
/// frame classes.
///
/// This function owns no descriptor storage and enables neither the RX walker
/// nor MAC interrupts. The caller must publish its ring after this returns.
pub fn initialize_promiscuous_receive<M: Mmio>(
    mmio: &M,
    handshake_sample_limit: u32,
    station_address: [u8; 6],
    access_point_address: [u8; 6],
) -> Result<MacColdStartOutcome, MacColdStartError> {
    // The PHY owner has already pulsed and released both Wi-Fi reset lines.
    // Open the remaining Wi-Fi-MAC and coexistence gates without resetting the
    // freshly calibrated baseband.
    modify(mmio, MODEM_SYSCON_CLK_CONF1, WIFI_CLOCKS, WIFI_CLOCKS);
    modify(mmio, MODEM_LPCON_CLK_CONF, COEX_CLOCK, COEX_CLOCK);
    mmio.write32(HP_MODEM_CONF, 0x3d);

    modify(mmio, MAC_INIT_HANDSHAKE, MAC_INIT_REQUEST, MAC_INIT_REQUEST);
    let mut handshake_samples = 0;
    let handshake_value = loop {
        let value = mmio.read32(MAC_INIT_HANDSHAKE);
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

    mmio.write32(MAC_INTERRUPT_ENABLE, 0);
    mmio.write32(MAC_INTERRUPT_CLEAR, u32::MAX);

    // `mac_txrx_init`
    modify(mmio, 0x2010_4c8c, 0x9080_b200, 0x9080_b200);
    modify(mmio, 0x2010_4c98, 1 << 3, 0);
    for address in [0x2010_40fc, 0x2010_4100, 0x2010_4104, 0x2010_4108] {
        modify(mmio, address, 0xffff_0000, 0);
    }
    for address in [0x2010_40fc, 0x2010_4100] {
        modify(mmio, address, 0x0500_0000, 0x0500_0000);
    }
    modify(mmio, 0x2010_4114, 0x11, 0x11);
    modify(mmio, 0x2010_4118, 0x8ff0_0000, 0x81b0_0000);
    modify(mmio, 0x2010_4ca0, 0x3, 0x3);
    modify(mmio, 0x2010_4c1c, 0xc000_0000, 0xc000_0000);
    for address in [0x2010_4c20, 0x2010_4c24] {
        modify(mmio, address, 0x0000_0fff, 0x0000_00f0);
    }
    modify(mmio, 0x2010_4ca8, 0x0000_00f0, 0x0000_0040);
    modify(mmio, 0x2010_4c60, 0x7fff_0000, 0x7fff_0000);
    modify(mmio, 0x2010_4308, 0x2, 0x2);
    modify(mmio, 0x2010_4080, 1 << 31, 0);

    // Reset the four hardware RX queue policy words. The first three have
    // address/BSSID policy registers; queue three is intentionally policy-only.
    for queue in 0..4_u32 {
        let policy = RX_FILTER_BASE + queue * 4;
        modify(mmio, policy, 0x0000_26c5, 0x0000_0285);
        if queue < 3 {
            modify(mmio, policy, 0x0000_0450, 0);
            let bssid_high = 0x2010_4004 + queue * 8;
            modify(
                mmio,
                bssid_high,
                0xc000_0000,
                if queue == 1 { 0x4000_0000 } else { 0 },
            );
            modify(mmio, 0x2010_4060 + queue * 8, 0x0000_ffff, 0);
        }
    }

    // `mac_rxbuf_init`, with descriptor publication left to the ring owner.
    modify(mmio, 0x2010_4c68, 0x000f_ffff, 0x000f_ffff);
    modify(mmio, 0x2010_4c6c, 0x000f_ffff, 0x0000_0004);
    modify(mmio, 0x2010_4c70, 0xfff0_0000, 0x2f00_0000);
    modify(mmio, 0x2010_407c, 0x0000_00ff, 0);

    initialize_he_receive(mmio);

    // `mac_last_rxbuf_init`
    for (address, value) in [
        (0x2010_4124, 0x0002_3006),
        (0x2010_4140, 0x0000_0608),
        (0x2010_415c, 0x0000_ffff),
        (0x2010_4128, 0x0002_3006),
        (0x2010_4144, 0x0000_0808),
        (0x2010_4160, 0x0000_ffff),
        (0x2010_412c, 0x0002_3006),
        (0x2010_4148, 0x0000_8e88),
        (0x2010_4164, 0x0000_ffff),
        (0x2010_4130, 0x0002_301c),
        (0x2010_414c, 0x4400_4300),
        (0x2010_4168, 0xffff_ffff),
        (0x2010_4134, 0x0002_301c),
        (0x2010_4150, 0x4300_4400),
        (0x2010_416c, 0xffff_ffff),
        (0x2010_4138, 0x0002_3011),
        (0x2010_4154, 0x0000_0001),
        (0x2010_4170, 0x0000_00ff),
    ] {
        mmio.write32(address, value);
    }
    modify(mmio, 0x2010_4120, 0x0000_3f7e, 0x0000_3f7e);
    modify(mmio, 0x2010_4098, 0x0800_0000, 0x0800_0000);

    // No-power-save timing defaults used by `mac_txrx_init`.
    modify(mmio, 0x2010_4c58, 0x001f_fc00, 0x000e_e000);
    modify(mmio, 0x2010_4c58, 0x0000_03ff, 0x0000_00f0);
    modify(mmio, 0x2010_4c58, 0x7fe0_0000, 0x0bc0_0000);
    modify(mmio, 0x2010_4c54, 0x7fe0_0000, 0x1d40_0000);
    modify(mmio, 0x2010_4c54, 0x001f_fc00, 0x0009_d800);
    mmio.write32(0x2010_444c, 0x0009_0a0b);
    mmio.write32(0x2010_4458, 0x0009_0a0b);
    mmio.write32(0x2010_4450, 0x0005_0100);
    mmio.write32(0x2010_445c, 0x0005_0100);
    modify(mmio, 0x2010_4c1c, 0x0000_0fff, 0x0000_000f);

    // `phy_disable_low_rate`, invoked by `hal_mac_disable_low_rate`.
    modify(mmio, 0x2010_8060, 0x0000_0c00, 0);
    modify(mmio, 0x2010_807c, 0x0000_0800, 0);

    // `hal_crypto_init`: even unencrypted promiscuous frames traverse the
    // common RX crypto bypass block.
    mmio.write32(0x2010_4800, 0x0003_0000);
    mmio.write32(0x2010_4804, 0x0003_0000);
    mmio.write32(0x2010_4808, 0);
    mmio.write32(0x2010_480c, 0);
    mmio.write32(0x2010_4810, 0);

    initialize_antenna_and_coex(mmio);

    // Common receive configuration at the tail of `hal_init`. This exact
    // vendor cold-start mask is also a hardware RX gate on S31. Publishing it
    // does not route the peripheral interrupt to a CPU; the platform interrupt
    // owner remains responsible for installing that route and ISR.
    mmio.write32(MAC_INTERRUPT_ENABLE, MAC_COLD_RX_INTERRUPT_MASK);
    modify(mmio, 0x2010_4098, 0x0000_ffff, 0x0000_0101);
    modify(mmio, 0x2010_4080, 0x0800_0000, 0x0800_0000);

    // `wifi_set_rx_policy(0)` publishes both valid interface addresses after
    // `hal_init`; the address-valid bits are part of the S31 RX start gate even
    // when the sniffer subsequently disables address filtering.
    program_interface_address(mmio, 0, station_address);
    program_interface_address(mmio, 1, access_point_address);

    // Promiscuous mode clears the recovered class-reject bits and enables all
    // miscellaneous packet classes. A target oracle shows that the queue
    // policy words at 0x40fc/0x4100/0x4108 do not change when promiscuous mode
    // is enabled; they must retain the defaults installed by `mac_txrx_init`.
    mmio.write32(MAC_CONTROL, 0);
    modify(
        mmio,
        RX_SNIFFER_CONTROL,
        RX_SNIFFER_ENABLE | RX_SNIFFER_REJECT_MASK,
        RX_SNIFFER_ENABLE,
    );
    modify(mmio, 0x2010_40f4, 0x0000_ff00, 0x0000_ff00);
    mmio.fence();

    Ok(MacColdStartOutcome {
        handshake_samples,
        handshake_value,
    })
}
