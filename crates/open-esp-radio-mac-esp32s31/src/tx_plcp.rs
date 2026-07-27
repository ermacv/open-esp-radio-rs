//! Pure non-HE TX vector formatting recovered from ESP32-S31 LMAC.
//!
//! This is the live extraction of the finite transformations formerly held in
//! `migration/esp32s31-hybrid-runtime/src/tx_plcp.rs`.

/// Build PLCP1 for a guarded legacy or HT non-HE rate.
///
/// For protected traffic `queue_word_low` is the hardware key selector from
/// the recovered descriptor control word. STA pairwise slot four therefore
/// contributes `4 << 17`.
pub const fn basic_non_he_plcp1_word(
    rate: u8,
    flags: u32,
    queue_word_low: u8,
    protection: u32,
    legacy_signal: u32,
) -> u32 {
    let mut word = if rate < 16 {
        0
    } else if flags & 0x0100_0000 != 0 {
        0x0600_0000
    } else {
        0x0200_0000
    };

    word = (word & 0xfe01_ffff) | (queue_word_low as u32) << 17;
    word = (word & 0xfffe_0fff) | ((rate & 0x1f) as u32) << 12;
    if rate < 16 {
        word = (word & 0xffff_f000) | (legacy_signal & 0x0000_0fff);
    }
    if flags & 0x0000_4000 != 0 && protection & 0x0000_8000 != 0 {
        word |= 0x2000_0000;
    }
    word
}

pub const fn basic_length_control_word(rts_rate: u8, entry_flags: u8, queue_word: u32) -> u32 {
    let one_symbol = ((queue_word & 0x0000_f000) == 0x0000_1000) as u32;
    (((entry_flags & 0x03) as u32) << 22)
        | (one_symbol << 1)
        | (((rts_rate as u32) << 6) & 0x0000_3fc0)
        | 0x04
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_plaintext_and_sta_slot_four_match_the_recovered_vectors() {
        assert_eq!(basic_non_he_plcp1_word(0, 0, 0, 0, 0x99), 0x99);
        assert_eq!(basic_non_he_plcp1_word(0, 0, 4, 0, 0x99), 0x0008_0099);
    }

    #[test]
    fn length_control_preserves_the_recovered_entry_and_rts_fields() {
        assert_eq!(basic_length_control_word(0, 1, 0x304), 0x0040_0004);
        assert_eq!(basic_length_control_word(9, 3, 0x1000), 0x00c0_0246);
    }
}
