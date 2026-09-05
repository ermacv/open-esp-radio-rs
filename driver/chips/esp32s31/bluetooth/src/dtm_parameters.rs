//! Typed Direct Test Mode channel and PHY inputs.
//!
//! The current and initial public ESP32-S31 Controller archives contain the
//! same forty-byte DTM channel permutation and BLE PHY frequency table. Their
//! composition maps DTM channel `n` to the positional frequency image `2*n`.
//! Complete current TX/RX validators and the PHY-mode-to-rate helper establish
//! the role-dependent rate images below. These are pure descriptor inputs;
//! they do not claim that a packet engine or radio event is live.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmSchedulerReceiverPhy, BluetoothDtmSchedulerTransmitterPhy,
};

/// Why an HCI DTM channel cannot enter the reviewed domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmChannelError {
    /// DTM exposes exactly the forty channels `0..=39`.
    OutsideTestChannelDomain,
}

/// One validated HCI DTM channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmChannel(u8);

impl BluetoothDtmChannel {
    /// Validate the complete channel domain accepted by both S31 DTM roles.
    pub const fn new(channel: u8) -> Result<Self, BluetoothDtmChannelError> {
        if channel > 39 {
            return Err(BluetoothDtmChannelError::OutsideTestChannelDomain);
        }
        Ok(Self(channel))
    }

    /// Return the HCI DTM channel image.
    pub const fn hci_image(self) -> u8 {
        self.0
    }

    /// Return the exact seven-bit scheduler frequency image.
    ///
    /// This is the composed output of the reviewed DTM-to-RF permutation and
    /// RF-channel-to-frequency table, not a public frequency unit.
    pub const fn scheduler_frequency_image(self) -> u8 {
        self.0 * 2
    }
}

/// Why an HCI DTM PHY selector cannot enter the reviewed domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmPhyError {
    /// Only selector images one through four are accepted by the TX entry.
    UnsupportedHciSelector,
}

/// PHY selectors accepted by the reviewed S31 DTM command entries.
///
/// Selector three is generic LE Coded for RX and the S=8 transmitter choice.
/// Selector four is the S=2 transmitter-only extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BluetoothDtmPhy {
    /// LE 1M PHY, HCI selector one.
    Le1M = 1,
    /// LE 2M PHY, HCI selector two.
    Le2M = 2,
    /// LE Coded RX or LE Coded S=8 TX, HCI selector three.
    LeCoded = 3,
    /// LE Coded S=2 TX, HCI selector four.
    LeCodedS2 = 4,
}

impl BluetoothDtmPhy {
    /// Decode the exact selector domain accepted across the reviewed entries.
    pub const fn from_hci_selector(selector: u8) -> Result<Self, BluetoothDtmPhyError> {
        match selector {
            1 => Ok(Self::Le1M),
            2 => Ok(Self::Le2M),
            3 => Ok(Self::LeCoded),
            4 => Ok(Self::LeCodedS2),
            _ => Err(BluetoothDtmPhyError::UnsupportedHciSelector),
        }
    }

    /// Return the HCI DTM selector image.
    pub const fn hci_selector(self) -> u8 {
        self as u8
    }

    /// Project this HCI selector into the transmitter-valid scheduler domain.
    pub(crate) const fn scheduler_transmitter_phy(self) -> BluetoothDtmSchedulerTransmitterPhy {
        match self {
            Self::Le1M => BluetoothDtmSchedulerTransmitterPhy::Le1M,
            Self::Le2M => BluetoothDtmSchedulerTransmitterPhy::Le2M,
            Self::LeCoded => BluetoothDtmSchedulerTransmitterPhy::LeCodedS8,
            Self::LeCodedS2 => BluetoothDtmSchedulerTransmitterPhy::LeCodedS2,
        }
    }

    /// Project this HCI selector into the receiver-valid scheduler domain.
    pub(crate) const fn scheduler_receiver_phy(
        self,
    ) -> Result<BluetoothDtmSchedulerReceiverPhy, BluetoothDtmPhyRoleError> {
        match self {
            Self::Le1M => Ok(BluetoothDtmSchedulerReceiverPhy::Le1M),
            Self::Le2M => Ok(BluetoothDtmSchedulerReceiverPhy::Le2M),
            Self::LeCoded => Ok(BluetoothDtmSchedulerReceiverPhy::LeCoded),
            Self::LeCodedS2 => Err(BluetoothDtmPhyRoleError::LeCodedS2RequiresTransmitter),
        }
    }
}

/// Why a valid DTM PHY selector is invalid for one role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmPhyRoleError {
    /// The reviewed RX command accepts only selectors one through three.
    LeCodedS2RequiresTransmitter,
}

#[cfg(test)]
mod tests;
