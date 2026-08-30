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
pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmLinkStateReviewedWords, BluetoothDtmRole,
};

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
        current.apply_reset(
            self.tx_head,
            self.rx_tail,
            self.rounded_power,
            self.config,
            self.role,
        )
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
