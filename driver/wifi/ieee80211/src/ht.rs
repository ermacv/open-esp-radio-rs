//! Shared, allocation-free IEEE 802.11n HT20 information elements.
//!
//! These records describe only capabilities owned by both ESP32-S31 roles:
//! one spatial stream, 20 MHz, short guard interval, immediate Block ACK and
//! an A-MPDU receive limit of 65,535 bytes. Role state decides whether they
//! may be advertised; this module only owns their byte representation.

/// Complete HT Capabilities information element for the selected HT20
/// profile.
pub const HT20_CAPABILITY_IE: [u8; 28] = [
    45, 26, 0x20, 0x00, 0x17, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];

/// Build a complete HT Operation information element for a 20-MHz BSS.
pub const fn ht20_operation_ie(primary_channel: u8) -> [u8; 24] {
    let mut element = [0_u8; 24];
    element[0] = 61;
    element[1] = 22;
    element[2] = primary_channel;
    // Secondary-channel offset zero and STA channel width zero select HT20.
    element
}

/// Whether a complete information-element stream advertises one-stream HT.
pub fn supports_ht(bytes: &[u8]) -> bool {
    let mut remaining = bytes;
    while remaining.len() >= 2 {
        let id = remaining[0];
        let length = usize::from(remaining[1]);
        let Some(record) = remaining.get(..length.saturating_add(2)) else {
            return false;
        };
        if id == 45 && length == 26 && record[5..21].iter().any(|byte| *byte != 0) {
            return true;
        }
        remaining = &remaining[record.len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ht20_records_are_complete_and_bounded() {
        assert_eq!(&HT20_CAPABILITY_IE[..2], &[45, 26]);
        assert_eq!(HT20_CAPABILITY_IE[5], 0xff);
        assert_eq!(ht20_operation_ie(6)[..3], [61, 22, 6]);
        assert!(supports_ht(&HT20_CAPABILITY_IE));
        let mut empty = [0_u8; 28];
        empty[..2].copy_from_slice(&[45, 26]);
        assert!(!supports_ht(&empty));
    }
}
