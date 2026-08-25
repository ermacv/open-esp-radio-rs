//! Exact reviewed region of the Direct Test Mode link-state reset.
//!
//! Current `r_sym_ble_VikJlxpO0kioDchKDFeI` and its named same-chip predecessor
//! `r_ble_lll_dtm_reset_link_state` perform the same positional transforms
//! below. The complete body copies the private TX head from link-state
//! `+0x6c` into word `+0x00` and the private RX tail from `+0x70` into word
//! `+0x08` before each scheduler insertion. This module deliberately models
//! only the eight observed words. It is not a complete hardware descriptor,
//! exposes no publication operation and cannot claim an on-air DTM path.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmBoundSramLinkAddress;
pub use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmLinkStateReviewedWords;

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
    tx_head: Option<BluetoothDtmBoundSramLinkAddress>,
    rx_tail: Option<BluetoothDtmBoundSramLinkAddress>,
    rounded_power: u8,
    config: u8,
    role: BluetoothDtmRole,
}

impl BluetoothDtmLinkStateReset {
    /// Validate every bounded image before modifying the reviewed region.
    pub const fn new(
        tx_head: Option<BluetoothDtmBoundSramLinkAddress>,
        rx_tail: Option<BluetoothDtmBoundSramLinkAddress>,
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
            tx_head,
            rx_tail,
            rounded_power,
            config,
            role,
        })
    }

    /// Apply the complete reviewed reset transforms to the positional words.
    ///
    /// `WORD_00` retains the exact overlap from the reference body: software
    /// first replaces its low-twenty-bit TX-head link, then transforms the
    /// halfword at byte offset `+0x02`. `WORD_08` receives the private RX tail.
    /// `WORD_34` is overwritten only for the RX role.
    pub const fn apply(
        self,
        current: BluetoothDtmLinkStateReviewedWords,
    ) -> BluetoothDtmLinkStateReviewedWords {
        let tx_head = compressed_or_zero(self.tx_head);
        let rx_tail = compressed_or_zero(self.rx_tail);

        let word_00_with_link = (current.word_00 & !LOW_TWENTY_MASK) | tx_head;
        let transformed_high_half = (((word_00_with_link >> 16) as u16 & 0x600f) | 0x8ff0) as u32;

        BluetoothDtmLinkStateReviewedWords {
            word_00: (word_00_with_link & 0x0000_ffff) | (transformed_high_half << 16),
            word_04: (current.word_04 & !WORD_04_POWER_MASK) | ((self.rounded_power as u32) << 23),
            word_08: (current.word_08 & 0xf000_0000) | 0x0ff0_0000 | rx_tail,
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

    /// Return the DTM role encoded by this validated reset.
    pub const fn role(self) -> BluetoothDtmRole {
        self.role
    }

    /// Replace both list links with one freshly sampled private-chain pair.
    ///
    /// The consuming memory-graph transaction calls this with links sampled
    /// after taking ownership, so a plan cannot retain stale links from an
    /// earlier event or another graph.
    pub const fn with_private_links(
        self,
        tx_head: BluetoothDtmBoundSramLinkAddress,
        rx_tail: BluetoothDtmBoundSramLinkAddress,
    ) -> Self {
        Self {
            tx_head: Some(tx_head),
            rx_tail: Some(rx_tail),
            ..self
        }
    }
}

const fn compressed_or_zero(address: Option<BluetoothDtmBoundSramLinkAddress>) -> u32 {
    match address {
        Some(address) => address.compressed_image(),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmBoundSramLinkAddress;

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

    fn address(bits: u32) -> BluetoothDtmBoundSramLinkAddress {
        BluetoothDtmBoundSramLinkAddress::new(bits)
            .expect("test address is a nonzero compressed link")
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
