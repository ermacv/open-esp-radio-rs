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
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::mac_tx_set_plcp0` and
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
/// `libpp.a[hal_mac_tx.o]::mac_tx_set_txop_q`, offsets
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

pub const fn basic_length_control_word(rts_rate: u8, entry_flags: u8, queue_word: u32) -> u32 {
    let one_symbol = ((queue_word & 0x0000_f000) == 0x0000_1000) as u32;
    (((entry_flags & 0x03) as u32) << 22)
        | (one_symbol << 1)
        | (((rts_rate as u32) << 6) & 0x0000_3fc0)
        | 0x04
}
