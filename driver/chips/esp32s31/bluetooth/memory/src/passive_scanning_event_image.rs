//! Private SRAM images for the restricted legacy passive-scanning profile.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

use crate::{
    le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
    le_tx_power::rounded_tx_power,
    sram_link::BluetoothControllerSramLinkAddress,
};

pub(super) const BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS: usize = 0x84 / 4;
const RX_HEAD_MASK: u32 = 0x000f_ffff;
const ROUNDED_POWER_MASK: u32 = 0x0f80_0000;
const WORD_00: usize = 0;
const WORD_04: usize = 1;
const WORD_08: usize = 2;
const WORD_0C: usize = 3;
const WORD_14: usize = 5;
const WORD_18: usize = 6;
const WORD_24: usize = 9;
const WORD_2C: usize = 11;
const WORD_30: usize = 12;
const WORD_34: usize = 13;
const WORD_38: usize = 14;
const WORD_48: usize = 18;
const WORD_50: usize = 20;

/// Physical default transmit-power request retained by the scanner profile.
///
/// Passive scanning never transmits, but the common hardware link state still
/// carries the controller's rounded default-power projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanDefaultTxPowerDbm(i8);

impl BluetoothPassiveScanDefaultTxPowerDbm {
    /// Bind one signed dBm request to the scanner hardware profile.
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    /// Return the physical signed request without exposing its SRAM encoding.
    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Dynamic inputs to the single supported passive-scanning reset profile.
///
/// Construction fixes LE 1M, public own address, accept-all filtering,
/// disabled privacy and disabled periodic synchronization. Callers cannot
/// supply positional descriptor words or vendor option images.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPassiveScanResetConfig {
    default_tx_power: BluetoothPassiveScanDefaultTxPowerDbm,
    controller_time: BluetoothControllerLatchedTime,
}

impl BluetoothPassiveScanResetConfig {
    /// Construct the restricted passive LE 1M profile.
    pub const fn le_1m_public_accept_all(
        default_tx_power: BluetoothPassiveScanDefaultTxPowerDbm,
        controller_time: BluetoothControllerLatchedTime,
    ) -> Self {
        Self {
            default_tx_power,
            controller_time,
        }
    }

    pub(super) const fn default_tx_power(self) -> BluetoothPassiveScanDefaultTxPowerDbm {
        self.default_tx_power
    }

    pub(super) const fn controller_time(self) -> BluetoothControllerLatchedTime {
        self.controller_time
    }
}

/// Opaque projection of the bound scanner RX head into a link-state word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothPassiveScanRxHeadProjection(u32);

impl BluetoothPassiveScanRxHeadProjection {
    pub(super) const fn from_bound(address: BluetoothControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn apply(self, word: u32) -> u32 {
        (word & !RX_HEAD_MASK) | self.0
    }
}

/// Complete private link-state allocation for the first scanner profile.
///
/// Raw words never leave the memory crate. The storage layer may only install
/// this image after binding the real RX head of the same pinned graph.
#[derive(Clone, Copy)]
pub(super) struct BluetoothPassiveScanLinkStateImage {
    words: [u32; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS],
}

impl BluetoothPassiveScanLinkStateImage {
    /// Build the exact reset result over a zero-based open-driver allocation.
    pub(super) const fn restricted_passive_le_1m(
        rx_head: BluetoothPassiveScanRxHeadProjection,
        config: BluetoothPassiveScanResetConfig,
    ) -> Self {
        let mut words = [0; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS];

        // All masks and positional images remain private to this SRAM codec.
        words[WORD_00] = 0x1ff0_0000;
        words[WORD_04] =
            ((rounded_tx_power(config.default_tx_power().dbm()) as u32) << 23) & ROUNDED_POWER_MASK;
        words[WORD_08] = rx_head.apply(0x4ff0_0000);
        words[WORD_0C] = 0xa010_0000;
        words[WORD_14] = 0x0400_0000;
        words[WORD_18] = 0x4000_0000;
        words[WORD_24] = 0x0110_0000;
        words[WORD_2C] = BluetoothLeCrcInit::LE_PRESET.apply_to_controller_word(0);
        words[WORD_30] = 0x0000_1e00;
        words[WORD_34] = config.controller_time().bits();
        words[WORD_38] = BluetoothLeAccessAddress::PRIMARY_ADVERTISING.controller_image();
        words[WORD_48] = 0x0000_0200;
        words[WORD_50] = 0x0300_0000;

        Self { words }
    }

    pub(super) const fn words(self) -> [u32; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS] {
        self.words
    }

    #[cfg(test)]
    pub(super) const fn from_words(words: [u32; BLUETOOTH_PASSIVE_SCAN_LINK_STATE_WORDS]) -> Self {
        Self { words }
    }

    #[cfg(test)]
    pub(super) const fn retains_rx_head(self, head: BluetoothPassiveScanRxHeadProjection) -> bool {
        self.words[WORD_08] & RX_HEAD_MASK == head.0
    }

    #[cfg(test)]
    pub(super) const fn crc_init(self) -> BluetoothLeCrcInit {
        BluetoothLeCrcInit::from_controller_word(self.words[WORD_2C])
    }

    #[cfg(test)]
    pub(super) const fn access_address(self) -> BluetoothLeAccessAddress {
        BluetoothLeAccessAddress::from_controller_image(self.words[WORD_38])
    }

    #[cfg(test)]
    pub(super) const fn controller_time(self) -> u32 {
        self.words[WORD_34]
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

    use crate::{
        le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
        sram_link::BluetoothControllerSramLinkAddress,
    };

    use super::{
        BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanLinkStateImage,
        BluetoothPassiveScanResetConfig, BluetoothPassiveScanRxHeadProjection,
    };

    #[test]
    fn restricted_profile_retains_only_semantic_dynamic_inputs() {
        let head = BluetoothPassiveScanRxHeadProjection::from_bound(
            BluetoothControllerSramLinkAddress::new(0x2f00_0100)
                .expect("the model header is a nonzero controller link"),
        );
        let config = BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
            BluetoothPassiveScanDefaultTxPowerDbm::new(0),
            BluetoothControllerLatchedTime::from_bits(0x1234_5678),
        );

        let image = BluetoothPassiveScanLinkStateImage::restricted_passive_le_1m(head, config);

        assert!(image.retains_rx_head(head));
        assert_eq!(image.crc_init(), BluetoothLeCrcInit::LE_PRESET);
        assert_eq!(
            image.access_address(),
            BluetoothLeAccessAddress::PRIMARY_ADVERTISING
        );
        assert_eq!(image.controller_time(), config.controller_time().bits());
    }
}
