//! Portable, non-retaining receive-monitor contract.
//!
//! A monitor sink observes a borrowed frame synchronously. It may copy the
//! frame into its own bounded storage, but it cannot retain the radio/DMA
//! borrow. Queue saturation is an ordinary observation loss and must never
//! backpressure the primary radio owner.

use core::{fmt, num::NonZeroU16};

use open_esp_radio_ieee80211::channel::WifiChannel;

use crate::{
    MacRxEvidence, MacRxMetadata,
    interface::{ChannelContextId, MonitorTapPoint},
};

/// Maximum number of ordered 2.4-GHz channels in one monitor hopping cycle.
///
/// This is a bounded product policy, not a claim that fourteen entries exhaust
/// every valid 20/40-MHz channel geometry. Each retained entry carries the
/// complete validated [`WifiChannel`], including its width relationship.
pub const MONITOR_CHANNEL_SEQUENCE_CAPACITY: usize = 14;

/// Allocation-free ordered channel cycle with one nonzero dwell per channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorChannelSequence {
    channels: [WifiChannel; MONITOR_CHANNEL_SEQUENCE_CAPACITY],
    length: u8,
    dwell_millis: NonZeroU16,
}

impl MonitorChannelSequence {
    pub fn new(
        channels: &[WifiChannel],
        dwell_millis: NonZeroU16,
    ) -> Result<Self, MonitorChannelSequenceError> {
        let Some(&first) = channels.first() else {
            return Err(MonitorChannelSequenceError::Empty);
        };
        if channels.len() > MONITOR_CHANNEL_SEQUENCE_CAPACITY {
            return Err(MonitorChannelSequenceError::TooMany {
                requested: channels.len(),
                capacity: MONITOR_CHANNEL_SEQUENCE_CAPACITY,
            });
        }
        let mut stored = [first; MONITOR_CHANNEL_SEQUENCE_CAPACITY];
        let mut length = 0_usize;
        for &channel in channels {
            if stored[..length].contains(&channel) {
                return Err(MonitorChannelSequenceError::Duplicate(channel));
            }
            stored[length] = channel;
            length += 1;
        }
        Ok(Self {
            channels: stored,
            length: length as u8,
            dwell_millis,
        })
    }

    pub fn channels(&self) -> &[WifiChannel] {
        &self.channels[..usize::from(self.length)]
    }

    pub const fn len(self) -> u8 {
        self.length
    }

    pub const fn first(self) -> WifiChannel {
        self.channels[0]
    }

    pub const fn dwell(self) -> NonZeroU16 {
        self.dwell_millis
    }

    pub const fn dwell_millis(self) -> u16 {
        self.dwell_millis.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelSequenceError {
    Empty,
    TooMany { requested: usize, capacity: usize },
    Duplicate(WifiChannel),
}

impl fmt::Display for MonitorChannelSequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a monitor hopping cycle cannot be empty"),
            Self::TooMany {
                requested,
                capacity,
            } => write!(
                formatter,
                "monitor hopping requested {requested} channels but capacity is {capacity}"
            ),
            Self::Duplicate(channel) => {
                write!(formatter, "monitor hopping repeats channel {channel:?}")
            }
        }
    }
}

impl core::error::Error for MonitorChannelSequenceError {}

/// Physical-channel policy for one exclusive standalone monitor role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelPolicy {
    /// Preserve the historical behavior: capture on one channel until stop.
    Fixed(WifiChannel),
    /// Repeatedly traverse one bounded ordered cycle.
    Hopping(MonitorChannelSequence),
}

impl MonitorChannelPolicy {
    pub const fn fixed(channel: WifiChannel) -> Self {
        Self::Fixed(channel)
    }

    pub const fn hopping(sequence: MonitorChannelSequence) -> Self {
        Self::Hopping(sequence)
    }

    pub const fn initial_channel(self) -> WifiChannel {
        match self {
            Self::Fixed(channel) => channel,
            Self::Hopping(sequence) => sequence.first(),
        }
    }

    pub const fn hopping_sequence(self) -> Option<MonitorChannelSequence> {
        match self {
            Self::Fixed(_) => None,
            Self::Hopping(sequence) => Some(sequence),
        }
    }
}

/// IEEE 802.11 frame class used by the bounded monitor filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MonitorFrameType {
    Management = 0,
    Control = 1,
    Data = 2,
    Extension = 3,
}

/// Fixed-size set of IEEE 802.11 frame classes.
///
/// This is deliberately not an allocated predicate tree. Filtering remains a
/// bounded amount of work in the RX bottom half, before capture-pool copying.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorFrameTypeMask(u8);

impl MonitorFrameTypeMask {
    pub const NONE: Self = Self(0);
    pub const MANAGEMENT: Self = Self(1 << MonitorFrameType::Management as u8);
    pub const CONTROL: Self = Self(1 << MonitorFrameType::Control as u8);
    pub const DATA: Self = Self(1 << MonitorFrameType::Data as u8);
    pub const EXTENSION: Self = Self(1 << MonitorFrameType::Extension as u8);
    pub const ALL: Self = Self(0x0f);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, frame_type: MonitorFrameType) -> bool {
        self.0 & (1 << frame_type as u8) != 0
    }
}

/// Allocation-free monitor admission policy.
///
/// All predicates are conjunctive. An RSSI predicate rejects frames whose
/// selected tap has no RSSI evidence. Address matching examines only address
/// fields defined by the decoded IEEE 802.11 frame class, never payload bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorFilter {
    frame_types: MonitorFrameTypeMask,
    minimum_rssi_dbm: Option<i8>,
    any_address: Option<[u8; 6]>,
}

impl MonitorFilter {
    pub const fn all() -> Self {
        Self {
            frame_types: MonitorFrameTypeMask::ALL,
            minimum_rssi_dbm: None,
            any_address: None,
        }
    }

    pub const fn frame_types(mut self, frame_types: MonitorFrameTypeMask) -> Self {
        self.frame_types = frame_types;
        self
    }

    pub const fn minimum_rssi_dbm(mut self, minimum_rssi_dbm: i8) -> Self {
        self.minimum_rssi_dbm = Some(minimum_rssi_dbm);
        self
    }

    pub const fn any_address(mut self, address: [u8; 6]) -> Self {
        self.any_address = Some(address);
        self
    }

    pub const fn selected_frame_types(self) -> MonitorFrameTypeMask {
        self.frame_types
    }

    pub const fn selected_minimum_rssi_dbm(self) -> Option<i8> {
        self.minimum_rssi_dbm
    }

    pub const fn selected_any_address(self) -> Option<[u8; 6]> {
        self.any_address
    }

    pub fn accepts<Rate>(self, frame: &MonitorFrame<'_, Rate>) -> bool {
        let Some(frame_type) = frame_type(frame.bytes) else {
            return false;
        };
        if !self.frame_types.contains(frame_type) {
            return false;
        }
        if let Some(minimum) = self.minimum_rssi_dbm {
            let observed = match frame.metadata.rssi_dbm {
                MacRxEvidence::HardwareObserved(value)
                | MacRxEvidence::ProtocolValidated(value) => value,
                MacRxEvidence::Unavailable => return false,
            };
            if observed < minimum {
                return false;
            }
        }
        self.any_address
            .is_none_or(|address| frame_contains_address(frame.bytes, frame_type, address))
    }
}

impl Default for MonitorFilter {
    fn default() -> Self {
        Self::all()
    }
}

fn frame_type(bytes: &[u8]) -> Option<MonitorFrameType> {
    let frame_control = u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]);
    Some(match (frame_control >> 2) & 0x03 {
        0 => MonitorFrameType::Management,
        1 => MonitorFrameType::Control,
        2 => MonitorFrameType::Data,
        _ => MonitorFrameType::Extension,
    })
}

fn frame_contains_address(bytes: &[u8], frame_type: MonitorFrameType, address: [u8; 6]) -> bool {
    let address_at = |offset: usize| {
        bytes
            .get(offset..offset + 6)
            .is_some_and(|candidate| candidate == address)
    };
    match frame_type {
        MonitorFrameType::Management => {
            bytes.len() >= 24 && (address_at(4) || address_at(10) || address_at(16))
        }
        MonitorFrameType::Control => {
            let subtype = bytes.first().map_or(0, |value| value >> 4);
            let has_transmitter = matches!(subtype, 8 | 9 | 10 | 11 | 14 | 15);
            address_at(4) || (has_transmitter && address_at(10))
        }
        MonitorFrameType::Data => {
            if bytes.len() < 24 {
                return false;
            }
            let to_ds_and_from_ds = bytes.get(1).is_some_and(|flags| flags & 0x03 == 0x03);
            address_at(4)
                || address_at(10)
                || address_at(16)
                || (to_ds_and_from_ds && address_at(24))
        }
        MonitorFrameType::Extension => address_at(4),
    }
}

/// One frame observed at an explicitly identified receive-pipeline boundary.
#[derive(Clone, Copy, Debug)]
pub struct MonitorFrame<'frame, Rate> {
    pub tap: MonitorTapPoint,
    pub channel_context: ChannelContextId,
    /// Bytes visible at this tap. They never include a hardware-stripped FCS.
    pub bytes: &'frame [u8],
    pub metadata: MacRxMetadata<Rate>,
    /// Complete logical MPDU length inferred from the receive status.
    ///
    /// `bytes` may be shorter when hardware consumed an authenticated cipher
    /// trailer before publishing the DMA view. The distinction prevents a
    /// capture consumer from treating such a frame as ordinary queue loss.
    pub logical_length: usize,
}

impl<Rate> MonitorFrame<'_, Rate> {
    pub const fn is_complete(&self) -> bool {
        self.bytes.len() == self.logical_length
    }
}

/// Why a best-effort monitor sink did not retain an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorDropReason {
    /// Every caller-owned capture slot was occupied.
    Full,
    /// The frame exceeds the sink's configured capture capacity.
    TooLong,
    /// Caller policy intentionally excluded this observation.
    Filtered,
}

/// Immediate result of a non-blocking monitor publication attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorPublishOutcome {
    Published,
    Dropped(MonitorDropReason),
}

/// Best-effort observation endpoint for one receive tap.
///
/// Implementations must return without waiting for capacity. An async adapter
/// can wake a consumer after copying into bounded storage, but the radio owner
/// never awaits that consumer and continues recycling its RX descriptor.
pub trait MonitorSink<Rate> {
    /// Start a fresh physical-channel capture epoch.
    ///
    /// The default preserves source-only sinks which do not retain channel
    /// metadata. Queue-backed product sinks override this edge and stamp the
    /// exact channel into every independently retained frame.
    fn begin_channel_epoch(&mut self, _channel: WifiChannel) {}

    fn try_publish(&mut self, frame: MonitorFrame<'_, Rate>) -> MonitorPublishOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MacRxMetadata;

    #[test]
    fn capture_completeness_is_distinct_from_sink_overflow() {
        let bytes = [0_u8; 24];
        let complete = MonitorFrame::<()> {
            tap: MonitorTapPoint::Normalized,
            channel_context: ChannelContextId::PRIMARY,
            bytes: &bytes,
            metadata: MacRxMetadata::unavailable(),
            logical_length: bytes.len(),
        };
        let hardware_consumed_trailer = MonitorFrame {
            logical_length: bytes.len() + 8,
            ..complete
        };

        assert!(complete.is_complete());
        assert!(!hardware_consumed_trailer.is_complete());
        assert_ne!(
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full),
            MonitorPublishOutcome::Published
        );
    }

    #[test]
    fn bounded_filter_combines_type_rssi_and_address_without_payload_matching() {
        let selected = [0x02, 1, 2, 3, 4, 5];
        let mut bytes = [0_u8; 32];
        bytes[0] = 0x08; // data
        bytes[4..10].copy_from_slice(&selected);
        let frame = MonitorFrame::<()> {
            tap: MonitorTapPoint::Normalized,
            channel_context: ChannelContextId::PRIMARY,
            bytes: &bytes,
            metadata: MacRxMetadata {
                rssi_dbm: MacRxEvidence::HardwareObserved(-51),
                ..MacRxMetadata::unavailable()
            },
            logical_length: bytes.len(),
        };
        let filter = MonitorFilter::all()
            .frame_types(MonitorFrameTypeMask::DATA)
            .minimum_rssi_dbm(-60)
            .any_address(selected);

        assert!(filter.accepts(&frame));
        assert!(!filter.minimum_rssi_dbm(-40).accepts(&frame));
        assert!(
            !filter
                .frame_types(MonitorFrameTypeMask::MANAGEMENT)
                .accepts(&frame)
        );

        let mut four_address_bytes = bytes;
        four_address_bytes[4..10].fill(0);
        four_address_bytes[24..30].copy_from_slice(&selected);
        assert!(!filter.accepts(&MonitorFrame {
            bytes: &four_address_bytes,
            ..frame
        }));
        four_address_bytes[1] = 0x03;
        assert!(filter.accepts(&MonitorFrame {
            bytes: &four_address_bytes,
            ..frame
        }));
    }

    #[test]
    fn unavailable_rssi_does_not_satisfy_a_threshold() {
        let bytes = [
            0x80_u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let frame = MonitorFrame::<()> {
            tap: MonitorTapPoint::Normalized,
            channel_context: ChannelContextId::PRIMARY,
            bytes: &bytes,
            metadata: MacRxMetadata::unavailable(),
            logical_length: bytes.len(),
        };

        assert!(!MonitorFilter::all().minimum_rssi_dbm(-100).accepts(&frame));
    }
}
