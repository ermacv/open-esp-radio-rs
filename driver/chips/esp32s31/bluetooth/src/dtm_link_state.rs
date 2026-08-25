//! Exact reviewed region of the Direct Test Mode link-state reset.
//!
//! Current `r_sym_ble_VikJlxpO0kioDchKDFeI` and its named same-chip predecessor
//! `r_ble_lll_dtm_reset_link_state` perform the same positional transforms
//! below. This module deliberately models only the eight observed words. It is
//! not a complete hardware descriptor, exposes no publication operation and
//! cannot claim an on-air DTM path.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::BluetoothControllerSramAddress;

const LOW_TWENTY_MASK: u32 = 0x000f_ffff;
const WORD_04_POWER_MASK: u32 = 0x0f80_0000;
const WORD_50_CONFIG_MASK: u32 = 0x3f00_0000;

/// DTM role selected by the complete event constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRole {
    /// Repeated transmitter-test events use role image one.
    Transmitter,
    /// Repeated receiver-test events use role image two.
    Receiver,
}

/// Why one reviewed DTM reset input cannot be represented exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmLinkStateResetError {
    /// The rounded power image is wider than its observed five-bit position.
    RoundedPowerOutsideFiveBits,
    /// The positional configuration image is wider than six bits.
    ConfigOutsideSixBits,
}

/// Validated dynamic inputs to one DTM link-state reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmLinkStateReset {
    next_link: Option<BluetoothControllerSramAddress>,
    buffer_header: Option<BluetoothControllerSramAddress>,
    rounded_power: u8,
    config: u8,
    role: BluetoothDtmRole,
}

impl BluetoothDtmLinkStateReset {
    /// Validate every bounded image before modifying the reviewed region.
    pub const fn new(
        next_link: Option<BluetoothControllerSramAddress>,
        buffer_header: Option<BluetoothControllerSramAddress>,
        rounded_power: u8,
        config: u8,
        role: BluetoothDtmRole,
    ) -> Result<Self, BluetoothDtmLinkStateResetError> {
        if rounded_power > 0x1f {
            return Err(BluetoothDtmLinkStateResetError::RoundedPowerOutsideFiveBits);
        }
        if config > 0x3f {
            return Err(BluetoothDtmLinkStateResetError::ConfigOutsideSixBits);
        }
        Ok(Self {
            next_link,
            buffer_header,
            rounded_power,
            config,
            role,
        })
    }

    /// Apply the complete reviewed reset transforms to the positional words.
    ///
    /// `WORD_00` retains the exact overlap from the reference body: software
    /// first replaces its low-twenty-bit link, then transforms the halfword at
    /// byte offset `+0x02`. `WORD_34` is overwritten only for the RX role.
    pub const fn apply(
        self,
        current: BluetoothDtmLinkStateReviewedWords,
    ) -> BluetoothDtmLinkStateReviewedWords {
        let next_link = compressed_or_zero(self.next_link);
        let buffer_header = compressed_or_zero(self.buffer_header);

        let word_00_with_link = (current.word_00 & !LOW_TWENTY_MASK) | next_link;
        let transformed_high_half = (((word_00_with_link >> 16) as u16 & 0x600f) | 0x8ff0) as u32;

        BluetoothDtmLinkStateReviewedWords {
            word_00: (word_00_with_link & 0x0000_ffff) | (transformed_high_half << 16),
            word_04: (current.word_04 & !WORD_04_POWER_MASK) | ((self.rounded_power as u32) << 23),
            word_08: (current.word_08 & 0xf000_0000) | 0x0ff0_0000 | buffer_header,
            word_14: current.word_14 | 0xc000_0000,
            word_2c: (current.word_2c & 0xff00_0000) | 0x0055_5555,
            word_34: match self.role {
                BluetoothDtmRole::Transmitter => current.word_34,
                BluetoothDtmRole::Receiver => 0,
            },
            word_38: 0x7176_4129,
            word_50: (current.word_50 & !WORD_50_CONFIG_MASK) | ((self.config as u32) << 24),
        }
    }
}

const fn compressed_or_zero(address: Option<BluetoothControllerSramAddress>) -> u32 {
    match address {
        Some(address) => address.compressed_image(),
        None => 0,
    }
}

/// The eight link-state words whose reset behavior is complete.
///
/// Names are byte offsets, not semantic descriptor fields. The omitted bytes
/// and the hardware consumer remain unresolved, so this value has no method
/// that yields a controller address or transfers hardware ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmLinkStateReviewedWords {
    /// Complete word at byte offset `+0x00`.
    pub word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub word_04: u32,
    /// Complete word at byte offset `+0x08`.
    pub word_08: u32,
    /// Complete word at byte offset `+0x14`.
    pub word_14: u32,
    /// Complete word at byte offset `+0x2c`.
    pub word_2c: u32,
    /// Complete word at byte offset `+0x34`.
    pub word_34: u32,
    /// Complete word at byte offset `+0x38`.
    pub word_38: u32,
    /// Complete word at byte offset `+0x50`.
    pub word_50: u32,
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothControllerSramAddress;

    use super::{
        BluetoothDtmLinkStateReset, BluetoothDtmLinkStateResetError,
        BluetoothDtmLinkStateReviewedWords, BluetoothDtmRole,
    };

    const CURRENT: BluetoothDtmLinkStateReviewedWords = BluetoothDtmLinkStateReviewedWords {
        word_00: 0x1234_5678,
        word_04: 0xa5a5_5a5a,
        word_08: 0xb123_4567,
        word_14: 0x0123_4567,
        word_2c: 0xabcd_ef01,
        word_34: 0xdead_beef,
        word_38: 0,
        word_50: 0x9123_4567,
    };

    fn address(bits: u32) -> BluetoothControllerSramAddress {
        BluetoothControllerSramAddress::new(bits).expect("test address is representable")
    }

    #[test]
    fn reset_matches_all_complete_positional_transforms() {
        let reset = BluetoothDtmLinkStateReset::new(
            Some(address(0x2f3f_fffc)),
            Some(address(0x2f00_0040)),
            0x15,
            0x2a,
            BluetoothDtmRole::Receiver,
        )
        .expect("bounded reset inputs are accepted");
        let reset_words = reset.apply(CURRENT);

        assert_eq!(reset_words.word_00, 0x8fff_ffff);
        assert_eq!(reset_words.word_04, 0xaaa5_5a5a);
        assert_eq!(reset_words.word_08, 0xbff0_0010);
        assert_eq!(reset_words.word_14, 0xc123_4567);
        assert_eq!(reset_words.word_2c, 0xab55_5555);
        assert_eq!(reset_words.word_34, 0);
        assert_eq!(reset_words.word_38, 0x7176_4129);
        assert_eq!(reset_words.word_50, 0xaa23_4567);
    }

    #[test]
    fn zero_links_and_tx_role_preserve_only_the_reviewed_state() {
        let reset =
            BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Transmitter)
                .expect("zero images are representable");
        let reset_words = reset.apply(CURRENT);

        assert_eq!(reset_words.word_00, 0x8ff0_0000);
        assert_eq!(reset_words.word_04, CURRENT.word_04 & !0x0f80_0000);
        assert_eq!(reset_words.word_08, 0xbff0_0000);
        assert_eq!(reset_words.word_34, CURRENT.word_34);
        assert_eq!(reset_words.word_50, CURRENT.word_50 & !0x3f00_0000);
    }

    #[test]
    fn reset_rejects_images_wider_than_the_complete_fields() {
        assert_eq!(
            BluetoothDtmLinkStateReset::new(None, None, 0x20, 0, BluetoothDtmRole::Transmitter),
            Err(BluetoothDtmLinkStateResetError::RoundedPowerOutsideFiveBits)
        );
        assert_eq!(
            BluetoothDtmLinkStateReset::new(None, None, 0, 0x40, BluetoothDtmRole::Receiver),
            Err(BluetoothDtmLinkStateResetError::ConfigOutsideSixBits)
        );
    }
}
