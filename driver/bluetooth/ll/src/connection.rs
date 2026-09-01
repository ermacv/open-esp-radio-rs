//! Portable LE peripheral-connection admission and event-channel planning.
//!
//! This module owns Bluetooth air-interface semantics only. It parses one
//! legacy `CONNECT_IND`, validates the complete first-event timing profile and
//! advances Channel Selection Algorithm #1 or #2 without MMIO, controller SRAM,
//! HCI, an executor or an allocator. A chip backend must retain the affine
//! event owner with its own hardware ticket before reporting completion.

use crate::{LeDeviceAddress, LeDeviceAddressKind};

pub const LEGACY_CONNECT_IND_PDU_BYTES: usize = 36;
pub const LEGACY_CONNECT_IND_PAYLOAD_BYTES: usize = 34;

const CONNECT_IND_TYPE: u8 = 0b0101;
const PDU_TYPE_MASK: u8 = 0x0f;
const HEADER_RESERVED: u8 = 1 << 4;
const CHANNEL_SELECTION_TWO: u8 = 1 << 5;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RX_ADD_RANDOM: u8 = 1 << 7;
const PAYLOAD_LENGTH_MASK: u8 = 0x3f;
const PAYLOAD_LENGTH_RESERVED: u8 = 0xc0;
const ADVERTISING_ACCESS_ADDRESS: u32 = 0x8e89_bed6;

/// Intrinsically valid Access Address for one LE ACL connection on an uncoded PHY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeUncodedAccessAddress(u32);

impl LeUncodedAccessAddress {
    /// Validate the bit-pattern requirements that do not depend on other live
    /// connections. Cross-connection uniqueness remains a Controller policy.
    pub const fn new(value: u32) -> Result<Self, LeUncodedAccessAddressError> {
        if (value ^ ADVERTISING_ACCESS_ADDRESS).count_ones() <= 1 {
            return Err(LeUncodedAccessAddressError::AdvertisingAddressOrOneBitAway);
        }
        let bytes = value.to_le_bytes();
        if bytes[0] == bytes[1] && bytes[1] == bytes[2] && bytes[2] == bytes[3] {
            return Err(LeUncodedAccessAddressError::AllOctetsEqual);
        }
        if longest_run(value) > 6 {
            return Err(LeUncodedAccessAddressError::RunTooLong);
        }
        if transition_count(value) > 24 {
            return Err(LeUncodedAccessAddressError::TooManyTransitions);
        }
        if most_significant_six_transition_count(value) < 2 {
            return Err(LeUncodedAccessAddressError::TooFewMostSignificantTransitions);
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// Fixed channel identifier used by Channel Selection Algorithm #2.
    pub const fn channel_identifier(self) -> u16 {
        (self.0 >> 16) as u16 ^ self.0 as u16
    }
}

const fn longest_run(value: u32) -> u8 {
    let mut longest = 1;
    let mut current = 1;
    let mut previous = value & 1;
    let mut bit = 1;
    while bit < 32 {
        let next = (value >> bit) & 1;
        if next == previous {
            current += 1;
            if current > longest {
                longest = current;
            }
        } else {
            current = 1;
            previous = next;
        }
        bit += 1;
    }
    longest
}

const fn transition_count(value: u32) -> u32 {
    ((value ^ (value >> 1)) & 0x7fff_ffff).count_ones()
}

const fn most_significant_six_transition_count(value: u32) -> u8 {
    let six = value >> 26;
    (((six ^ (six >> 1)) & 0x1f).count_ones()) as u8
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeUncodedAccessAddressError {
    AdvertisingAddressOrOneBitAway,
    AllOctetsEqual,
    RunTooLong,
    TooManyTransitions,
    TooFewMostSignificantTransitions,
}

/// Three-octet CRC initialization value carried by `CONNECT_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeCrcInitialization([u8; 3]);

impl LeCrcInitialization {
    pub const fn from_wire_bytes(bytes: [u8; 3]) -> Self {
        Self(bytes)
    }

    pub const fn wire_bytes(self) -> [u8; 3] {
        self.0
    }

    pub const fn value(self) -> u32 {
        self.0[0] as u32 | (self.0[1] as u32) << 8 | (self.0[2] as u32) << 16
    }
}

/// One of the 37 general-purpose LE data channel indices.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LeDataChannelIndex(u8);

impl LeDataChannelIndex {
    pub const fn new(index: u8) -> Option<Self> {
        if index < 37 { Some(Self(index)) } else { None }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Validated map containing at least two used data channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataChannelMap {
    bytes: [u8; 5],
    used: u8,
}

impl LeDataChannelMap {
    pub const fn new(bytes: [u8; 5]) -> Result<Self, LeDataChannelMapError> {
        if bytes[4] & 0xe0 != 0 {
            return Err(LeDataChannelMapError::ReservedBitsSet);
        }
        let used = bytes[0].count_ones()
            + bytes[1].count_ones()
            + bytes[2].count_ones()
            + bytes[3].count_ones()
            + bytes[4].count_ones();
        if used < 2 {
            return Err(LeDataChannelMapError::FewerThanTwoUsedChannels);
        }
        Ok(Self {
            bytes,
            used: used as u8,
        })
    }

    pub const fn all() -> Self {
        Self {
            bytes: [0xff, 0xff, 0xff, 0xff, 0x1f],
            used: 37,
        }
    }

    pub const fn wire_bytes(self) -> [u8; 5] {
        self.bytes
    }

    pub const fn used_channel_count(self) -> u8 {
        self.used
    }

    pub const fn contains(self, channel: LeDataChannelIndex) -> bool {
        let index = channel.get() as usize;
        self.bytes[index / 8] & (1 << (index % 8)) != 0
    }

    const fn remap(self, index: u8) -> LeDataChannelIndex {
        let mut remaining = index;
        let mut channel = 0;
        while channel < 37 {
            let candidate = LeDataChannelIndex(channel);
            if self.contains(candidate) {
                if remaining == 0 {
                    return candidate;
                }
                remaining -= 1;
            }
            channel += 1;
        }
        unreachable!()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDataChannelMapError {
    ReservedBitsSet,
    FewerThanTwoUsedChannels,
}

/// Channel selection negotiated by the advertising header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeChannelSelectionAlgorithm {
    AlgorithmOne,
    AlgorithmTwo,
}

/// Worst-case sleep-clock accuracy class supplied by the Central.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeSleepClockAccuracy(u8);

impl LeSleepClockAccuracy {
    pub const fn encoded(self) -> u8 {
        self.0
    }
}

/// Validated first transmit window and recurring connection timing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionTiming {
    window_size: u8,
    window_offset: u16,
    interval: u16,
    peripheral_latency: u16,
    supervision_timeout: u16,
}

impl LeConnectionTiming {
    pub const fn new(
        window_size: u8,
        window_offset: u16,
        interval: u16,
        peripheral_latency: u16,
        supervision_timeout: u16,
    ) -> Result<Self, LeConnectionTimingError> {
        if interval < 6 || interval > 3200 {
            return Err(LeConnectionTimingError::IntervalOutsideRange);
        }
        if window_size == 0 || window_size > 8 || window_size as u16 >= interval {
            return Err(LeConnectionTimingError::WindowSizeOutsideRange);
        }
        if window_offset > interval {
            return Err(LeConnectionTimingError::WindowOffsetOutsideRange);
        }
        if peripheral_latency > 499 {
            return Err(LeConnectionTimingError::PeripheralLatencyOutsideRange);
        }
        if supervision_timeout < 10 || supervision_timeout > 3200 {
            return Err(LeConnectionTimingError::SupervisionTimeoutOutsideRange);
        }
        if supervision_timeout as u32 * 4 <= interval as u32 * (peripheral_latency as u32 + 1) {
            return Err(LeConnectionTimingError::SupervisionTimeoutTooShort);
        }
        Ok(Self {
            window_size,
            window_offset,
            interval,
            peripheral_latency,
            supervision_timeout,
        })
    }

    pub const fn window_size_units(self) -> u8 {
        self.window_size
    }

    pub const fn window_offset_units(self) -> u16 {
        self.window_offset
    }

    pub const fn interval_units(self) -> u16 {
        self.interval
    }

    pub const fn interval_micros(self) -> u32 {
        self.interval as u32 * 1_250
    }

    pub const fn peripheral_latency(self) -> u16 {
        self.peripheral_latency
    }

    pub const fn supervision_timeout_units(self) -> u16 {
        self.supervision_timeout
    }

    pub const fn supervision_timeout_micros(self) -> u32 {
        self.supervision_timeout as u32 * 10_000
    }

    /// Earliest first anchor relative to the end of the `CONNECT_IND` packet.
    pub const fn first_window_start_micros(self) -> u32 {
        1_250 + self.window_offset as u32 * 1_250
    }

    /// Exclusive upper edge of the first transmit window relative to the end
    /// of the `CONNECT_IND` packet.
    pub const fn first_window_end_micros(self) -> u32 {
        self.first_window_start_micros() + self.window_size as u32 * 1_250
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeConnectionTimingError {
    IntervalOutsideRange,
    WindowSizeOutsideRange,
    WindowOffsetOutsideRange,
    PeripheralLatencyOutsideRange,
    SupervisionTimeoutOutsideRange,
    SupervisionTimeoutTooShort,
}

/// Complete semantic value carried by one legacy `CONNECT_IND` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyConnectionRequest {
    initiator: LeDeviceAddress,
    advertiser: LeDeviceAddress,
    access_address: LeUncodedAccessAddress,
    crc_initialization: LeCrcInitialization,
    timing: LeConnectionTiming,
    channel_map: LeDataChannelMap,
    hop_increment: u8,
    sleep_clock_accuracy: LeSleepClockAccuracy,
    channel_selection: LeChannelSelectionAlgorithm,
}

impl LeLegacyConnectionRequest {
    /// Decode and validate exactly one complete legacy `CONNECT_IND` PDU.
    pub const fn decode(pdu: &[u8]) -> Result<Self, LeLegacyConnectionRequestError> {
        if pdu.len() < 2 {
            return Err(LeLegacyConnectionRequestError::HeaderTruncated);
        }
        if pdu[0] & PDU_TYPE_MASK != CONNECT_IND_TYPE {
            return Err(LeLegacyConnectionRequestError::UnexpectedPduType);
        }
        if pdu[0] & HEADER_RESERVED != 0 || pdu[1] & PAYLOAD_LENGTH_RESERVED != 0 {
            return Err(LeLegacyConnectionRequestError::ReservedHeaderBitsSet);
        }
        let declared = (pdu[1] & PAYLOAD_LENGTH_MASK) as usize;
        if declared != LEGACY_CONNECT_IND_PAYLOAD_BYTES || pdu.len() != LEGACY_CONNECT_IND_PDU_BYTES
        {
            return Err(LeLegacyConnectionRequestError::LengthMismatch);
        }

        let initiator = LeDeviceAddress::from_wire_bytes(
            copy_six(pdu, 2),
            if pdu[0] & TX_ADD_RANDOM == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        );
        let advertiser = LeDeviceAddress::from_wire_bytes(
            copy_six(pdu, 8),
            if pdu[0] & RX_ADD_RANDOM == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        );
        let access_address =
            match LeUncodedAccessAddress::new(u32::from_le_bytes(copy_four(pdu, 14))) {
                Ok(value) => value,
                Err(error) => return Err(LeLegacyConnectionRequestError::AccessAddress(error)),
            };
        let crc_initialization = LeCrcInitialization::from_wire_bytes([pdu[18], pdu[19], pdu[20]]);
        let timing = match LeConnectionTiming::new(
            pdu[21],
            u16::from_le_bytes([pdu[22], pdu[23]]),
            u16::from_le_bytes([pdu[24], pdu[25]]),
            u16::from_le_bytes([pdu[26], pdu[27]]),
            u16::from_le_bytes([pdu[28], pdu[29]]),
        ) {
            Ok(value) => value,
            Err(error) => return Err(LeLegacyConnectionRequestError::Timing(error)),
        };
        let channel_map = match LeDataChannelMap::new([pdu[30], pdu[31], pdu[32], pdu[33], pdu[34]])
        {
            Ok(value) => value,
            Err(error) => return Err(LeLegacyConnectionRequestError::ChannelMap(error)),
        };
        let hop_increment = pdu[35] & 0x1f;
        if hop_increment < 5 || hop_increment > 16 {
            return Err(LeLegacyConnectionRequestError::HopIncrementOutsideRange);
        }
        Ok(Self {
            initiator,
            advertiser,
            access_address,
            crc_initialization,
            timing,
            channel_map,
            hop_increment,
            sleep_clock_accuracy: LeSleepClockAccuracy(pdu[35] >> 5),
            channel_selection: if pdu[0] & CHANNEL_SELECTION_TWO == 0 {
                LeChannelSelectionAlgorithm::AlgorithmOne
            } else {
                LeChannelSelectionAlgorithm::AlgorithmTwo
            },
        })
    }

    pub const fn initiator(self) -> LeDeviceAddress {
        self.initiator
    }

    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    pub fn is_addressed_to(self, address: LeDeviceAddress) -> bool {
        self.advertiser == address
    }

    pub const fn access_address(self) -> LeUncodedAccessAddress {
        self.access_address
    }

    pub const fn crc_initialization(self) -> LeCrcInitialization {
        self.crc_initialization
    }

    pub const fn timing(self) -> LeConnectionTiming {
        self.timing
    }

    pub const fn channel_map(self) -> LeDataChannelMap {
        self.channel_map
    }

    pub const fn hop_increment(self) -> u8 {
        self.hop_increment
    }

    pub const fn sleep_clock_accuracy(self) -> LeSleepClockAccuracy {
        self.sleep_clock_accuracy
    }

    pub const fn channel_selection(self) -> LeChannelSelectionAlgorithm {
        self.channel_selection
    }
}

const fn copy_four(source: &[u8], start: usize) -> [u8; 4] {
    [
        source[start],
        source[start + 1],
        source[start + 2],
        source[start + 3],
    ]
}

const fn copy_six(source: &[u8], start: usize) -> [u8; 6] {
    [
        source[start],
        source[start + 1],
        source[start + 2],
        source[start + 3],
        source[start + 4],
        source[start + 5],
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyConnectionRequestError {
    HeaderTruncated,
    UnexpectedPduType,
    ReservedHeaderBitsSet,
    LengthMismatch,
    AccessAddress(LeUncodedAccessAddressError),
    Timing(LeConnectionTimingError),
    ChannelMap(LeDataChannelMapError),
    HopIncrementOutsideRange,
}

/// Pure Channel Selection Algorithm #2 event selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeChannelSelectionAlgorithmTwo {
    channel_identifier: u16,
    channel_map: LeDataChannelMap,
}

impl LeChannelSelectionAlgorithmTwo {
    pub const fn new(
        access_address: LeUncodedAccessAddress,
        channel_map: LeDataChannelMap,
    ) -> Self {
        Self {
            channel_identifier: access_address.channel_identifier(),
            channel_map,
        }
    }

    pub const fn select(self, event_counter: u16) -> LeDataChannelIndex {
        let mut pseudo_random = event_counter ^ self.channel_identifier;
        let mut round = 0;
        while round < 3 {
            pseudo_random = permute(pseudo_random);
            pseudo_random = pseudo_random
                .wrapping_mul(17)
                .wrapping_add(self.channel_identifier);
            round += 1;
        }
        pseudo_random ^= self.channel_identifier;
        let unmapped = LeDataChannelIndex((pseudo_random % 37) as u8);
        if self.channel_map.contains(unmapped) {
            unmapped
        } else {
            let remapping_index =
                ((self.channel_map.used_channel_count() as u32 * pseudo_random as u32) >> 16) as u8;
            self.channel_map.remap(remapping_index)
        }
    }
}

const fn permute(value: u16) -> u16 {
    let bytes = value.to_le_bytes();
    u16::from_le_bytes([bytes[0].reverse_bits(), bytes[1].reverse_bits()])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionChannelSelector {
    One {
        channel_map: LeDataChannelMap,
        hop_increment: u8,
        last_unmapped: u8,
    },
    Two(LeChannelSelectionAlgorithmTwo),
}

impl ConnectionChannelSelector {
    const fn preview(self, event_counter: u16) -> (LeDataChannelIndex, u8) {
        match self {
            Self::One {
                channel_map,
                hop_increment,
                last_unmapped,
            } => {
                let unmapped = (last_unmapped + hop_increment) % 37;
                let candidate = LeDataChannelIndex(unmapped);
                let selected = if channel_map.contains(candidate) {
                    candidate
                } else {
                    channel_map.remap(unmapped % channel_map.used_channel_count())
                };
                (selected, unmapped)
            }
            Self::Two(selector) => (selector.select(event_counter), 0),
        }
    }

    const fn complete(self, unmapped: u8) -> Self {
        match self {
            Self::One {
                channel_map,
                hop_increment,
                ..
            } => Self::One {
                channel_map,
                hop_increment,
                last_unmapped: unmapped,
            },
            Self::Two(selector) => Self::Two(selector),
        }
    }
}

/// Portable peripheral connection between hardware events.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepare the next event or retain the connection"]
pub struct LePeripheralConnection {
    request: LeLegacyConnectionRequest,
    event_counter: u16,
    first_event: bool,
    selector: ConnectionChannelSelector,
}

impl LePeripheralConnection {
    pub const fn from_request(request: LeLegacyConnectionRequest) -> Self {
        let selector = match request.channel_selection {
            LeChannelSelectionAlgorithm::AlgorithmOne => ConnectionChannelSelector::One {
                channel_map: request.channel_map,
                hop_increment: request.hop_increment,
                last_unmapped: 0,
            },
            LeChannelSelectionAlgorithm::AlgorithmTwo => ConnectionChannelSelector::Two(
                LeChannelSelectionAlgorithmTwo::new(request.access_address, request.channel_map),
            ),
        };
        Self {
            request,
            event_counter: 0,
            first_event: true,
            selector,
        }
    }

    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.request
    }

    pub const fn event_counter(&self) -> u16 {
        self.event_counter
    }

    /// Prepare one exact event without advancing channel or counter state.
    pub const fn prepare_event(self) -> LePeripheralConnectionEventPrepared {
        let (channel, unmapped) = self.selector.preview(self.event_counter);
        LePeripheralConnectionEventPrepared {
            connection: self,
            channel,
            unmapped,
        }
    }
}

/// Protocol event not yet accepted by a hardware backend.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "submit, cancel, or retain the prepared connection event"]
pub struct LePeripheralConnectionEventPrepared {
    connection: LePeripheralConnection,
    channel: LeDataChannelIndex,
    unmapped: u8,
}

impl LePeripheralConnectionEventPrepared {
    pub const fn event_counter(&self) -> u16 {
        self.connection.event_counter
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.channel
    }

    pub const fn timing(&self) -> LeConnectionTiming {
        self.connection.request.timing
    }

    pub const fn first_transmit_window_micros(&self) -> Option<(u32, u32)> {
        if self.connection.first_event {
            Some((
                self.timing().first_window_start_micros(),
                self.timing().first_window_end_micros(),
            ))
        } else {
            None
        }
    }

    /// Reject lower admission without advancing protocol time or hopping state.
    pub const fn cancel(self) -> LePeripheralConnection {
        self.connection
    }

    /// Mark the point where a lower backend accepted this exact event.
    pub const fn into_submitted(self) -> LePeripheralConnectionEventInFlight {
        LePeripheralConnectionEventInFlight { prepared: self }
    }
}

/// Portable continuation retained alongside one lower hardware transaction.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "complete the exact backend event or retain its connection owner"]
pub struct LePeripheralConnectionEventInFlight {
    prepared: LePeripheralConnectionEventPrepared,
}

impl LePeripheralConnectionEventInFlight {
    pub const fn event_counter(&self) -> u16 {
        self.prepared.event_counter()
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.prepared.channel()
    }

    /// Advance only after the paired lower owner proves the event closed.
    pub const fn complete(self) -> LePeripheralConnection {
        let LePeripheralConnectionEventPrepared {
            mut connection,
            unmapped,
            ..
        } = self.prepared;
        connection.selector = connection.selector.complete(unmapped);
        connection.event_counter = connection.event_counter.wrapping_add(1);
        connection.first_event = false;
        connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INITIATOR: [u8; 6] = [1, 2, 3, 4, 5, 6];
    const ADVERTISER: [u8; 6] = [7, 8, 9, 10, 11, 12];

    fn connection_request(channel_selection_two: bool) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = CONNECT_IND_TYPE
            | if channel_selection_two {
                CHANNEL_SELECTION_TWO
            } else {
                0
            };
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&INITIATOR);
        pdu[8..14].copy_from_slice(&ADVERTISER);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
        pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&LeDataChannelMap::all().wire_bytes());
        pdu[35] = 5 | (4 << 5);
        pdu
    }

    #[test]
    fn complete_connect_ind_becomes_semantic_peripheral_input() {
        let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();

        assert_eq!(request.initiator().wire_bytes(), INITIATOR);
        assert_eq!(request.advertiser().wire_bytes(), ADVERTISER);
        assert_eq!(request.access_address().value(), 0xa1b2_c3d4);
        assert_eq!(request.crc_initialization().value(), 0x11_2233);
        assert_eq!(request.timing().interval_micros(), 30_000);
        assert_eq!(request.timing().first_window_start_micros(), 2_500);
        assert_eq!(request.timing().first_window_end_micros(), 5_000);
        assert_eq!(request.channel_map().used_channel_count(), 37);
        assert_eq!(request.hop_increment(), 5);
        assert_eq!(request.sleep_clock_accuracy().encoded(), 4);
        assert_eq!(
            request.channel_selection(),
            LeChannelSelectionAlgorithm::AlgorithmTwo
        );
    }

    #[test]
    fn malformed_request_fails_before_connection_ownership_exists() {
        let mut pdu = connection_request(false);
        pdu[0] |= HEADER_RESERVED;
        assert_eq!(
            LeLegacyConnectionRequest::decode(&pdu),
            Err(LeLegacyConnectionRequestError::ReservedHeaderBitsSet)
        );

        let mut pdu = connection_request(false);
        pdu[21] = 0;
        assert_eq!(
            LeLegacyConnectionRequest::decode(&pdu),
            Err(LeLegacyConnectionRequestError::Timing(
                LeConnectionTimingError::WindowSizeOutsideRange
            ))
        );

        let mut pdu = connection_request(false);
        pdu[30..35].copy_from_slice(&[1, 0, 0, 0, 0]);
        assert_eq!(
            LeLegacyConnectionRequest::decode(&pdu),
            Err(LeLegacyConnectionRequestError::ChannelMap(
                LeDataChannelMapError::FewerThanTwoUsedChannels
            ))
        );
    }

    #[test]
    fn access_address_rejects_intrinsically_ambiguous_patterns() {
        assert_eq!(
            LeUncodedAccessAddress::new(ADVERTISING_ACCESS_ADDRESS),
            Err(LeUncodedAccessAddressError::AdvertisingAddressOrOneBitAway)
        );
        assert_eq!(
            LeUncodedAccessAddress::new(0x1212_1212),
            Err(LeUncodedAccessAddressError::AllOctetsEqual)
        );
        assert!(LeUncodedAccessAddress::new(0xa1b2_c3d4).is_ok());
    }

    #[test]
    fn supervision_relation_is_strict_and_lossless() {
        assert_eq!(
            LeConnectionTiming::new(1, 0, 24, 4, 30),
            Err(LeConnectionTimingError::SupervisionTimeoutTooShort)
        );
        assert!(LeConnectionTiming::new(1, 0, 24, 4, 31).is_ok());
    }

    #[test]
    fn csa2_matches_bluetooth_sig_event_samples() {
        let selector = LeChannelSelectionAlgorithmTwo {
            channel_identifier: 0x305f,
            channel_map: LeDataChannelMap::all(),
        };
        assert_eq!(selector.select(0).get(), 25);
        assert_eq!(selector.select(1).get(), 20);
        assert_eq!(selector.select(2).get(), 6);
        assert_eq!(selector.select(3).get(), 21);

        let sparse = LeChannelSelectionAlgorithmTwo {
            channel_identifier: 0x305f,
            channel_map: LeDataChannelMap::new([0x00, 0x06, 0xe0, 0x00, 0x1e]).unwrap(),
        };
        assert_eq!(sparse.select(6).get(), 23);
        assert_eq!(sparse.select(7).get(), 9);
        assert_eq!(sparse.select(8).get(), 34);
    }

    #[test]
    fn csa1_commits_hop_only_after_exact_event_completion() {
        let mut pdu = connection_request(false);
        pdu[30..35].copy_from_slice(&[0x06, 0, 0, 0, 0]);
        let request = LeLegacyConnectionRequest::decode(&pdu).unwrap();
        let connection = LePeripheralConnection::from_request(request);

        let first = connection.prepare_event();
        assert_eq!(first.event_counter(), 0);
        assert_eq!(first.channel().get(), 2);
        assert_eq!(first.first_transmit_window_micros(), Some((2_500, 5_000)));

        let retry = first.cancel().prepare_event();
        assert_eq!(retry.event_counter(), 0);
        assert_eq!(retry.channel().get(), 2);

        let second = retry.into_submitted().complete().prepare_event();
        assert_eq!(second.event_counter(), 1);
        assert_eq!(second.channel().get(), 1);
        assert_eq!(second.first_transmit_window_micros(), None);
    }

    #[test]
    fn event_counter_wraps_without_reusing_an_in_flight_owner() {
        let request = LeLegacyConnectionRequest::decode(&connection_request(true)).unwrap();
        let mut connection = LePeripheralConnection::from_request(request);
        connection.event_counter = u16::MAX;

        let in_flight = connection.prepare_event().into_submitted();
        assert_eq!(in_flight.event_counter(), u16::MAX);
        assert_eq!(in_flight.complete().event_counter(), 0);
    }
}
