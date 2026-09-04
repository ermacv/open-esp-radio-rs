//! Portable legacy connectable advertising and `CONNECT_IND` admission.
//!
//! This module stops at the air-interface/backend boundary. It owns the
//! `ADV_IND` PDU, the selected primary-channel event and the lossless protocol
//! transition into a peripheral connection. A chip backend still owns packet
//! timing, receive filtering, scheduler publication and completion.

use crate::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        AdvertisingDelay, AdvertisingInterval, LEGACY_ADVERTISING_PDU_CAPACITY,
        LegacyAdvertisingData, LegacyAdvertisingDataError, LegacyAdvertisingEncodeError,
        PrimaryAdvertisingChannelMap,
    },
    advertising_lifecycle::{LegacyAdvertisingEventIdentity, LegacyAdvertisingGenerationAllocator},
    connection::{
        LeChannelSelectionAlgorithm, LeLegacyConnectionRequest, LeLegacyConnectionRequestError,
        LePeripheralConnection,
    },
};

const ADVERTISING_HEADER_LENGTH: usize = 2;
const DEVICE_ADDRESS_LENGTH: usize = 6;
const ADV_IND_TYPE: u8 = 0;
const SCAN_RSP_TYPE: u8 = 4;
const PDU_TYPE_MASK: u8 = 0x0f;
const RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 7);
const CHANNEL_SELECTION_TWO: u8 = 1 << 5;
const TX_ADD_RANDOM: u8 = 1 << 6;

/// Whether this advertiser may negotiate Channel Selection Algorithm #2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeChannelSelectionAlgorithmTwoSupport {
    Unsupported,
    Supported,
}

/// Semantic `ADV_IND` payload and channel-selection capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisement<'a> {
    advertiser: LeDeviceAddress,
    data: LegacyAdvertisingData<'a>,
    channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
}

impl<'a> LegacyConnectableAdvertisement<'a> {
    pub const fn new(
        advertiser: LeDeviceAddress,
        data: LegacyAdvertisingData<'a>,
        channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
    ) -> Self {
        Self {
            advertiser,
            data,
            channel_selection_two,
        }
    }

    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    pub const fn data(self) -> LegacyAdvertisingData<'a> {
        self.data
    }

    pub const fn channel_selection_two(self) -> LeChannelSelectionAlgorithmTwoSupport {
        self.channel_selection_two
    }

    pub const fn encoded_len(self) -> usize {
        ADVERTISING_HEADER_LENGTH + DEVICE_ADDRESS_LENGTH + self.data.len()
    }

    /// Encode the complete Link Layer PDU into bounded caller storage.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let required = self.encoded_len();
        if destination.len() < required {
            return Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required,
                available: destination.len(),
            });
        }

        destination[0] = match self.advertiser.kind() {
            LeDeviceAddressKind::Public => 0,
            LeDeviceAddressKind::Random => TX_ADD_RANDOM,
        } | match self.channel_selection_two {
            LeChannelSelectionAlgorithmTwoSupport::Unsupported => 0,
            LeChannelSelectionAlgorithmTwoSupport::Supported => CHANNEL_SELECTION_TWO,
        };
        destination[1] = (DEVICE_ADDRESS_LENGTH + self.data.len()) as u8;
        destination[2..8].copy_from_slice(&self.advertiser.wire_bytes());
        destination[8..required].copy_from_slice(self.data.as_bytes());
        Ok(required)
    }

    /// Decode one exact legacy `ADV_IND` PDU.
    pub fn decode(source: &'a [u8]) -> Result<Self, LegacyConnectableAdvertisementDecodeError> {
        if source.len() < ADVERTISING_HEADER_LENGTH {
            return Err(LegacyConnectableAdvertisementDecodeError::TruncatedHeader {
                available: source.len(),
            });
        }

        let header = source[0];
        let pdu_type = header & PDU_TYPE_MASK;
        if pdu_type != ADV_IND_TYPE {
            return Err(LegacyConnectableAdvertisementDecodeError::UnexpectedPduType { pdu_type });
        }
        if header & RESERVED_HEADER_BITS != 0 {
            return Err(LegacyConnectableAdvertisementDecodeError::ReservedHeaderBitsSet);
        }

        let payload_length = source[1] as usize;
        if !(DEVICE_ADDRESS_LENGTH
            ..=DEVICE_ADDRESS_LENGTH + crate::advertising::LEGACY_ADVERTISING_DATA_CAPACITY)
            .contains(&payload_length)
        {
            return Err(
                LegacyConnectableAdvertisementDecodeError::InvalidPayloadLength {
                    length: payload_length,
                },
            );
        }
        let required = ADVERTISING_HEADER_LENGTH + payload_length;
        if source.len() != required {
            return Err(LegacyConnectableAdvertisementDecodeError::LengthMismatch {
                declared: required,
                available: source.len(),
            });
        }

        let mut address = [0; DEVICE_ADDRESS_LENGTH];
        address.copy_from_slice(&source[2..8]);
        let data = LegacyAdvertisingData::new(&source[8..required])
            .expect("the checked legacy payload bounds its advertising data");
        Ok(Self::new(
            LeDeviceAddress::from_wire_bytes(
                address,
                if header & TX_ADD_RANDOM == 0 {
                    LeDeviceAddressKind::Public
                } else {
                    LeDeviceAddressKind::Random
                },
            ),
            data,
            if header & CHANNEL_SELECTION_TWO == 0 {
                LeChannelSelectionAlgorithmTwoSupport::Unsupported
            } else {
                LeChannelSelectionAlgorithmTwoSupport::Supported
            },
        ))
    }
}

/// Semantic Host data carried by the matching legacy `SCAN_RSP`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanResponseData<'a>(LegacyAdvertisingData<'a>);

impl<'a> LegacyScanResponseData<'a> {
    /// Validate caller-owned scan-response data without copying it.
    pub const fn new(bytes: &'a [u8]) -> Result<Self, LegacyAdvertisingDataError> {
        match LegacyAdvertisingData::new(bytes) {
            Ok(data) => Ok(Self(data)),
            Err(error) => Err(error),
        }
    }

    /// Borrow the validated response data.
    pub const fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Number of response-data octets.
    pub const fn len(self) -> usize {
        self.0.len()
    }

    /// Whether the response carries no Host data after AdvA.
    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

impl LegacyScanResponseData<'static> {
    /// Copy one ephemeral Host response into an async-safe owner.
    pub const fn new_owned(bytes: &[u8]) -> Result<Self, LegacyAdvertisingDataError> {
        match LegacyAdvertisingData::new_owned(bytes) {
            Ok(data) => Ok(Self(data)),
            Err(error) => Err(error),
        }
    }
}

/// Complete bounded `ADV_IND` wire image produced by a prepared event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvIndPdu {
    bytes: [u8; LEGACY_ADVERTISING_PDU_CAPACITY],
    length: u8,
}

impl LegacyConnectableAdvIndPdu {
    /// Complete two-byte header and declared payload.
    pub const fn as_bytes(&self) -> &[u8] {
        self.bytes.split_at(self.length as usize).0
    }

    /// Link Layer payload length following the two-byte header.
    pub const fn payload_length(self) -> u8 {
        self.length - ADVERTISING_HEADER_LENGTH as u8
    }
}

/// Complete bounded `SCAN_RSP` wire image paired with one prepared event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanResponsePdu {
    bytes: [u8; LEGACY_ADVERTISING_PDU_CAPACITY],
    length: u8,
}

impl LegacyScanResponsePdu {
    /// Complete two-byte header and declared payload.
    pub const fn as_bytes(&self) -> &[u8] {
        self.bytes.split_at(self.length as usize).0
    }

    /// Link Layer payload length following the two-byte header.
    pub const fn payload_length(self) -> u8 {
        self.length - ADVERTISING_HEADER_LENGTH as u8
    }
}

fn encode_prepared_pdu(
    advertiser: LeDeviceAddress,
    data: &[u8],
    pdu_type: u8,
    channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
) -> ([u8; LEGACY_ADVERTISING_PDU_CAPACITY], u8) {
    let mut bytes = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
    bytes[0] = pdu_type
        | match advertiser.kind() {
            LeDeviceAddressKind::Public => 0,
            LeDeviceAddressKind::Random => TX_ADD_RANDOM,
        }
        | match channel_selection_two {
            LeChannelSelectionAlgorithmTwoSupport::Unsupported => 0,
            LeChannelSelectionAlgorithmTwoSupport::Supported => CHANNEL_SELECTION_TWO,
        };
    bytes[1] = (DEVICE_ADDRESS_LENGTH + data.len()) as u8;
    bytes[2..8].copy_from_slice(&advertiser.wire_bytes());
    bytes[8..8 + data.len()].copy_from_slice(data);
    (
        bytes,
        (ADVERTISING_HEADER_LENGTH + DEVICE_ADDRESS_LENGTH + data.len()) as u8,
    )
}

/// Malformed or unsupported `ADV_IND` input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisementDecodeError {
    TruncatedHeader { available: usize },
    UnexpectedPduType { pdu_type: u8 },
    ReservedHeaderBitsSet,
    InvalidPayloadLength { length: usize },
    LengthMismatch { declared: usize, available: usize },
}

/// Reusable protocol configuration of one legacy connectable advertiser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisingSet<'a> {
    advertisement: LegacyConnectableAdvertisement<'a>,
    scan_response: LegacyScanResponseData<'a>,
    channels: PrimaryAdvertisingChannelMap,
    interval: AdvertisingInterval,
}

impl<'a> LegacyConnectableAdvertisingSet<'a> {
    pub const fn new(
        advertisement: LegacyConnectableAdvertisement<'a>,
        scan_response: LegacyScanResponseData<'a>,
        channels: PrimaryAdvertisingChannelMap,
        interval: AdvertisingInterval,
    ) -> Self {
        Self {
            advertisement,
            scan_response,
            channels,
            interval,
        }
    }

    pub const fn advertisement(self) -> LegacyConnectableAdvertisement<'a> {
        self.advertisement
    }

    pub const fn scan_response(self) -> LegacyScanResponseData<'a> {
        self.scan_response
    }

    pub const fn channels(self) -> PrimaryAdvertisingChannelMap {
        self.channels
    }

    pub const fn interval(self) -> AdvertisingInterval {
        self.interval
    }
}

/// Disabled connectable advertiser retaining the next unique Enable generation.
#[derive(Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertiserStandby {
    generations: LegacyAdvertisingGenerationAllocator,
}

impl LegacyConnectableAdvertiserStandby {
    /// Construct a fresh connectable advertiser lifecycle.
    pub const fn new() -> Self {
        Self {
            generations: LegacyAdvertisingGenerationAllocator::new(),
        }
    }

    /// Install one validated connectable advertising configuration.
    pub const fn configure<'a>(
        self,
        set: LegacyConnectableAdvertisingSet<'a>,
    ) -> LegacyConnectableAdvertiserConfigured<'a> {
        LegacyConnectableAdvertiserConfigured { standby: self, set }
    }
}

impl Default for LegacyConnectableAdvertiserStandby {
    fn default() -> Self {
        Self::new()
    }
}

/// Disabled connectable advertiser with a configuration available for Enable.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "enable, reconfigure, or retain the connectable advertiser"]
pub struct LegacyConnectableAdvertiserConfigured<'a> {
    standby: LegacyConnectableAdvertiserStandby,
    set: LegacyConnectableAdvertisingSet<'a>,
}

impl<'a> LegacyConnectableAdvertiserConfigured<'a> {
    /// Current reusable configuration retained across Enable epochs.
    pub const fn set(&self) -> LegacyConnectableAdvertisingSet<'a> {
        self.set
    }

    /// Replace the disabled configuration without allocating a generation.
    pub const fn reconfigure(self, set: LegacyConnectableAdvertisingSet<'a>) -> Self {
        Self {
            standby: self.standby,
            set,
        }
    }

    /// Begin a new Enable epoch and its first event.
    pub fn enable(
        self,
    ) -> Result<LegacyConnectableAdvertisingEvent<'a>, LegacyConnectableAdvertiserEnableError<'a>>
    {
        let Self { standby, set } = self;
        let (generations, identity) = match standby.generations.begin_enable() {
            Ok(allocated) => allocated,
            Err(generations) => {
                return Err(LegacyConnectableAdvertiserEnableError {
                    configured: Self {
                        standby: LegacyConnectableAdvertiserStandby { generations },
                        set,
                    },
                });
            }
        };
        Ok(LegacyConnectableAdvertisingEvent {
            standby: LegacyConnectableAdvertiserStandby { generations },
            identity,
            set,
        })
    }

    /// Remove the configuration without allocating an Enable generation.
    pub fn into_standby(self) -> LegacyConnectableAdvertiserStandby {
        self.standby
    }
}

/// Generation space was exhausted before connectable advertising Enable.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the configured connectable advertiser remains recoverable"]
pub struct LegacyConnectableAdvertiserEnableError<'a> {
    configured: LegacyConnectableAdvertiserConfigured<'a>,
}

impl<'a> LegacyConnectableAdvertiserEnableError<'a> {
    pub fn into_configured(self) -> LegacyConnectableAdvertiserConfigured<'a> {
        self.configured
    }
}

/// One connectable advertising event not yet admitted by a backend.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepare, cancel, or retain the connectable advertising event"]
pub struct LegacyConnectableAdvertisingEvent<'a> {
    standby: LegacyConnectableAdvertiserStandby,
    identity: LegacyAdvertisingEventIdentity,
    set: LegacyConnectableAdvertisingSet<'a>,
}

impl<'a> LegacyConnectableAdvertisingEvent<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub fn prepare(self) -> LegacyPreparedConnectableAdvertisingEvent<'a> {
        LegacyPreparedConnectableAdvertisingEvent { event: self }
    }

    /// Disable before the event was accepted by a backend.
    pub fn disable(self) -> LegacyConnectableAdvertiserConfigured<'a> {
        LegacyConnectableAdvertiserConfigured {
            standby: self.standby,
            set: self.set,
        }
    }
}

/// Prepared `ADV_IND` plus its complete primary-channel plan.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "submit to a backend or cancel the prepared event"]
pub struct LegacyPreparedConnectableAdvertisingEvent<'a> {
    event: LegacyConnectableAdvertisingEvent<'a>,
}

impl<'a> LegacyPreparedConnectableAdvertisingEvent<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.event.identity
    }

    pub const fn channels(&self) -> PrimaryAdvertisingChannelMap {
        self.event.set.channels
    }

    pub const fn advertisement(&self) -> LegacyConnectableAdvertisement<'a> {
        self.event.set.advertisement
    }

    pub const fn scan_response(&self) -> LegacyScanResponseData<'a> {
        self.event.set.scan_response
    }

    pub fn encode(&self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        self.event.set.advertisement.encode(destination)
    }

    /// Produce the complete bounded `ADV_IND` selected by this event.
    pub fn adv_ind_pdu(&self) -> LegacyConnectableAdvIndPdu {
        let advertisement = self.event.set.advertisement;
        let (bytes, length) = encode_prepared_pdu(
            advertisement.advertiser(),
            advertisement.data().as_bytes(),
            ADV_IND_TYPE,
            advertisement.channel_selection_two(),
        );
        LegacyConnectableAdvIndPdu { bytes, length }
    }

    /// Produce the matching bounded `SCAN_RSP` for this advertiser.
    pub fn scan_response_pdu(&self) -> LegacyScanResponsePdu {
        let (bytes, length) = encode_prepared_pdu(
            self.event.set.advertisement.advertiser(),
            self.event.set.scan_response.as_bytes(),
            SCAN_RSP_TYPE,
            LeChannelSelectionAlgorithmTwoSupport::Unsupported,
        );
        LegacyScanResponsePdu { bytes, length }
    }

    pub fn cancel(self) -> LegacyConnectableAdvertisingEvent<'a> {
        self.event
    }

    /// Disable before hardware accepted the prepared event.
    pub fn disable(self) -> LegacyConnectableAdvertiserConfigured<'a> {
        self.cancel().disable()
    }

    /// Mark the exact point where a backend accepted this event for execution.
    ///
    /// A chip backend must call this only after publishing its non-forgeable
    /// hardware `RUN` proof, then retain the returned owner privately until
    /// that exact event completes. Portable code deliberately cannot mint or
    /// interpret a chip-specific hardware proof.
    pub fn into_submitted(self) -> LegacyConnectableAdvertisingEventInFlight<'a> {
        LegacyConnectableAdvertisingEventInFlight { prepared: self }
    }
}

/// Protocol continuation retained while one response-capable event is in flight.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "admit received connection requests or complete the submitted event"]
pub struct LegacyConnectableAdvertisingEventInFlight<'a> {
    prepared: LegacyPreparedConnectableAdvertisingEvent<'a>,
}

impl<'a> LegacyConnectableAdvertisingEventInFlight<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.prepared.identity()
    }

    /// Validate one received `CONNECT_IND` without losing the event on failure.
    pub fn admit_connection_request(
        self,
        pdu: &[u8],
    ) -> LegacyConnectableConnectionRequestAdmission<'a> {
        let request = match LeLegacyConnectionRequest::decode(pdu) {
            Ok(request) => request,
            Err(error) => {
                return LegacyConnectableConnectionRequestAdmission::Rejected(
                    LegacyConnectableConnectionRequestRejected {
                        in_flight: self,
                        error: LegacyConnectableConnectionRequestRejection::Malformed(error),
                    },
                );
            }
        };

        let advertisement = self.prepared.event.set.advertisement;
        if !request.is_addressed_to(advertisement.advertiser()) {
            return LegacyConnectableConnectionRequestAdmission::Rejected(
                LegacyConnectableConnectionRequestRejected {
                    in_flight: self,
                    error: LegacyConnectableConnectionRequestRejection::DifferentAdvertiser,
                },
            );
        }
        if request.channel_selection() == LeChannelSelectionAlgorithm::AlgorithmTwo
            && advertisement.channel_selection_two()
                == LeChannelSelectionAlgorithmTwoSupport::Unsupported
        {
            return LegacyConnectableConnectionRequestAdmission::Rejected(
                LegacyConnectableConnectionRequestRejected {
                    in_flight: self,
                    error:
                        LegacyConnectableConnectionRequestRejection::UnsupportedChannelSelectionAlgorithmTwo,
                },
            );
        }

        LegacyConnectableConnectionRequestAdmission::Accepted(
            LegacyConnectableConnectionRequestAccepted {
                configured: LegacyConnectableAdvertiserConfigured {
                    standby: self.prepared.event.standby,
                    set: self.prepared.event.set,
                },
                identity: self.prepared.event.identity,
                connection: LePeripheralConnection::from_request(request),
            },
        )
    }

    /// Close an event in which no acceptable connection request was received.
    pub fn complete_without_connection(self) -> LegacyConnectableAdvertisingEventComplete<'a> {
        LegacyConnectableAdvertisingEventComplete {
            standby: self.prepared.event.standby,
            identity: self.prepared.event.identity,
            set: self.prepared.event.set,
        }
    }
}

/// Result of portable connection-request admission.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain either the accepted connection or the rejected event"]
pub enum LegacyConnectableConnectionRequestAdmission<'a> {
    Accepted(LegacyConnectableConnectionRequestAccepted<'a>),
    Rejected(LegacyConnectableConnectionRequestRejected<'a>),
}

/// Successful transition from advertising into a peripheral connection.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain the connection and persistent advertising configuration"]
pub struct LegacyConnectableConnectionRequestAccepted<'a> {
    configured: LegacyConnectableAdvertiserConfigured<'a>,
    identity: LegacyAdvertisingEventIdentity,
    connection: LePeripheralConnection,
}

impl<'a> LegacyConnectableConnectionRequestAccepted<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.connection.request()
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertiserConfigured<'a>,
        LegacyAdvertisingEventIdentity,
        LePeripheralConnection,
    ) {
        (self.configured, self.identity, self.connection)
    }
}

/// Failed admission with the exact in-flight advertising event retained.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "inspect the error and retain or retry the in-flight event"]
pub struct LegacyConnectableConnectionRequestRejected<'a> {
    in_flight: LegacyConnectableAdvertisingEventInFlight<'a>,
    error: LegacyConnectableConnectionRequestRejection,
}

impl<'a> LegacyConnectableConnectionRequestRejected<'a> {
    pub const fn error(&self) -> LegacyConnectableConnectionRequestRejection {
        self.error
    }

    pub fn into_in_flight(self) -> LegacyConnectableAdvertisingEventInFlight<'a> {
        self.in_flight
    }
}

/// Portable reason why a received request did not stop advertising.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableConnectionRequestRejection {
    Malformed(LeLegacyConnectionRequestError),
    DifferentAdvertiser,
    UnsupportedChannelSelectionAlgorithmTwo,
}

/// Completed connectable event retaining its reusable configuration.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "schedule the next event or retain the advertising set"]
pub struct LegacyConnectableAdvertisingEventComplete<'a> {
    standby: LegacyConnectableAdvertiserStandby,
    identity: LegacyAdvertisingEventIdentity,
    set: LegacyConnectableAdvertisingSet<'a>,
}

impl<'a> LegacyConnectableAdvertisingEventComplete<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    /// Disable between events while no backend owns an event.
    pub fn disable(self) -> LegacyConnectableAdvertiserConfigured<'a> {
        LegacyConnectableAdvertiserConfigured {
            standby: self.standby,
            set: self.set,
        }
    }

    pub fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> Result<
        ScheduledLegacyConnectableAdvertisingEvent<'a>,
        LegacyConnectableAdvertisingEventSequenceExhausted<'a>,
    > {
        let Some(identity) = self.identity.next_event() else {
            return Err(LegacyConnectableAdvertisingEventSequenceExhausted { complete: self });
        };
        Ok(ScheduledLegacyConnectableAdvertisingEvent {
            start_offset_micros: self.set.interval.as_micros() + delay.as_micros() as u64,
            event: LegacyConnectableAdvertisingEvent {
                standby: self.standby,
                identity,
                set: self.set,
            },
        })
    }
}

/// Event-sequence space was exhausted without losing the completed owner.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the completed connectable advertiser remains recoverable"]
pub struct LegacyConnectableAdvertisingEventSequenceExhausted<'a> {
    complete: LegacyConnectableAdvertisingEventComplete<'a>,
}

impl<'a> LegacyConnectableAdvertisingEventSequenceExhausted<'a> {
    pub fn into_complete(self) -> LegacyConnectableAdvertisingEventComplete<'a> {
        self.complete
    }
}

/// Next connectable event paired with its relative start deadline.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "wait until the deadline or retain the advertising set"]
pub struct ScheduledLegacyConnectableAdvertisingEvent<'a> {
    start_offset_micros: u64,
    event: LegacyConnectableAdvertisingEvent<'a>,
}

impl<'a> ScheduledLegacyConnectableAdvertisingEvent<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.event.identity
    }

    pub const fn start_offset_micros(&self) -> u64 {
        self.start_offset_micros
    }

    pub fn into_event(self) -> LegacyConnectableAdvertisingEvent<'a> {
        self.event
    }

    /// Disable before the scheduled event was submitted to a backend.
    pub fn disable(self) -> LegacyConnectableAdvertiserConfigured<'a> {
        self.event.disable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        advertising::LEGACY_ADVERTISING_PDU_CAPACITY,
        connection::{LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES},
    };

    const ADVERTISER_BYTES: [u8; 6] = [7, 8, 9, 10, 11, 12];

    fn advertiser(
        support: LeChannelSelectionAlgorithmTwoSupport,
    ) -> LegacyConnectableAdvertisingSet<'static> {
        LegacyConnectableAdvertisingSet::new(
            LegacyConnectableAdvertisement::new(
                LeDeviceAddress::from_wire_bytes(ADVERTISER_BYTES, LeDeviceAddressKind::Random),
                LegacyAdvertisingData::new(&[2, 1, 6]).unwrap(),
                support,
            ),
            LegacyScanResponseData::new(&[3, 9, 8, 7]).unwrap(),
            PrimaryAdvertisingChannelMap::all(),
            AdvertisingInterval::new(32).unwrap(),
        )
    }

    fn first_event(
        support: LeChannelSelectionAlgorithmTwoSupport,
    ) -> LegacyConnectableAdvertisingEvent<'static> {
        LegacyConnectableAdvertiserStandby::new()
            .configure(advertiser(support))
            .enable()
            .unwrap()
    }

    fn connection_request(
        advertiser: [u8; 6],
        channel_selection_two: bool,
    ) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = 0b0101 | (1 << 7) | if channel_selection_two { 1 << 5 } else { 0 };
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        pdu[8..14].copy_from_slice(&advertiser);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
        pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        pdu[35] = 5 | (4 << 5);
        pdu
    }

    #[test]
    fn adv_ind_roundtrips_with_channel_selection_capability() {
        let advertisement =
            advertiser(LeChannelSelectionAlgorithmTwoSupport::Supported).advertisement();
        let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        let length = advertisement.encode(&mut encoded).unwrap();

        assert_eq!(
            LegacyConnectableAdvertisement::decode(&encoded[..length]),
            Ok(advertisement)
        );
        assert_eq!(encoded[0], 0x60);
        assert_eq!(&encoded[2..8], &ADVERTISER_BYTES);
    }

    #[test]
    fn prepared_event_owns_matching_adv_ind_and_scan_response_pdus() {
        let prepared = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported).prepare();
        let identity = prepared.identity();
        let adv_ind = prepared.adv_ind_pdu();
        let scan_response = prepared.scan_response_pdu();

        assert_eq!(
            LegacyConnectableAdvertisement::decode(adv_ind.as_bytes()),
            Ok(prepared.advertisement())
        );
        assert_eq!(scan_response.as_bytes()[0], 0x44);
        assert_eq!(&scan_response.as_bytes()[2..8], &ADVERTISER_BYTES);
        assert_eq!(&scan_response.as_bytes()[8..], &[3, 9, 8, 7]);
        assert_eq!(scan_response.payload_length(), 10);

        let event = prepared.cancel();
        assert_eq!(event.identity(), identity);
        assert_eq!(event.prepare().scan_response().as_bytes(), &[3, 9, 8, 7]);
    }

    #[test]
    fn rejected_response_retains_the_exact_in_flight_event_for_a_later_packet() {
        let in_flight = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported)
            .prepare()
            .into_submitted();
        let identity = in_flight.identity();
        let LegacyConnectableConnectionRequestAdmission::Rejected(rejected) =
            in_flight.admit_connection_request(&connection_request([9; 6], true))
        else {
            panic!("a request for another advertiser must be rejected");
        };
        assert_eq!(
            rejected.error(),
            LegacyConnectableConnectionRequestRejection::DifferentAdvertiser
        );
        assert_eq!(rejected.in_flight.identity(), identity);

        let accepted = rejected
            .into_in_flight()
            .admit_connection_request(&connection_request(ADVERTISER_BYTES, true));
        let LegacyConnectableConnectionRequestAdmission::Accepted(accepted) = accepted else {
            panic!("the retained event must admit a valid request");
        };
        assert_eq!(
            accepted.request().advertiser().wire_bytes(),
            ADVERTISER_BYTES
        );
        assert_eq!(accepted.identity(), identity);
        let (configured, accepted_identity, connection) = accepted.into_parts();
        assert_eq!(accepted_identity, identity);
        assert_eq!(
            configured.enable().unwrap().prepare().channels(),
            PrimaryAdvertisingChannelMap::all()
        );
        assert_eq!(connection.event_counter(), 0);
    }

    #[test]
    fn algorithm_two_rejection_keeps_the_submitted_event_in_flight() {
        let in_flight = first_event(LeChannelSelectionAlgorithmTwoSupport::Unsupported)
            .prepare()
            .into_submitted();
        let identity = in_flight.identity();
        let LegacyConnectableConnectionRequestAdmission::Rejected(rejected) =
            in_flight.admit_connection_request(&connection_request(ADVERTISER_BYTES, true))
        else {
            panic!("algorithm two cannot be negotiated without advertised support");
        };
        assert_eq!(
            rejected.error(),
            LegacyConnectableConnectionRequestRejection::UnsupportedChannelSelectionAlgorithmTwo
        );
        let retained = rejected.into_in_flight();
        assert_eq!(retained.identity(), identity);
        let configured = retained.complete_without_connection().disable();
        assert_eq!(
            configured.enable().unwrap().prepare().channels(),
            PrimaryAdvertisingChannelMap::all()
        );
    }

    #[test]
    fn no_request_completion_requires_a_fresh_bounded_delay() {
        let first = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported);
        let first_identity = first.identity();
        let scheduled = first
            .prepare()
            .into_submitted()
            .complete_without_connection()
            .schedule_next(AdvertisingDelay::from_micros(7_500).unwrap())
            .unwrap();
        assert_eq!(scheduled.start_offset_micros(), 27_500);
        assert_eq!(
            scheduled.identity().generation(),
            first_identity.generation()
        );
        assert_eq!(scheduled.identity().event().get(), 1);
        assert_eq!(
            scheduled
                .into_event()
                .prepare()
                .advertisement()
                .advertiser()
                .wire_bytes(),
            ADVERTISER_BYTES
        );
    }

    #[test]
    fn disable_and_reenable_mints_a_new_generation_at_event_zero() {
        let first = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported);
        let first_identity = first.identity();
        let second = first.disable().enable().unwrap();

        assert_eq!(second.identity().generation().get(), 2);
        assert_eq!(second.identity().event().get(), 0);
        assert_ne!(second.identity().generation(), first_identity.generation());
    }

    #[test]
    fn generation_and_event_exhaustion_return_lossless_connectable_owners() {
        let standby = LegacyConnectableAdvertiserStandby {
            generations: LegacyAdvertisingGenerationAllocator::from_next_generation(Some(u32::MAX)),
        };
        let last_generation = standby
            .configure(advertiser(LeChannelSelectionAlgorithmTwoSupport::Supported))
            .enable()
            .unwrap();
        assert_eq!(last_generation.identity().generation().get(), u32::MAX);
        let exhausted = last_generation.disable().enable().unwrap_err();
        let configured = exhausted.into_configured().reconfigure(advertiser(
            LeChannelSelectionAlgorithmTwoSupport::Unsupported,
        ));
        assert!(configured.enable().is_err());

        let mut last_event = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported);
        last_event.identity = LegacyAdvertisingEventIdentity::from_parts(1, u32::MAX);
        let exhausted = last_event
            .prepare()
            .into_submitted()
            .complete_without_connection()
            .schedule_next(AdvertisingDelay::from_micros(0).unwrap())
            .unwrap_err();
        let next_generation = exhausted.into_complete().disable().enable().unwrap();
        assert_eq!(next_generation.identity().generation().get(), 2);
        assert_eq!(next_generation.identity().event().get(), 0);
    }
}
