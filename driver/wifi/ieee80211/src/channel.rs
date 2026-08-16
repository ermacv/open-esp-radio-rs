//! Typed IEEE 802.11 channel definitions.
//!
//! Protocol and application code uses a primary channel plus an explicit
//! bandwidth relationship. Chip PHY leaves may lower this value into their
//! recovered register encodings, but those encodings do not cross this
//! portable boundary.

use core::fmt;

/// Channel width and secondary-channel relationship for one 2.4 GHz channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiChannelWidth {
    Mhz20,
    Mhz40Above,
    Mhz40Below,
}

/// Validated IEEE 802.11 2.4 GHz channel definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiChannel {
    primary: u8,
    width: WifiChannelWidth,
}

impl WifiChannel {
    /// Construct a 2.4 GHz channel supported by the current radio family.
    ///
    /// Channel 14 is admitted only at 20 MHz. Forty-megahertz definitions
    /// require the complete secondary channel to remain in channels 1..=13.
    pub const fn new_2_4_ghz(
        primary: u8,
        width: WifiChannelWidth,
    ) -> Result<Self, WifiChannelError> {
        if primary == 0 || primary > 14 {
            return Err(WifiChannelError::InvalidPrimary(primary));
        }
        let secondary_valid = match width {
            WifiChannelWidth::Mhz20 => true,
            WifiChannelWidth::Mhz40Above => primary <= 9,
            WifiChannelWidth::Mhz40Below => primary >= 5 && primary <= 13,
        };
        if !secondary_valid {
            return Err(WifiChannelError::InvalidSecondary { primary, width });
        }
        Ok(Self { primary, width })
    }

    pub const fn mhz20(primary: u8) -> Result<Self, WifiChannelError> {
        Self::new_2_4_ghz(primary, WifiChannelWidth::Mhz20)
    }

    pub const fn primary(self) -> u8 {
        self.primary
    }

    pub const fn width(self) -> WifiChannelWidth {
        self.width
    }

    pub const fn bandwidth_mhz(self) -> u16 {
        match self.width {
            WifiChannelWidth::Mhz20 => 20,
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => 40,
        }
    }

    pub const fn primary_frequency_mhz(self) -> u16 {
        if self.primary == 14 {
            2_484
        } else {
            2_407 + self.primary as u16 * 5
        }
    }

    pub const fn center_frequency_mhz(self) -> u16 {
        match self.width {
            WifiChannelWidth::Mhz20 => self.primary_frequency_mhz(),
            WifiChannelWidth::Mhz40Above => self.primary_frequency_mhz() + 10,
            WifiChannelWidth::Mhz40Below => self.primary_frequency_mhz() - 10,
        }
    }
}

/// Invalid portable channel geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiChannelError {
    InvalidPrimary(u8),
    InvalidSecondary {
        primary: u8,
        width: WifiChannelWidth,
    },
}

impl fmt::Display for WifiChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrimary(primary) => {
                write!(formatter, "invalid 2.4 GHz primary channel {primary}")
            }
            Self::InvalidSecondary { primary, width } => write!(
                formatter,
                "primary channel {primary} cannot use {width:?} secondary-channel geometry"
            ),
        }
    }
}

impl core::error::Error for WifiChannelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_fourteen_has_its_nonuniform_frequency() {
        let channel = WifiChannel::mhz20(14).unwrap();
        assert_eq!(channel.primary_frequency_mhz(), 2_484);
        assert_eq!(channel.center_frequency_mhz(), 2_484);
    }

    #[test]
    fn forty_megahertz_geometry_is_bounded_to_real_secondary_channels() {
        let above = WifiChannel::new_2_4_ghz(9, WifiChannelWidth::Mhz40Above).unwrap();
        let below = WifiChannel::new_2_4_ghz(5, WifiChannelWidth::Mhz40Below).unwrap();
        assert_eq!(above.center_frequency_mhz(), 2_462);
        assert_eq!(below.center_frequency_mhz(), 2_422);
        assert_eq!(
            WifiChannel::new_2_4_ghz(10, WifiChannelWidth::Mhz40Above),
            Err(WifiChannelError::InvalidSecondary {
                primary: 10,
                width: WifiChannelWidth::Mhz40Above,
            })
        );
        assert_eq!(
            WifiChannel::new_2_4_ghz(4, WifiChannelWidth::Mhz40Below),
            Err(WifiChannelError::InvalidSecondary {
                primary: 4,
                width: WifiChannelWidth::Mhz40Below,
            })
        );
    }
}
