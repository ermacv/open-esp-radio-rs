//! Hardware-independent CCMP header encoding.
//!
//! Packet-number allocation, key-slot mapping and hardware publication belong
//! to the concrete LMAC. This module only encodes an already allocated
//! 48-bit packet number and logical key identifier.

pub const CCMP_HEADER_LEN: usize = 8;

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
    fn ccmp_header_uses_the_recovered_48_bit_layout() {
        assert_eq!(
            ccmp_header(0x4433_2211, 0x8877_6655, 0x80),
            [0x11, 0x22, 0, 0xa0, 0x33, 0x44, 0x55, 0x66]
        );
    }
}
