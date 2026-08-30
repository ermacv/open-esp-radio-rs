//! Typed Direct Test Mode channel and PHY inputs.
//!
//! The current and initial public ESP32-S31 Controller archives contain the
//! same forty-byte DTM channel permutation and BLE PHY frequency table. Their
//! composition maps DTM channel `n` to the positional frequency image `2*n`.
//! Complete current TX/RX validators and the PHY-mode-to-rate helper establish
//! the role-dependent rate images below. These are pure descriptor inputs;
//! they do not claim that a packet engine or radio event is live.

#![forbid(unsafe_code)]

use crate::BluetoothDtmRole;

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

    /// Return the exact two-bit scheduler rate image when the role accepts it.
    pub const fn scheduler_rate_image(
        self,
        role: BluetoothDtmRole,
    ) -> Result<u8, BluetoothDtmPhyRoleError> {
        match (self, role) {
            (Self::Le1M, _) => Ok(0),
            (Self::Le2M, _) => Ok(1),
            (Self::LeCoded, BluetoothDtmRole::Transmitter) => Ok(2),
            (Self::LeCoded, BluetoothDtmRole::Receiver) => Ok(3),
            (Self::LeCodedS2, BluetoothDtmRole::Transmitter) => Ok(3),
            (Self::LeCodedS2, BluetoothDtmRole::Receiver) => {
                Err(BluetoothDtmPhyRoleError::LeCodedS2RequiresTransmitter)
            }
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
mod tests {
    use super::{
        BluetoothDtmChannel, BluetoothDtmChannelError, BluetoothDtmPhy, BluetoothDtmPhyError,
        BluetoothDtmPhyRoleError,
    };
    use crate::BluetoothDtmRole;

    #[test]
    fn channel_domain_accepts_its_bounds_and_rejects_the_first_outside_image() {
        assert!(BluetoothDtmChannel::new(0).is_ok());
        assert!(BluetoothDtmChannel::new(39).is_ok());
        assert_eq!(
            BluetoothDtmChannel::new(40),
            Err(BluetoothDtmChannelError::OutsideTestChannelDomain)
        );
    }

    #[test]
    fn phy_role_domain_rejects_only_the_transmitter_only_receiver_case() {
        let tx = BluetoothDtmRole::Transmitter;
        let rx = BluetoothDtmRole::Receiver;

        for phy in [
            BluetoothDtmPhy::Le1M,
            BluetoothDtmPhy::Le2M,
            BluetoothDtmPhy::LeCoded,
            BluetoothDtmPhy::LeCodedS2,
        ] {
            assert!(phy.scheduler_rate_image(tx).is_ok());
        }
        for phy in [
            BluetoothDtmPhy::Le1M,
            BluetoothDtmPhy::Le2M,
            BluetoothDtmPhy::LeCoded,
        ] {
            assert!(phy.scheduler_rate_image(rx).is_ok());
        }
        assert_eq!(
            BluetoothDtmPhy::LeCodedS2.scheduler_rate_image(rx),
            Err(BluetoothDtmPhyRoleError::LeCodedS2RequiresTransmitter)
        );
    }

    #[test]
    fn hci_phy_decoder_accepts_only_the_reviewed_selector_domain() {
        assert_eq!(
            BluetoothDtmPhy::from_hci_selector(1),
            Ok(BluetoothDtmPhy::Le1M)
        );
        assert_eq!(
            BluetoothDtmPhy::from_hci_selector(2),
            Ok(BluetoothDtmPhy::Le2M)
        );
        assert_eq!(
            BluetoothDtmPhy::from_hci_selector(3),
            Ok(BluetoothDtmPhy::LeCoded)
        );
        assert_eq!(
            BluetoothDtmPhy::from_hci_selector(4),
            Ok(BluetoothDtmPhy::LeCodedS2)
        );
        assert_eq!(
            BluetoothDtmPhy::from_hci_selector(0),
            Err(BluetoothDtmPhyError::UnsupportedHciSelector)
        );
        assert_eq!(
            BluetoothDtmPhy::from_hci_selector(5),
            Err(BluetoothDtmPhyError::UnsupportedHciSelector)
        );
        assert_eq!(BluetoothDtmPhy::Le1M.hci_selector(), 1);
        assert_eq!(BluetoothDtmPhy::Le2M.hci_selector(), 2);
        assert_eq!(BluetoothDtmPhy::LeCoded.hci_selector(), 3);
        assert_eq!(BluetoothDtmPhy::LeCodedS2.hci_selector(), 4);
    }
}
