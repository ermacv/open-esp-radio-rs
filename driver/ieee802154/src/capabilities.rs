use core::ops::{BitAnd, BitOr, BitOrAssign};

use crate::{Configuration, TxMode};

/// A capability image contained bits unknown to this API version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityBitsError {
    /// Complete rejected image.
    pub bits: u16,
    /// Unknown bits from the rejected image.
    pub unknown: u16,
}

/// Portable controller capability bitset.
///
/// The private image and validating [`RadioCapabilities::from_bits`] prevent
/// adapters from silently publishing capabilities unknown to this contract.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct RadioCapabilities(u16);

impl RadioCapabilities {
    /// No optional operation is implemented.
    pub const NONE: Self = Self(0);
    /// Standalone clear-channel assessment.
    pub const CLEAR_CHANNEL_ASSESSMENT: Self = Self(1 << 0);
    /// Bounded CSMA-CA transmission.
    pub const CSMA_CA: Self = Self(1 << 1);
    /// Standalone energy detection scan.
    pub const ENERGY_SCAN: Self = Self(1 << 2);
    /// Hardware acknowledgement wait and correlation.
    pub const HARDWARE_ACKNOWLEDGEMENT: Self = Self(1 << 3);
    /// Scheduled monotonic-time transmission.
    pub const SCHEDULED_TRANSMIT: Self = Self(1 << 4);
    /// Per-request or configured transmit power.
    pub const TRANSMIT_POWER: Self = Self(1 << 5);
    /// Promiscuous receive mode.
    pub const PROMISCUOUS: Self = Self(1 << 6);
    /// Receive timestamps in a monotonic radio epoch.
    pub const RECEIVE_TIMESTAMP: Self = Self(1 << 7);
    /// Automatic acknowledgement generation.
    pub const AUTOMATIC_ACKNOWLEDGEMENT: Self = Self(1 << 8);
    /// MAC security processing offload.
    pub const SECURITY_OFFLOAD: Self = Self(1 << 9);
    /// Hardware source matching and frame-pending selection.
    pub const SOURCE_MATCH: Self = Self(1 << 10);

    const KNOWN: u16 = (1 << 11) - 1;

    /// Validate a serialized capability image.
    pub const fn from_bits(bits: u16) -> Result<Self, CapabilityBitsError> {
        let unknown = bits & !Self::KNOWN;
        if unknown == 0 {
            Ok(Self(bits))
        } else {
            Err(CapabilityBitsError { bits, unknown })
        }
    }

    /// Return the stable serialized image.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether every bit in `required` is present.
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    /// Whether this controller supports the requested transmit mode.
    pub const fn supports_tx_mode(self, mode: TxMode) -> bool {
        match mode {
            TxMode::Direct => true,
            TxMode::ClearChannelAssessment => self.contains(Self::CLEAR_CHANNEL_ASSESSMENT),
            TxMode::CsmaCa { .. } => self.contains(Self::CSMA_CA),
            TxMode::Scheduled { .. } => self.contains(Self::SCHEDULED_TRANSMIT),
        }
    }

    /// Whether this controller supports the optional part of a configuration
    /// update. Address filtering itself is a baseline operation.
    pub const fn supports_configuration(self, configuration: Configuration) -> bool {
        match configuration {
            Configuration::Promiscuous(_) => self.contains(Self::PROMISCUOUS),
            Configuration::AutomaticAcknowledgement(_) => {
                self.contains(Self::AUTOMATIC_ACKNOWLEDGEMENT)
            }
            Configuration::TransmitPowerDbm(_) => self.contains(Self::TRANSMIT_POWER),
            Configuration::PanId(_)
            | Configuration::ShortAddress(_)
            | Configuration::ExtendedAddress(_) => true,
        }
    }
}

impl BitOr for RadioCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RadioCapabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for RadioCapabilities {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RadioTimestamp;

    #[test]
    fn unknown_bits_fail_closed_and_known_sets_round_trip() {
        for bit in 0..16 {
            let image = 1_u16 << bit;
            assert_eq!(RadioCapabilities::from_bits(image).is_ok(), bit < 11);
        }
        let set = RadioCapabilities::CSMA_CA | RadioCapabilities::ENERGY_SCAN;
        assert_eq!(RadioCapabilities::from_bits(set.bits()), Ok(set));
        assert!(set.supports_tx_mode(TxMode::CsmaCa { max_backoffs: 4 }));
        assert!(!set.supports_tx_mode(TxMode::Scheduled {
            at: RadioTimestamp::from_micros(10),
        }));
    }
}
