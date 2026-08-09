//! Chip- and executor-independent station discovery request values.
//!
//! These types belong to the IEEE 802.11 station protocol boundary. They
//! contain no radio, DMA, interrupt, executor or security-key ownership, so a
//! chip adapter can consume the exact application policy without depending on
//! the top-level radio facade.

use core::{fmt, num::NonZeroU16};

use open_esp_radio_ieee80211::{management::MAX_SSID_LEN, station::StaAssociationPreference};

const CHANNEL_ONE_BIT: u16 = 1;
const CHANNEL_FOURTEEN_BIT: u16 = 1 << 13;
const ALL_2_4_GHZ_CHANNEL_BITS: u16 = (CHANNEL_FOURTEEN_BIT << 1) - 1;

/// Binary IEEE 802.11 SSID with its protocol length validated once.
///
/// An SSID is not required to be UTF-8. Keeping bytes here prevents an
/// application facade from inventing a text-only restriction absent from the
/// wire protocol.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub struct WifiSsid {
    bytes: [u8; MAX_SSID_LEN],
    length: u8,
}

impl WifiSsid {
    pub fn new(bytes: &[u8]) -> Result<Self, WifiSsidError> {
        if bytes.is_empty() {
            return Err(WifiSsidError::Empty);
        }
        if bytes.len() > MAX_SSID_LEN {
            return Err(WifiSsidError::TooLong {
                length: bytes.len(),
                maximum: MAX_SSID_LEN,
            });
        }
        let mut stored = [0; MAX_SSID_LEN];
        stored[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: stored,
            length: bytes.len() as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    pub const fn len(&self) -> usize {
        self.length as usize
    }

    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Debug for WifiSsid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiSsid")
            .field("bytes", &self.as_bytes())
            .finish()
    }
}

/// Invalid station SSID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSsidError {
    Empty,
    TooLong { length: usize, maximum: usize },
}

impl fmt::Display for WifiSsidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a station SSID cannot be empty"),
            Self::TooLong { length, maximum } => write!(
                formatter,
                "station SSID contains {length} bytes; the maximum is {maximum}"
            ),
        }
    }
}

impl core::error::Error for WifiSsidError {}

/// Allocation-free set of 2.4-GHz primary channels selected for scanning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct StationScanChannels(u16);

impl StationScanChannels {
    /// Channels 1 through 13. Applications must still select a set permitted
    /// by their regulatory policy.
    pub const CHANNELS_1_TO_13: Self = Self(CHANNEL_FOURTEEN_BIT - 1);

    /// All 2.4-GHz channel numbers representable by the current IEEE layer.
    pub const CHANNELS_1_TO_14: Self = Self(ALL_2_4_GHZ_CHANNEL_BITS);

    pub fn from_primary_channels(channels: &[u8]) -> Result<Self, StationScanChannelsError> {
        let mut bits = 0_u16;
        for &channel in channels {
            if !(1..=14).contains(&channel) {
                return Err(StationScanChannelsError::InvalidPrimary(channel));
            }
            bits |= CHANNEL_ONE_BIT << (channel - 1);
        }
        if bits == 0 {
            return Err(StationScanChannelsError::Empty);
        }
        Ok(Self(bits))
    }

    pub const fn contains(self, primary: u8) -> bool {
        primary != 0 && primary <= 14 && self.0 & (CHANNEL_ONE_BIT << (primary - 1)) != 0
    }

    pub const fn count(self) -> u8 {
        self.0.count_ones() as u8
    }

    pub const fn primary_channels(self) -> StationScanChannelIter {
        StationScanChannelIter { remaining: self.0 }
    }

    /// Iterate the selected set with one optional channel first.
    ///
    /// A backend may use this as a scan-latency hint without changing the
    /// application-selected channel set. An absent or unselected preference
    /// falls back to ascending channel order.
    pub const fn primary_channels_preferred(
        self,
        preferred: Option<u8>,
    ) -> StationScanChannelOrderIter {
        let preferred = match preferred {
            Some(channel) if self.contains(channel) => Some(channel),
            _ => None,
        };
        StationScanChannelOrderIter {
            remaining: match preferred {
                Some(channel) => self.0 & !(CHANNEL_ONE_BIT << (channel - 1)),
                None => self.0,
            },
            preferred,
        }
    }
}

/// Allocation-free iterator over one checked station scan channel set.
pub struct StationScanChannelIter {
    remaining: u16,
}

impl Iterator for StationScanChannelIter {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let offset = self.remaining.trailing_zeros() as u8;
        self.remaining &= self.remaining - 1;
        Some(offset + 1)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining.count_ones() as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for StationScanChannelIter {}

/// Allocation-free ordered traversal of one checked channel set.
pub struct StationScanChannelOrderIter {
    remaining: u16,
    preferred: Option<u8>,
}

impl Iterator for StationScanChannelOrderIter {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(preferred) = self.preferred.take() {
            return Some(preferred);
        }
        if self.remaining == 0 {
            return None;
        }
        let offset = self.remaining.trailing_zeros() as u8;
        self.remaining &= self.remaining - 1;
        Some(offset + 1)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining =
            self.remaining.count_ones() as usize + usize::from(self.preferred.is_some());
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for StationScanChannelOrderIter {}

/// Invalid finite station scan channel set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationScanChannelsError {
    Empty,
    InvalidPrimary(u8),
}

impl fmt::Display for StationScanChannelsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a station scan requires at least one channel"),
            Self::InvalidPrimary(channel) => {
                write!(formatter, "invalid 2.4-GHz primary channel {channel}")
            }
        }
    }
}

impl core::error::Error for StationScanChannelsError {}

/// Candidate-discovery and association policy for one station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationScanPolicy {
    channels: StationScanChannels,
    dwell_millis: NonZeroU16,
    association: StaAssociationPreference,
}

impl StationScanPolicy {
    pub const fn new(
        channels: StationScanChannels,
        dwell_millis: NonZeroU16,
        association: StaAssociationPreference,
    ) -> Self {
        Self {
            channels,
            dwell_millis,
            association,
        }
    }

    pub const fn channels(self) -> StationScanChannels {
        self.channels
    }

    pub const fn dwell_millis(self) -> u16 {
        self.dwell_millis.get()
    }

    pub const fn dwell(self) -> NonZeroU16 {
        self.dwell_millis
    }

    pub const fn association_preference(self) -> StaAssociationPreference {
        self.association
    }
}

/// Complete chip- and executor-independent candidate-discovery request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationDiscovery {
    ssid: WifiSsid,
    scan: StationScanPolicy,
}

impl StationDiscovery {
    pub const fn new(ssid: WifiSsid, scan: StationScanPolicy) -> Self {
        Self { ssid, scan }
    }

    pub const fn ssid(self) -> WifiSsid {
        self.ssid
    }

    pub const fn scan(self) -> StationScanPolicy {
        self.scan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_is_binary_and_length_checked() {
        let ssid = WifiSsid::new(&[0xff, 0, b'a']).unwrap();
        assert_eq!(ssid.as_bytes(), &[0xff, 0, b'a']);
        assert_eq!(WifiSsid::new(&[]), Err(WifiSsidError::Empty));
        assert!(matches!(
            WifiSsid::new(&[0; MAX_SSID_LEN + 1]),
            Err(WifiSsidError::TooLong { .. })
        ));
    }

    #[test]
    fn scan_channels_are_bounded_and_deduplicated() {
        let channels = StationScanChannels::from_primary_channels(&[1, 6, 6, 11]).unwrap();
        assert_eq!(channels.count(), 3);
        assert!(channels.contains(1));
        assert!(channels.contains(6));
        assert!(channels.contains(11));
        assert!(!channels.contains(14));
        assert_eq!(
            channels.primary_channels().collect::<std::vec::Vec<_>>(),
            [1, 6, 11]
        );
        assert_eq!(channels.primary_channels().len(), 3);
        assert_eq!(
            StationScanChannels::from_primary_channels(&[0]),
            Err(StationScanChannelsError::InvalidPrimary(0))
        );
    }

    #[test]
    fn discovery_keeps_protocol_policy_chip_independent() {
        let ssid = WifiSsid::new(b"portable-station").unwrap();
        let scan = StationScanPolicy::new(
            StationScanChannels::from_primary_channels(&[1, 6, 11]).unwrap(),
            NonZeroU16::new(40).unwrap(),
            StaAssociationPreference::Automatic,
        );
        let discovery = StationDiscovery::new(ssid, scan);
        assert_eq!(discovery.ssid(), ssid);
        assert_eq!(discovery.scan(), scan);
    }

    #[test]
    fn preferred_scan_order_preserves_the_exact_selected_set() {
        let channels = StationScanChannels::from_primary_channels(&[1, 6, 11, 14]).unwrap();
        assert_eq!(
            channels
                .primary_channels_preferred(Some(11))
                .collect::<std::vec::Vec<_>>(),
            [11, 1, 6, 14]
        );
        assert_eq!(
            channels
                .primary_channels_preferred(Some(9))
                .collect::<std::vec::Vec<_>>(),
            [1, 6, 11, 14]
        );
    }
}
