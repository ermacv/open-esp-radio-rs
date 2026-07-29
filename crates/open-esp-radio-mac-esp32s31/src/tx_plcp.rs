/// Build the queue PLCP0 word for the guarded non-HE submission branch.
pub const fn basic_plcp0_word(metadata_address: usize, flags: u32) -> u32 {
    let mut word = (metadata_address as u32 & 0x000f_ffff) | 0x0060_0000;

    if flags & 0x0000_0402 != 0 || flags & 0x4048_0000 == 0x0040_0000 {
        return word;
    }

    let format = if flags & 0x0010_0000 != 0 {
        3
    } else {
        ((flags >> 19) & 1) + 1
    };
    word = (word & 0xf8ff_ffff) | (format << 24);
    word
}

/// Build the bounded HE SU A-MPDU PLCP0/control image.
///
/// SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::mac_tx_set_plcp0` and
/// `HIL_VENDOR_HE20_MCS9_SU_2026_07_29`. Descriptor flags `0xc0403009`
/// select queue format five; the address remains the direct DMA-chain head.
pub const fn he_ampdu_plcp0_word(descriptor_address: usize) -> u32 {
    (descriptor_address as u32 & 0x000f_ffff) | 0x0560_0000
}

/// Build the HE PLCP1/vector word for one canonical rate code.
///
/// The HE branch uses format image `0x04000000`; its low five rate bits and
/// descriptor control byte use the same positions as complete
/// `mac_tx_set_plcp1`. Live MCS9 with control byte four produced
/// `0x04083000`.
pub const fn he_plcp1_word(rate: u8, descriptor_control: u8) -> u32 {
    0x0400_0000 | ((descriptor_control as u32) << 17) | (((rate & 0x1f) as u32) << 12)
}

/// Apply the ordinary non-TXOP descriptor policy to the PLCP0/control word.
///
/// This is a separate formatter edge from [`basic_plcp0_word`]. Complete
/// `_oracles/libpp.a[hal_mac_tx.o]::mac_tx_set_txop_q`, offsets
/// `0x86..0xaa`, sets queue-control bit 22 only when descriptor flag bits
/// `7:6` equal binary `10`, and clears bit 22 for every other value. The
/// complete `ppTxFragmentProc` common legacy setup supplies bit seven for the
/// bounded direct CCMP path, while the receiver class independently supplies
/// zero or bit one. Both therefore select the set branch.
///
/// The electrical meaning of bit 22 is not inferred from the function name;
/// only the instruction-exact descriptor-to-register mapping is exposed.
pub const fn apply_basic_txop_control_word(word: u32, flags: u32) -> u32 {
    if flags & 0x0000_00c0 == 0x0000_0080 {
        word | 0x0040_0000
    } else {
        word & !0x0040_0000
    }
}

/// Build the queue PLCP1 word for a rate already guarded to the finite legacy
/// or HT non-HE range 0..=35.
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

/// Build the packed HT-SIG word for one S31 non-HE queue vector.
///
/// `channel_width_40` becomes HT-SIG1 bit 7. Complete
/// `_oracles/libpp.a[hal_mac_tx.o]::mac_tx_set_htsig` copies descriptor word1
/// bit 15 to this exact bit; its standard 802.11n meaning is CBW (zero for
/// 20 MHz, one for 40 MHz).
pub const fn ht_htsig_word(rate: u8, channel_width_40: bool, length: u32, aggregate: bool) -> u32 {
    let mcs = if rate <= 25 { rate - 16 } else { rate - 26 };
    let low = mcs | ((channel_width_40 as u8) << 7);
    let high = 0x07 | ((aggregate as u8) << 3) | (((rate >= 26) as u8) << 7);
    u32::from_le_bytes([low, length as u8, (length >> 8) as u8, high])
}

pub const fn basic_htsig_word(rate: u8, channel_width_40: bool, length: u32) -> u32 {
    ht_htsig_word(rate, channel_width_40, length, false)
}

pub const fn basic_length_control_word(rts_rate: u8, entry_flags: u8, queue_word: u32) -> u32 {
    let one_symbol = ((queue_word & 0x0000_f000) == 0x0000_1000) as u32;
    (((entry_flags & 0x03) as u32) << 22)
        | (one_symbol << 1)
        | (((rts_rate as u32) << 6) & 0x0000_3fc0)
        | 0x04
}

pub const fn basic_data_length_word(rate: u8, length: u32, entry_flags: u8) -> u32 {
    let rate = if rate <= 25 { rate - 16 } else { rate - 26 };
    (((entry_flags & 0x03) as u32) << 22) | (length & 0x003f_ffff) | ((rate as u32) << 28)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_basic_txop_control_word, basic_data_length_word, basic_htsig_word,
        basic_length_control_word, basic_non_he_plcp1_word, basic_plcp0_word, he_ampdu_plcp0_word,
        he_plcp1_word, ht_htsig_word,
    };

    const ADDRESS: usize = 0x2f12_3456;
    const BASE: u32 = 0x0062_3456;

    #[test]
    fn masks_the_metadata_address_and_preserves_the_base_format() {
        assert_eq!(basic_plcp0_word(ADDRESS, 0x0000_0400), BASE);
        assert_eq!(basic_plcp0_word(ADDRESS, 0x0000_0002), BASE);
        assert_eq!(basic_plcp0_word(ADDRESS, 0x0040_0000), BASE);
    }

    #[test]
    fn reproduces_each_guarded_non_he_format() {
        assert_eq!(basic_plcp0_word(ADDRESS, 0), BASE | 0x0100_0000);
        assert_eq!(basic_plcp0_word(ADDRESS, 0x0008_0000), BASE | 0x0200_0000);
        assert_eq!(basic_plcp0_word(ADDRESS, 0x0010_0000), BASE | 0x0300_0000);
    }

    #[test]
    fn reproduces_the_vendor_he_mcs9_plcp_prefix() {
        assert_eq!(he_ampdu_plcp0_word(0x2f03_1638), 0x0563_1638);
        assert_eq!(he_plcp1_word(35, 4), 0x0408_3000);
    }

    #[test]
    fn applies_the_complete_txop_queue_bit_22_mapping() {
        assert_eq!(apply_basic_txop_control_word(0x0162_3456, 0), 0x0122_3456);
        assert_eq!(
            apply_basic_txop_control_word(0x0162_3456, 0x0000_0040),
            0x0122_3456
        );
        assert_eq!(
            apply_basic_txop_control_word(0x0122_3456, 0x0000_0080),
            0x0162_3456
        );
        assert_eq!(
            apply_basic_txop_control_word(0x0162_3456, 0x0000_00c0),
            0x0122_3456
        );
    }

    #[test]
    fn reproduces_basic_ht_plcp1_format_and_rate_fields() {
        assert_eq!(basic_non_he_plcp1_word(16, 0, 0, 0, 0x4188), 0x0201_0000);
        assert_eq!(
            basic_non_he_plcp1_word(35, 0x0100_0000, 0, 0, 0x00d0),
            0x0600_3000
        );
        assert_eq!(
            basic_non_he_plcp1_word(16, 0x0100_0000, 0x12, 0, 0),
            0x0625_0000
        );
    }

    #[test]
    fn reproduces_legacy_plcp1_without_ht_format_bits() {
        assert_eq!(
            basic_non_he_plcp1_word(0, 0, 0, 0, 0x0000_4188),
            0x0000_0188
        );
        assert_eq!(
            basic_non_he_plcp1_word(0, 0, 0, 0, 0x0000_00d0),
            0x0000_00d0
        );
        assert_eq!(
            basic_non_he_plcp1_word(15, 0, 0x12, 0, 0x1234_5abc),
            0x0024_fabc
        );
    }

    #[test]
    fn sets_protection_only_when_both_guard_bits_are_present() {
        assert_eq!(
            basic_non_he_plcp1_word(16, 0x0000_4000, 0, 0, 0),
            0x0201_0000
        );
        assert_eq!(
            basic_non_he_plcp1_word(16, 0, 0, 0x0000_8000, 0),
            0x0201_0000
        );
        assert_eq!(
            basic_non_he_plcp1_word(16, 0x0000_4000, 0, 0x0000_8000, 0),
            0x2201_0000
        );
    }

    #[test]
    fn reproduces_htsig_rate_class_cbw_and_length() {
        assert_eq!(basic_htsig_word(16, false, 0x1234), 0x0712_3400);
        assert_eq!(basic_htsig_word(25, true, 0x3fff), 0x073f_ff89);
        assert_eq!(basic_htsig_word(26, false, 0x0201), 0x8702_0100);
        assert_eq!(basic_htsig_word(35, true, 0x0001), 0x8700_0189);
    }

    #[test]
    fn sets_the_recovered_ht_ampdu_bit_independently_of_length() {
        assert_eq!(ht_htsig_word(16, false, 0x248e, true), 0x0f24_8e00);
        assert_eq!(ht_htsig_word(35, true, 0x248e, true), 0x8f24_8e89);
        assert_eq!(ht_htsig_word(16, false, 0x248e, false), 0x0724_8e00);
    }

    #[test]
    fn reproduces_length_control_fields() {
        assert_eq!(basic_length_control_word(11, 0, 0), 0x0000_02c4);
        assert_eq!(basic_length_control_word(9, 3, 0x0000_1000), 0x00c0_0246);
    }

    #[test]
    fn reproduces_data_length_fields() {
        assert_eq!(basic_data_length_word(16, 0x1234, 0), 0x0000_1234);
        assert_eq!(basic_data_length_word(25, 0x3fff, 3), 0x90c0_3fff);
        assert_eq!(basic_data_length_word(35, 0x0201, 1), 0x9040_0201);
    }
}
