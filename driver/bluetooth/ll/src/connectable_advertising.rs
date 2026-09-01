//! Portable legacy connectable advertising and `CONNECT_IND` admission.
//!
//! This module stops at the air-interface/backend boundary. It owns the
//! `ADV_IND` PDU, the selected primary-channel event and the lossless protocol
//! transition into a peripheral connection. A chip backend still owns packet
//! timing, receive filtering, scheduler publication and completion.

use crate::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        AdvertisingDelay, AdvertisingInterval, LegacyAdvertisingData, LegacyAdvertisingEncodeError,
        PrimaryAdvertisingChannelMap,
    },
    connection::{
        LeChannelSelectionAlgorithm, LeLegacyConnectionRequest, LeLegacyConnectionRequestError,
        LePeripheralConnection,
    },
};

const ADVERTISING_HEADER_LENGTH: usize = 2;
const DEVICE_ADDRESS_LENGTH: usize = 6;
const ADV_IND_TYPE: u8 = 0;
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
    channels: PrimaryAdvertisingChannelMap,
    interval: AdvertisingInterval,
}

impl<'a> LegacyConnectableAdvertisingSet<'a> {
    pub const fn new(
        advertisement: LegacyConnectableAdvertisement<'a>,
        channels: PrimaryAdvertisingChannelMap,
        interval: AdvertisingInterval,
    ) -> Self {
        Self {
            advertisement,
            channels,
            interval,
        }
    }

    pub const fn advertisement(self) -> LegacyConnectableAdvertisement<'a> {
        self.advertisement
    }

    pub const fn channels(self) -> PrimaryAdvertisingChannelMap {
        self.channels
    }

    pub const fn interval(self) -> AdvertisingInterval {
        self.interval
    }

    pub const fn begin_event(self) -> LegacyConnectableAdvertisingEvent<'a> {
        LegacyConnectableAdvertisingEvent { set: self }
    }
}

/// One connectable advertising event not yet admitted by a backend.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepare, cancel, or retain the connectable advertising event"]
pub struct LegacyConnectableAdvertisingEvent<'a> {
    set: LegacyConnectableAdvertisingSet<'a>,
}

impl<'a> LegacyConnectableAdvertisingEvent<'a> {
    pub fn prepare(self) -> LegacyPreparedConnectableAdvertisingEvent<'a> {
        LegacyPreparedConnectableAdvertisingEvent { event: self }
    }

    pub fn into_set(self) -> LegacyConnectableAdvertisingSet<'a> {
        self.set
    }
}

/// Prepared `ADV_IND` plus its complete primary-channel plan.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "submit, admit a response, or cancel the prepared event"]
pub struct LegacyPreparedConnectableAdvertisingEvent<'a> {
    event: LegacyConnectableAdvertisingEvent<'a>,
}

impl<'a> LegacyPreparedConnectableAdvertisingEvent<'a> {
    pub const fn channels(&self) -> PrimaryAdvertisingChannelMap {
        self.event.set.channels
    }

    pub fn encode(&self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        self.event.set.advertisement.encode(destination)
    }

    pub fn cancel(self) -> LegacyConnectableAdvertisingEvent<'a> {
        self.event
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
                        prepared: self,
                        error: LegacyConnectableConnectionRequestRejection::Malformed(error),
                    },
                );
            }
        };

        let advertisement = self.event.set.advertisement;
        if !request.is_addressed_to(advertisement.advertiser()) {
            return LegacyConnectableConnectionRequestAdmission::Rejected(
                LegacyConnectableConnectionRequestRejected {
                    prepared: self,
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
                    prepared: self,
                    error:
                        LegacyConnectableConnectionRequestRejection::UnsupportedChannelSelectionAlgorithmTwo,
                },
            );
        }

        LegacyConnectableConnectionRequestAdmission::Accepted(
            LegacyConnectableConnectionRequestAccepted {
                set: self.event.set,
                connection: LePeripheralConnection::from_request(request),
            },
        )
    }

    /// Close an event in which no acceptable connection request was received.
    pub fn complete_without_connection(self) -> LegacyConnectableAdvertisingEventComplete<'a> {
        LegacyConnectableAdvertisingEventComplete {
            set: self.event.set,
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
    set: LegacyConnectableAdvertisingSet<'a>,
    connection: LePeripheralConnection,
}

impl<'a> LegacyConnectableConnectionRequestAccepted<'a> {
    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.connection.request()
    }

    pub fn into_parts(self) -> (LegacyConnectableAdvertisingSet<'a>, LePeripheralConnection) {
        (self.set, self.connection)
    }
}

/// Failed admission with the exact prepared advertising event retained.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "inspect the error and retain or retry the prepared event"]
pub struct LegacyConnectableConnectionRequestRejected<'a> {
    prepared: LegacyPreparedConnectableAdvertisingEvent<'a>,
    error: LegacyConnectableConnectionRequestRejection,
}

impl<'a> LegacyConnectableConnectionRequestRejected<'a> {
    pub const fn error(&self) -> LegacyConnectableConnectionRequestRejection {
        self.error
    }

    pub fn into_prepared(self) -> LegacyPreparedConnectableAdvertisingEvent<'a> {
        self.prepared
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
    set: LegacyConnectableAdvertisingSet<'a>,
}

impl<'a> LegacyConnectableAdvertisingEventComplete<'a> {
    pub fn into_set(self) -> LegacyConnectableAdvertisingSet<'a> {
        self.set
    }

    pub const fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> ScheduledLegacyConnectableAdvertisingEvent<'a> {
        ScheduledLegacyConnectableAdvertisingEvent {
            start_offset_micros: self.set.interval.as_micros() + delay.as_micros() as u64,
            event: self.set.begin_event(),
        }
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
    pub const fn start_offset_micros(&self) -> u64 {
        self.start_offset_micros
    }

    pub fn into_event(self) -> LegacyConnectableAdvertisingEvent<'a> {
        self.event
    }

    pub fn cancel(self) -> LegacyConnectableAdvertisingSet<'a> {
        self.event.into_set()
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
            PrimaryAdvertisingChannelMap::all(),
            AdvertisingInterval::new(32).unwrap(),
        )
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
    fn rejected_response_retains_the_exact_event_for_a_later_packet() {
        let prepared = advertiser(LeChannelSelectionAlgorithmTwoSupport::Supported)
            .begin_event()
            .prepare();
        let LegacyConnectableConnectionRequestAdmission::Rejected(rejected) =
            prepared.admit_connection_request(&connection_request([9; 6], true))
        else {
            panic!("a request for another advertiser must be rejected");
        };
        assert_eq!(
            rejected.error(),
            LegacyConnectableConnectionRequestRejection::DifferentAdvertiser
        );

        let accepted = rejected
            .into_prepared()
            .admit_connection_request(&connection_request(ADVERTISER_BYTES, true));
        let LegacyConnectableConnectionRequestAdmission::Accepted(accepted) = accepted else {
            panic!("the retained event must admit a valid request");
        };
        assert_eq!(
            accepted.request().advertiser().wire_bytes(),
            ADVERTISER_BYTES
        );
        let (set, connection) = accepted.into_parts();
        assert_eq!(set.channels(), PrimaryAdvertisingChannelMap::all());
        assert_eq!(connection.event_counter(), 0);
    }

    #[test]
    fn algorithm_two_requires_advertised_support_without_losing_the_event() {
        let prepared = advertiser(LeChannelSelectionAlgorithmTwoSupport::Unsupported)
            .begin_event()
            .prepare();
        let LegacyConnectableConnectionRequestAdmission::Rejected(rejected) =
            prepared.admit_connection_request(&connection_request(ADVERTISER_BYTES, true))
        else {
            panic!("algorithm two cannot be negotiated without advertised support");
        };
        assert_eq!(
            rejected.error(),
            LegacyConnectableConnectionRequestRejection::UnsupportedChannelSelectionAlgorithmTwo
        );
        assert_eq!(
            rejected.into_prepared().channels(),
            PrimaryAdvertisingChannelMap::all()
        );
    }

    #[test]
    fn no_request_completion_requires_a_fresh_bounded_delay() {
        let scheduled = advertiser(LeChannelSelectionAlgorithmTwoSupport::Supported)
            .begin_event()
            .prepare()
            .complete_without_connection()
            .schedule_next(AdvertisingDelay::from_micros(7_500).unwrap());
        assert_eq!(scheduled.start_offset_micros(), 27_500);
        assert_eq!(
            scheduled.cancel().advertisement().advertiser().wire_bytes(),
            ADVERTISER_BYTES
        );
    }
}
