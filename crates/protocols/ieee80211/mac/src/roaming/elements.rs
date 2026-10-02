//! Borrowed k/v information elements and their assigned IE identifiers.
use super::WireError;
use crate::management::elements as framing;
pub use framing::{
    ELEMENT_HEADER_LEN, ELEMENT_LENGTH_OFFSET, MAX_ELEMENT_BODY_LEN, MAX_ENCODED_ELEMENT_LEN,
};

/// Information-element identifiers; subelement namespaces are protocol-specific.
pub mod element_id {
    pub const SSID: u8 = crate::management::SSID_ELEMENT_ID;
    pub const BSS_LOAD: u8 = 11;
    pub const TSPEC: u8 = 13;
    pub const TPC_REPORT: u8 = 35;
    pub const AP_CHANNEL_REPORT: u8 = 51;
    pub const MEASUREMENT_REQUEST: u8 = 38;
    pub const MEASUREMENT_REPORT: u8 = 39;
    pub const TCLAS: u8 = 14;
    pub const TCLAS_PROCESSING: u8 = 44;
    pub const NEIGHBOR_REPORT: u8 = 52;
    pub const SUPPORTED_OPERATING_CLASSES: u8 = 59;
    pub const RM_ENABLED_CAPABILITIES: u8 = 70;
    pub const EVENT_REQUEST: u8 = 78;
    pub const EVENT_REPORT: u8 = 79;
    pub const DIAGNOSTIC_REQUEST: u8 = 80;
    pub const DIAGNOSTIC_REPORT: u8 = 81;
    pub const TFS_REQUEST: u8 = 91;
    pub const TFS_RESPONSE: u8 = 92;
    pub const DMS_REQUEST: u8 = 99;
    pub const DMS_RESPONSE: u8 = 100;
    pub const BSS_MAX_IDLE: u8 = 90;
    pub const WNM_SLEEP: u8 = 93;
    pub const EXTENDED_CAPABILITIES: u8 = 127;
    pub const DESTINATION_URI: u8 = 141;
    pub const VENDOR_SPECIFIC: u8 = crate::management::VENDOR_ELEMENT_ID;
}

/// A completely validated sequence of two-octet-header TLVs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Elements<'a>(framing::Elements<'a>);

impl<'a> Elements<'a> {
    pub const EMPTY: Self = Self(framing::Elements::EMPTY);

    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        framing::Elements::parse(bytes)
            .map(Self)
            .map_err(wire_error)
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }

    pub fn iter(self) -> impl Iterator<Item = Element<'a>> {
        self.0.iter().map(|element| Element {
            id: element.id,
            body: element.body,
        })
    }

    pub fn unique(self, id: u8) -> Result<Option<&'a [u8]>, WireError> {
        self.0
            .unique(id)
            .map(|element| element.map(|element| element.body))
            .map_err(wire_error)
    }
}

fn wire_error(error: framing::ElementError) -> WireError {
    match error {
        framing::ElementError::Truncated => WireError::MalformedElement,
        framing::ElementError::Duplicate(id) => WireError::DuplicateElement(id),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub id: u8,
    pub body: &'a [u8],
}
