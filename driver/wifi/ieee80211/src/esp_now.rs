//! Allocation-free ESP-NOW v1 wire format.
//!
//! ESP-NOW v1 is carried in a vendor-specific IEEE 802.11 Action management
//! frame. This module owns only the public on-air representation: it does not
//! select a radio channel, install a key, allocate a DMA descriptor or weaken
//! a receive filter. In particular, decoding a frame here is not evidence that
//! a backend has admitted or authenticated its sender.

use core::fmt;

pub const ESP_NOW_ACTION_CATEGORY: u8 = 127;
pub const ESP_NOW_ORGANIZATION_IDENTIFIER: [u8; 3] = [0x18, 0xfe, 0x34];
pub const ESP_NOW_VENDOR_ELEMENT_ID: u8 = 221;
pub const ESP_NOW_VENDOR_ELEMENT_TYPE: u8 = 4;
pub const ESP_NOW_V1_VERSION: u8 = 1;
pub const ESP_NOW_V1_MAX_PAYLOAD_LEN: usize = 250;
pub const ESP_NOW_V1_ACTION_OVERHEAD: usize = 15;
pub const ESP_NOW_MANAGEMENT_HEADER_LEN: usize = 24;
pub const ESP_NOW_V1_MAX_MPDU_LEN: usize =
    ESP_NOW_MANAGEMENT_HEADER_LEN + ESP_NOW_V1_ACTION_OVERHEAD + ESP_NOW_V1_MAX_PAYLOAD_LEN;

const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const PROTOCOL_VERSION_MASK: u16 = 0x0003;
const FRAME_TYPE_AND_SUBTYPE_MASK: u16 = 0x00fc;
const TO_FROM_DS_MASK: u16 = 0x0300;
const MORE_FRAGMENTS_FLAG: u16 = 0x0400;
const RETRY_FLAG: u16 = 0x0800;
const POWER_MANAGEMENT_FLAG: u16 = 0x1000;
const MORE_DATA_FLAG: u16 = 0x2000;
const PROTECTED_FLAG: u16 = 0x4000;
const ORDER_FLAG: u16 = 0x8000;
const BROADCAST_ADDRESS: [u8; 6] = [0xff; 6];
const VENDOR_ELEMENT_FIXED_BODY_LEN: usize = 5;

/// Valid individual address used as the source of an ESP-NOW frame.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct EspNowUnicastAddress([u8; 6]);

impl EspNowUnicastAddress {
    pub const fn new(bytes: [u8; 6]) -> Result<Self, EspNowAddressError> {
        if bytes[0] == 0
            && bytes[1] == 0
            && bytes[2] == 0
            && bytes[3] == 0
            && bytes[4] == 0
            && bytes[5] == 0
        {
            return Err(EspNowAddressError::Unspecified);
        }
        if bytes[0] & 1 != 0 {
            return Err(EspNowAddressError::Multicast);
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> [u8; 6] {
        self.0
    }
}

/// Address accepted as address one of a plaintext ESP-NOW frame.
///
/// Other group addresses are rejected. The initial production profile owns
/// only exact broadcast and individual-peer exchanges.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EspNowDestination {
    Broadcast,
    Unicast(EspNowUnicastAddress),
}

impl EspNowDestination {
    pub const fn new(bytes: [u8; 6]) -> Result<Self, EspNowAddressError> {
        if bytes[0] == 0xff
            && bytes[1] == 0xff
            && bytes[2] == 0xff
            && bytes[3] == 0xff
            && bytes[4] == 0xff
            && bytes[5] == 0xff
        {
            return Ok(Self::Broadcast);
        }
        match EspNowUnicastAddress::new(bytes) {
            Ok(address) => Ok(Self::Unicast(address)),
            Err(error) => Err(error),
        }
    }

    pub const fn bytes(self) -> [u8; 6] {
        match self {
            Self::Broadcast => BROADCAST_ADDRESS,
            Self::Unicast(address) => address.bytes(),
        }
    }

    pub const fn is_broadcast(self) -> bool {
        matches!(self, Self::Broadcast)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowAddressError {
    Unspecified,
    Multicast,
}

impl fmt::Display for EspNowAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unspecified => "an ESP-NOW address cannot be all zero",
            Self::Multicast => "an ESP-NOW peer must be unicast or the exact broadcast address",
        })
    }
}

impl core::error::Error for EspNowAddressError {}

/// Four opaque anti-replay/anti-collision bytes in the Action frame prefix.
///
/// Entropy ownership stays with the caller. Keeping this value byte-oriented
/// avoids imposing a host-endian interpretation on an on-air opaque field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct EspNowRandomValue([u8; 4]);

impl EspNowRandomValue {
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 4] {
        self.0
    }
}

/// Borrowed v1 payload whose size fits the one-byte vendor-element length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct EspNowV1Payload<'payload>(&'payload [u8]);

impl<'payload> EspNowV1Payload<'payload> {
    pub const fn new(bytes: &'payload [u8]) -> Result<Self, EspNowV1WireError> {
        if bytes.len() > ESP_NOW_V1_MAX_PAYLOAD_LEN {
            return Err(EspNowV1WireError::PayloadTooLong {
                length: bytes.len(),
            });
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> &'payload [u8] {
        self.0
    }

    pub const fn len(self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

/// Strictly validated ESP-NOW v1 vendor Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowV1Action<'payload> {
    random_value: EspNowRandomValue,
    payload: EspNowV1Payload<'payload>,
}

impl<'payload> EspNowV1Action<'payload> {
    pub const fn new(
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<Self, EspNowV1WireError> {
        let payload = match EspNowV1Payload::new(payload) {
            Ok(payload) => payload,
            Err(error) => return Err(error),
        };
        Ok(Self {
            random_value,
            payload,
        })
    }

    pub const fn random_value(self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn payload(self) -> EspNowV1Payload<'payload> {
        self.payload
    }

    pub const fn encoded_len(self) -> usize {
        ESP_NOW_V1_ACTION_OVERHEAD + self.payload.len()
    }

    /// Encode category, action OUI, random value and exactly one v1 vendor IE.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV1WireError> {
        let required = self.encoded_len();
        if output.len() < required {
            return Err(EspNowV1WireError::OutputTooSmall { required });
        }

        let output = &mut output[..required];
        output[0] = ESP_NOW_ACTION_CATEGORY;
        output[1..4].copy_from_slice(&ESP_NOW_ORGANIZATION_IDENTIFIER);
        output[4..8].copy_from_slice(&self.random_value.bytes());
        output[8] = ESP_NOW_VENDOR_ELEMENT_ID;
        output[9] = (VENDOR_ELEMENT_FIXED_BODY_LEN + self.payload.len()) as u8;
        output[10..13].copy_from_slice(&ESP_NOW_ORGANIZATION_IDENTIFIER);
        output[13] = ESP_NOW_VENDOR_ELEMENT_TYPE;
        // Version occupies the low nibble. All v1 reserved bits remain zero.
        output[14] = ESP_NOW_V1_VERSION;
        output[15..].copy_from_slice(self.payload.bytes());
        Ok(required)
    }

    /// Parse one complete v1 Action body and reject trailing or partial IEs.
    pub fn parse(input: &'payload [u8]) -> Result<Self, EspNowV1WireError> {
        if input.len() < 10 {
            return Err(EspNowV1WireError::FrameTooShort { minimum: 10 });
        }
        if input[0] != ESP_NOW_ACTION_CATEGORY {
            return Err(EspNowV1WireError::UnsupportedActionCategory(input[0]));
        }
        if input[1..4] != ESP_NOW_ORGANIZATION_IDENTIFIER {
            return Err(EspNowV1WireError::InvalidActionOrganizationIdentifier);
        }
        if input[8] != ESP_NOW_VENDOR_ELEMENT_ID {
            return Err(EspNowV1WireError::UnsupportedElementId(input[8]));
        }

        let declared = usize::from(input[9]);
        if declared < VENDOR_ELEMENT_FIXED_BODY_LEN {
            return Err(EspNowV1WireError::ElementBodyTooShort { declared: input[9] });
        }
        let expected = 10 + declared;
        if input.len() != expected {
            return Err(EspNowV1WireError::ElementLengthMismatch {
                declared: input[9],
                actual: input.len().saturating_sub(10),
            });
        }
        if input[10..13] != ESP_NOW_ORGANIZATION_IDENTIFIER {
            return Err(EspNowV1WireError::InvalidElementOrganizationIdentifier);
        }
        if input[13] != ESP_NOW_VENDOR_ELEMENT_TYPE {
            return Err(EspNowV1WireError::UnsupportedElementType(input[13]));
        }
        let version = input[14];
        if version & 0xf0 != 0 {
            return Err(EspNowV1WireError::ReservedVersionBitsSet(version));
        }
        if version & 0x0f != ESP_NOW_V1_VERSION {
            return Err(EspNowV1WireError::UnsupportedVersion(version & 0x0f));
        }

        Ok(Self {
            random_value: EspNowRandomValue::new([input[4], input[5], input[6], input[7]]),
            payload: EspNowV1Payload(&input[15..]),
        })
    }
}

/// Complete plaintext ESP-NOW v1 MPDU without the hardware-generated FCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowV1Frame<'payload> {
    destination: EspNowDestination,
    source: EspNowUnicastAddress,
    sequence_number: u16,
    retry: bool,
    action: EspNowV1Action<'payload>,
}

impl<'payload> EspNowV1Frame<'payload> {
    pub const fn new(
        destination: EspNowDestination,
        source: EspNowUnicastAddress,
        sequence_number: u16,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<Self, EspNowV1WireError> {
        if sequence_number > 0x0fff {
            return Err(EspNowV1WireError::InvalidSequenceNumber(sequence_number));
        }
        let action = match EspNowV1Action::new(random_value, payload) {
            Ok(action) => action,
            Err(error) => return Err(error),
        };
        Ok(Self {
            destination,
            source,
            sequence_number,
            retry: false,
            action,
        })
    }

    pub const fn destination(self) -> EspNowDestination {
        self.destination
    }

    pub const fn source(self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn sequence_number(self) -> u16 {
        self.sequence_number
    }

    pub const fn retry(self) -> bool {
        self.retry
    }

    pub const fn action(self) -> EspNowV1Action<'payload> {
        self.action
    }

    pub const fn encoded_len(self) -> usize {
        ESP_NOW_MANAGEMENT_HEADER_LEN + self.action.encoded_len()
    }

    /// Encode an Action MPDU with broadcast BSSID and no FCS.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV1WireError> {
        let required = self.encoded_len();
        if output.len() < required {
            return Err(EspNowV1WireError::OutputTooSmall { required });
        }

        let output = &mut output[..required];
        output[..ESP_NOW_MANAGEMENT_HEADER_LEN].fill(0);
        let frame_control = ACTION_FRAME_CONTROL | if self.retry { RETRY_FLAG } else { 0 };
        output[0..2].copy_from_slice(&frame_control.to_le_bytes());
        output[4..10].copy_from_slice(&self.destination.bytes());
        output[10..16].copy_from_slice(&self.source.bytes());
        output[16..22].copy_from_slice(&BROADCAST_ADDRESS);
        output[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        self.action
            .encode(&mut output[ESP_NOW_MANAGEMENT_HEADER_LEN..])?;
        Ok(required)
    }

    /// Parse one complete unprotected, unfragmented ESP-NOW v1 Action MPDU.
    pub fn parse(input: &'payload [u8]) -> Result<Self, EspNowV1WireError> {
        if input.len() < ESP_NOW_MANAGEMENT_HEADER_LEN {
            return Err(EspNowV1WireError::FrameTooShort {
                minimum: ESP_NOW_MANAGEMENT_HEADER_LEN,
            });
        }
        let frame_control = u16::from_le_bytes([input[0], input[1]]);
        if frame_control & PROTOCOL_VERSION_MASK != 0
            || frame_control & FRAME_TYPE_AND_SUBTYPE_MASK != ACTION_FRAME_CONTROL
            || frame_control & TO_FROM_DS_MASK != 0
            || frame_control & (POWER_MANAGEMENT_FLAG | MORE_DATA_FLAG | ORDER_FLAG) != 0
        {
            return Err(EspNowV1WireError::UnsupportedFrameControl(frame_control));
        }
        if frame_control & PROTECTED_FLAG != 0 {
            return Err(EspNowV1WireError::ProtectedFrameUnsupported);
        }

        let destination =
            EspNowDestination::new([input[4], input[5], input[6], input[7], input[8], input[9]])
                .map_err(EspNowV1WireError::InvalidDestination)?;
        let source = EspNowUnicastAddress::new([
            input[10], input[11], input[12], input[13], input[14], input[15],
        ])
        .map_err(EspNowV1WireError::InvalidSource)?;
        if input[16..22] != BROADCAST_ADDRESS {
            return Err(EspNowV1WireError::InvalidBssid);
        }
        let sequence_control = u16::from_le_bytes([input[22], input[23]]);
        if frame_control & MORE_FRAGMENTS_FLAG != 0 || sequence_control & 0x000f != 0 {
            return Err(EspNowV1WireError::FragmentedFrame(
                (sequence_control & 0x000f) as u8,
            ));
        }
        let action = EspNowV1Action::parse(&input[ESP_NOW_MANAGEMENT_HEADER_LEN..])?;
        Ok(Self {
            destination,
            source,
            sequence_number: sequence_control >> 4,
            retry: frame_control & RETRY_FLAG != 0,
            action,
        })
    }
}

/// Why bytes cannot cross the strict plaintext ESP-NOW v1 boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV1WireError {
    PayloadTooLong { length: usize },
    OutputTooSmall { required: usize },
    FrameTooShort { minimum: usize },
    InvalidSequenceNumber(u16),
    InvalidDestination(EspNowAddressError),
    InvalidSource(EspNowAddressError),
    InvalidBssid,
    UnsupportedFrameControl(u16),
    ProtectedFrameUnsupported,
    FragmentedFrame(u8),
    UnsupportedActionCategory(u8),
    InvalidActionOrganizationIdentifier,
    UnsupportedElementId(u8),
    ElementBodyTooShort { declared: u8 },
    ElementLengthMismatch { declared: u8, actual: usize },
    InvalidElementOrganizationIdentifier,
    UnsupportedElementType(u8),
    ReservedVersionBitsSet(u8),
    UnsupportedVersion(u8),
}

impl fmt::Display for EspNowV1WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLong { length } => write!(
                formatter,
                "ESP-NOW v1 payload length {length} exceeds {ESP_NOW_V1_MAX_PAYLOAD_LEN} bytes"
            ),
            Self::OutputTooSmall { required } => {
                write!(formatter, "ESP-NOW output needs {required} bytes")
            }
            Self::FrameTooShort { minimum } => {
                write!(formatter, "ESP-NOW input is shorter than {minimum} bytes")
            }
            Self::InvalidSequenceNumber(sequence) => {
                write!(
                    formatter,
                    "ESP-NOW sequence number {sequence} exceeds 12 bits"
                )
            }
            Self::InvalidDestination(error) => write!(formatter, "invalid destination: {error}"),
            Self::InvalidSource(error) => write!(formatter, "invalid source: {error}"),
            Self::InvalidBssid => formatter.write_str("ESP-NOW v1 BSSID is not broadcast"),
            Self::UnsupportedFrameControl(frame_control) => write!(
                formatter,
                "unsupported ESP-NOW frame-control value {frame_control:#06x}"
            ),
            Self::ProtectedFrameUnsupported => {
                formatter.write_str("protected ESP-NOW frames are outside the v1 plaintext profile")
            }
            Self::FragmentedFrame(fragment) => {
                write!(formatter, "ESP-NOW fragment {fragment} is unsupported")
            }
            Self::UnsupportedActionCategory(category) => {
                write!(formatter, "unsupported Action category {category}")
            }
            Self::InvalidActionOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW Action organization identifier")
            }
            Self::UnsupportedElementId(id) => {
                write!(formatter, "unsupported ESP-NOW element id {id}")
            }
            Self::ElementBodyTooShort { declared } => write!(
                formatter,
                "ESP-NOW vendor element declares only {declared} body bytes"
            ),
            Self::ElementLengthMismatch { declared, actual } => write!(
                formatter,
                "ESP-NOW vendor element declares {declared} bytes but contains {actual}"
            ),
            Self::InvalidElementOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW element organization identifier")
            }
            Self::UnsupportedElementType(element_type) => {
                write!(formatter, "unsupported ESP-NOW element type {element_type}")
            }
            Self::ReservedVersionBitsSet(version) => write!(
                formatter,
                "ESP-NOW v1 version byte {version:#04x} has reserved bits set"
            ),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported ESP-NOW version {version}")
            }
        }
    }
}

impl core::error::Error for EspNowV1WireError {}
