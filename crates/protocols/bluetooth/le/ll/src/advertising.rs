//! Legacy advertising PDU and event state.
//!
//! The first closed role is non-connectable, non-scannable undirected
//! advertising. It needs no receive window or response policy and therefore
//! exposes the smallest honest Link Layer/backend contract. One prepared event
//! contains the single `ADV_NONCONN_IND` PDU and the complete ordered primary
//! channel plan. Only completion of that whole backend event may advance the
//! affine Link Layer owner.

use crate::{LeDeviceAddress, LeDeviceAddressKind};

/// Maximum Host advertising data carried by a legacy advertising PDU.
pub const LEGACY_ADVERTISING_DATA_CAPACITY: usize = 31;
/// Complete encoded capacity of a legacy advertising PDU header, AdvA and data.
pub const LEGACY_ADVERTISING_PDU_CAPACITY: usize = 39;

const ADVERTISING_HEADER_LENGTH: usize = 2;
const DEVICE_ADDRESS_LENGTH: usize = 6;
const ADV_NONCONN_IND_TYPE: u8 = 0b0010;
const TX_ADD_RANDOM: u8 = 1 << 6;
const ADV_NONCONN_IND_RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 5) | (1 << 7);

/// Borrowed or internally owned legacy advertising data with a checked limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingData<'a> {
    storage: LegacyAdvertisingDataStorage<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyAdvertisingDataStorage<'a> {
    Borrowed(&'a [u8]),
    Owned {
        bytes: [u8; LEGACY_ADVERTISING_DATA_CAPACITY],
        length: u8,
    },
}

impl<'a> LegacyAdvertisingData<'a> {
    /// Validate one caller-owned advertising-data value without copying it.
    pub const fn new(bytes: &'a [u8]) -> Result<Self, LegacyAdvertisingDataError> {
        if bytes.len() > LEGACY_ADVERTISING_DATA_CAPACITY {
            return Err(LegacyAdvertisingDataError::TooLong {
                length: bytes.len(),
            });
        }
        Ok(Self {
            storage: LegacyAdvertisingDataStorage::Borrowed(bytes),
        })
    }

    /// Borrow the validated bytes.
    pub const fn as_bytes(&self) -> &[u8] {
        match &self.storage {
            LegacyAdvertisingDataStorage::Borrowed(bytes) => bytes,
            LegacyAdvertisingDataStorage::Owned { bytes, length } => {
                bytes.split_at(*length as usize).0
            }
        }
    }

    /// Number of advertising-data octets.
    pub const fn len(self) -> usize {
        match self.storage {
            LegacyAdvertisingDataStorage::Borrowed(bytes) => bytes.len(),
            LegacyAdvertisingDataStorage::Owned { length, .. } => length as usize,
        }
    }

    /// Whether the advertising-data field is empty.
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }
}

impl LegacyAdvertisingData<'static> {
    /// Copy one ephemeral Host value into a self-contained async-safe owner.
    pub const fn new_owned(bytes: &[u8]) -> Result<Self, LegacyAdvertisingDataError> {
        if bytes.len() > LEGACY_ADVERTISING_DATA_CAPACITY {
            return Err(LegacyAdvertisingDataError::TooLong {
                length: bytes.len(),
            });
        }
        let mut owned = [0; LEGACY_ADVERTISING_DATA_CAPACITY];
        let mut index = 0;
        while index < bytes.len() {
            owned[index] = bytes[index];
            index += 1;
        }
        Ok(Self {
            storage: LegacyAdvertisingDataStorage::Owned {
                bytes: owned,
                length: bytes.len() as u8,
            },
        })
    }
}

/// Invalid legacy advertising data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingDataError {
    /// More than 31 octets were supplied.
    TooLong { length: usize },
}

/// Semantic `ADV_NONCONN_IND` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyNonconnectableAdvertisement<'a> {
    advertiser: LeDeviceAddress,
    data: LegacyAdvertisingData<'a>,
}

impl<'a> LegacyNonconnectableAdvertisement<'a> {
    /// Construct a non-connectable, non-scannable undirected advertisement.
    pub const fn new(advertiser: LeDeviceAddress, data: LegacyAdvertisingData<'a>) -> Self {
        Self { advertiser, data }
    }

    /// Advertiser address and TxAdd class.
    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    /// Validated Host advertising data.
    pub const fn data(self) -> LegacyAdvertisingData<'a> {
        self.data
    }

    /// Required encoded PDU length.
    pub const fn encoded_len(self) -> usize {
        ADVERTISING_HEADER_LENGTH + DEVICE_ADDRESS_LENGTH + self.data.len()
    }

    /// Encode the complete Link Layer PDU into caller-owned bounded storage.
    ///
    /// Preamble, advertising Access Address, CRC and whitening are deliberately
    /// outside this codec: the chip backend must state which of those operations
    /// are performed by hardware.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let required = self.encoded_len();
        if destination.len() < required {
            return Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required,
                available: destination.len(),
            });
        }

        let tx_add = match self.advertiser.kind() {
            LeDeviceAddressKind::Public => 0,
            LeDeviceAddressKind::Random => TX_ADD_RANDOM,
        };
        destination[0] = ADV_NONCONN_IND_TYPE | tx_add;
        destination[1] = (DEVICE_ADDRESS_LENGTH + self.data.len()) as u8;
        destination[2..8].copy_from_slice(&self.advertiser.wire_bytes());
        destination[8..required].copy_from_slice(self.data.as_bytes());
        Ok(required)
    }

    /// Decode one exact `ADV_NONCONN_IND` PDU.
    pub fn decode(source: &'a [u8]) -> Result<Self, LegacyAdvertisingDecodeError> {
        if source.len() < ADVERTISING_HEADER_LENGTH {
            return Err(LegacyAdvertisingDecodeError::TruncatedHeader {
                available: source.len(),
            });
        }

        let header = source[0];
        let pdu_type = header & 0x0f;
        if pdu_type != ADV_NONCONN_IND_TYPE {
            return Err(LegacyAdvertisingDecodeError::UnexpectedPduType { pdu_type });
        }
        if header & ADV_NONCONN_IND_RESERVED_HEADER_BITS != 0 {
            return Err(LegacyAdvertisingDecodeError::ReservedHeaderBitsSet);
        }

        let payload_length = source[1] as usize;
        if !(DEVICE_ADDRESS_LENGTH..=DEVICE_ADDRESS_LENGTH + LEGACY_ADVERTISING_DATA_CAPACITY)
            .contains(&payload_length)
        {
            return Err(LegacyAdvertisingDecodeError::InvalidPayloadLength {
                length: payload_length,
            });
        }
        let required = ADVERTISING_HEADER_LENGTH + payload_length;
        if source.len() != required {
            return Err(LegacyAdvertisingDecodeError::LengthMismatch {
                declared: required,
                available: source.len(),
            });
        }

        let mut wire_bytes = [0; DEVICE_ADDRESS_LENGTH];
        wire_bytes.copy_from_slice(&source[2..8]);
        let kind = if header & TX_ADD_RANDOM == 0 {
            LeDeviceAddressKind::Public
        } else {
            LeDeviceAddressKind::Random
        };
        let data = LegacyAdvertisingData::new(&source[8..required])
            .expect("the checked legacy payload length bounds its advertising data");
        Ok(Self::new(
            LeDeviceAddress::from_wire_bytes(wire_bytes, kind),
            data,
        ))
    }
}

/// Failed bounded PDU encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingEncodeError {
    /// Caller storage cannot retain the complete PDU.
    DestinationTooSmall { required: usize, available: usize },
}

/// Malformed or unsupported advertising PDU input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingDecodeError {
    /// The two-octet advertising header is incomplete.
    TruncatedHeader { available: usize },
    /// The PDU is not an `ADV_NONCONN_IND`.
    UnexpectedPduType { pdu_type: u8 },
    /// ChSel, RxAdd or another reserved header bit was nonzero.
    ReservedHeaderBitsSet,
    /// The payload cannot contain exactly AdvA plus at most 31 data octets.
    InvalidPayloadLength { length: usize },
    /// The input does not end at the declared payload boundary.
    LengthMismatch { declared: usize, available: usize },
}

/// One primary advertising channel index.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum PrimaryAdvertisingChannel {
    Channel37 = 37,
    Channel38 = 38,
    Channel39 = 39,
}

impl PrimaryAdvertisingChannel {
    /// Bluetooth channel index.
    pub const fn index(self) -> u8 {
        self as u8
    }

    const fn map_bit(self) -> u8 {
        match self {
            Self::Channel37 => 1,
            Self::Channel38 => 2,
            Self::Channel39 => 4,
        }
    }
}

/// Non-empty selection of primary advertising channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimaryAdvertisingChannelMap {
    selected: u8,
}

impl PrimaryAdvertisingChannelMap {
    /// Select a non-empty subset of channels 37, 38 and 39.
    pub const fn new(
        channel_37: bool,
        channel_38: bool,
        channel_39: bool,
    ) -> Result<Self, PrimaryAdvertisingChannelMapError> {
        let selected = channel_37 as u8 | (channel_38 as u8) << 1 | (channel_39 as u8) << 2;
        if selected == 0 {
            return Err(PrimaryAdvertisingChannelMapError::Empty);
        }
        Ok(Self { selected })
    }

    /// Select all three primary advertising channels.
    pub const fn all() -> Self {
        Self { selected: 0b111 }
    }

    /// Whether the map includes one channel.
    pub const fn contains(self, channel: PrimaryAdvertisingChannel) -> bool {
        self.selected & channel.map_bit() != 0
    }

    /// Number of selected primary channels in the event.
    pub const fn channel_count(self) -> usize {
        self.selected.count_ones() as usize
    }

    /// The selected channel at one canonical event position.
    pub const fn channel(self, position: usize) -> Option<PrimaryAdvertisingChannel> {
        let mut current = 0;
        let channels = [
            PrimaryAdvertisingChannel::Channel37,
            PrimaryAdvertisingChannel::Channel38,
            PrimaryAdvertisingChannel::Channel39,
        ];
        let mut index = 0;
        while index < channels.len() {
            let channel = channels[index];
            if self.contains(channel) {
                if current == position {
                    return Some(channel);
                }
                current += 1;
            }
            index += 1;
        }
        None
    }
}

/// Invalid primary advertising channel map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryAdvertisingChannelMapError {
    Empty,
}

/// Advertising interval in 0.625 ms Link Layer units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingInterval {
    units_625_us: u32,
}

impl AdvertisingInterval {
    /// Minimum general undirected advertising interval: 20 ms.
    pub const MIN_UNITS: u32 = 32;
    /// Maximum Link Layer advertising interval in 0.625 ms units.
    pub const MAX_UNITS: u32 = 0x00ff_ffff;

    /// Validate the general Link Layer advertising-interval domain.
    ///
    /// A legacy HCI command adapter may impose its smaller 16-bit range before
    /// constructing this portable value.
    pub const fn new(units_625_us: u32) -> Result<Self, AdvertisingIntervalError> {
        if units_625_us < Self::MIN_UNITS || units_625_us > Self::MAX_UNITS {
            return Err(AdvertisingIntervalError::OutsideLinkLayerRange { units_625_us });
        }
        Ok(Self { units_625_us })
    }

    /// Encoded Link Layer interval units.
    pub const fn units_625_us(self) -> u32 {
        self.units_625_us
    }

    /// Interval duration in microseconds.
    pub const fn as_micros(self) -> u64 {
        self.units_625_us as u64 * 625
    }
}

/// Invalid advertising interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvertisingIntervalError {
    OutsideLinkLayerRange { units_625_us: u32 },
}

/// Per-event pseudo-random advertising delay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingDelay {
    micros: u16,
}

impl AdvertisingDelay {
    /// Largest delay allowed by the Link Layer: 10 ms.
    pub const MAX_MICROS: u16 = 10_000;

    /// Validate a delay generated by a higher source-owned random provider.
    pub const fn from_micros(micros: u16) -> Result<Self, AdvertisingDelayError> {
        if micros > Self::MAX_MICROS {
            return Err(AdvertisingDelayError::OutsideLinkLayerRange { micros });
        }
        Ok(Self { micros })
    }

    /// Delay in microseconds.
    pub const fn as_micros(self) -> u16 {
        self.micros
    }
}

/// Invalid advertising delay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvertisingDelayError {
    OutsideLinkLayerRange { micros: u16 },
}

/// Reusable configuration of one non-connectable legacy advertising set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyNonconnectableAdvertisingSet<'a> {
    advertisement: LegacyNonconnectableAdvertisement<'a>,
    channels: PrimaryAdvertisingChannelMap,
    interval: AdvertisingInterval,
}

impl<'a> LegacyNonconnectableAdvertisingSet<'a> {
    pub const fn new(
        advertisement: LegacyNonconnectableAdvertisement<'a>,
        channels: PrimaryAdvertisingChannelMap,
        interval: AdvertisingInterval,
    ) -> Self {
        Self {
            advertisement,
            channels,
            interval,
        }
    }

    /// Begin one event with every configured channel still pending.
    pub const fn begin_event(self) -> LegacyNonconnectableAdvertisingEvent<'a> {
        LegacyNonconnectableAdvertisingEvent { set: self }
    }
}

/// Affine event which has not reached a hardware backend.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepare, cancel, or retain the advertising event"]
pub struct LegacyNonconnectableAdvertisingEvent<'a> {
    set: LegacyNonconnectableAdvertisingSet<'a>,
}

impl<'a> LegacyNonconnectableAdvertisingEvent<'a> {
    /// Prepare the complete ordered primary-channel event.
    pub fn prepare(self) -> LegacyPreparedAdvertisingEvent<'a> {
        LegacyPreparedAdvertisingEvent { event: self }
    }

    /// Cancel an event which has no in-flight transmission and retain its set.
    pub fn into_set(self) -> LegacyNonconnectableAdvertisingSet<'a> {
        self.set
    }
}

/// One prepared PDU and its complete selected primary-channel plan.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "publish the event or cancel back to the unchanged event"]
pub struct LegacyPreparedAdvertisingEvent<'a> {
    event: LegacyNonconnectableAdvertisingEvent<'a>,
}

impl<'a> LegacyPreparedAdvertisingEvent<'a> {
    /// Complete non-empty channel selection in canonical 37, 38, 39 order.
    pub const fn channels(&self) -> PrimaryAdvertisingChannelMap {
        self.event.set.channels
    }

    /// Encode the prepared PDU without advancing the event.
    pub fn encode(&self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        self.event.set.advertisement.encode(destination)
    }

    /// Return the unchanged event after publication was cancelled or rejected.
    pub fn cancel(self) -> LegacyNonconnectableAdvertisingEvent<'a> {
        self.event
    }

    /// Record completion of the complete selected-channel backend event.
    ///
    /// Completion means that the backend consumed every scheduled item in the
    /// event. It does not by itself claim that any packet reached the air;
    /// chip-specific diagnostic status remains below this protocol boundary.
    pub fn into_event_completed(self) -> LegacyAdvertisingEventComplete<'a> {
        LegacyAdvertisingEventComplete {
            set: self.event.set,
        }
    }
}

/// Completed event retaining the reusable advertising-set configuration.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "schedule the next event or retain the advertising set"]
pub struct LegacyAdvertisingEventComplete<'a> {
    set: LegacyNonconnectableAdvertisingSet<'a>,
}

impl<'a> LegacyAdvertisingEventComplete<'a> {
    /// Retain the configured set without scheduling another event.
    pub fn into_set(self) -> LegacyNonconnectableAdvertisingSet<'a> {
        self.set
    }

    /// Schedule the next event from the previous event's start.
    ///
    /// Random generation is deliberately not hidden here: the LL runtime must
    /// supply a fresh, source-owned value in the required 0..=10 ms domain.
    pub const fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> ScheduledLegacyAdvertisingEvent<'a> {
        ScheduledLegacyAdvertisingEvent {
            start_offset_micros: self.set.interval.as_micros() + delay.as_micros() as u64,
            event: self.set.begin_event(),
        }
    }
}

/// Next event paired with its relative start deadline.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "wait until the deadline and retain the event"]
pub struct ScheduledLegacyAdvertisingEvent<'a> {
    start_offset_micros: u64,
    event: LegacyNonconnectableAdvertisingEvent<'a>,
}

impl<'a> ScheduledLegacyAdvertisingEvent<'a> {
    /// Offset from the previous advertising-event start.
    pub const fn start_offset_micros(&self) -> u64 {
        self.start_offset_micros
    }

    /// Consume the schedule after its deadline and recover the next event.
    pub fn into_event(self) -> LegacyNonconnectableAdvertisingEvent<'a> {
        self.event
    }

    /// Cancel the not-yet-due event and retain its configured set.
    pub fn cancel(self) -> LegacyNonconnectableAdvertisingSet<'a> {
        self.event.into_set()
    }
}

#[cfg(test)]
mod tests;
