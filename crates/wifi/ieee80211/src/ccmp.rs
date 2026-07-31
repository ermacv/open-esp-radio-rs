//! Stateless CCMP transmit-header policy recovered from ESP32-S31 net80211.
//!
//! Key ownership, DMA buffers, and hardware publication stay in the
//! target-specific MAC crate. This module is the live source of truth for the
//! finite arithmetic formerly retained in
//! `migration/esp32s31-hybrid-runtime/src/net80211_crypto.rs`.

pub const CCMP_HEADER_LEN: usize = 8;

pub const fn multicast_key_id_bits(hardware_index: u8) -> u8 {
    let logical = if hardware_index <= 1 {
        hardware_index.wrapping_add(1)
    } else {
        hardware_index.wrapping_sub(1)
    };
    (logical << 6) & 0xc0
}

pub const fn advance_vendor_tx_pn(low: u32, high: u32) -> (u32, u32) {
    let next_low = low.wrapping_add(3);
    let carry = (next_low < low) as u32;
    (next_low, high.wrapping_add(carry))
}

pub const fn ccmp_header(low: u32, high: u32, key_id_bits: u8) -> [u8; CCMP_HEADER_LEN] {
    [
        low as u8,
        (low >> 8) as u8,
        0,
        key_id_bits | 0x20,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_id_mapping_matches_the_pinned_hardware_selector() {
        assert_eq!(multicast_key_id_bits(0), 0x40);
        assert_eq!(multicast_key_id_bits(1), 0x80);
        assert_eq!(multicast_key_id_bits(2), 0x40);
        assert_eq!(multicast_key_id_bits(3), 0x80);
        assert_eq!(multicast_key_id_bits(4), 0xc0);
    }

    #[test]
    fn vendor_packet_number_step_and_carry_are_exact() {
        assert_eq!(advance_vendor_tx_pn(0, 0), (3, 0));
        assert_eq!(advance_vendor_tx_pn(u32::MAX - 1, 7), (1, 8));
    }

    #[test]
    fn ccmp_header_uses_the_recovered_48_bit_layout() {
        assert_eq!(
            ccmp_header(0x4433_2211, 0x8877_6655, 0x80),
            [0x11, 0x22, 0, 0xa0, 0x33, 0x44, 0x55, 0x66]
        );
    }
}
