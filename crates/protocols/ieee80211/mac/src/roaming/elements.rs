//! Borrowed k/v information elements and their assigned IE identifiers.
use super::WireError;
/// ID and Length octets preceding each element body.
pub const ELEMENT_HEADER_LEN: usize = 2;
pub const ELEMENT_LENGTH_OFFSET: usize = 1;
pub const MAX_ELEMENT_BODY_LEN: usize = u8::MAX as usize;
pub const MAX_ENCODED_ELEMENT_LEN: usize = ELEMENT_HEADER_LEN + MAX_ELEMENT_BODY_LEN;

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
pub struct Elements<'a>(&'a [u8]);

impl<'a> Elements<'a> {
    pub const EMPTY: Self = Self(&[]);

    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut rest = bytes;
        while !rest.is_empty() {
            if rest.len() < ELEMENT_HEADER_LEN {
                return Err(WireError::MalformedElement);
            }
            let len = ELEMENT_HEADER_LEN + usize::from(rest[ELEMENT_LENGTH_OFFSET]);
            rest = rest.get(len..).ok_or(WireError::MalformedElement)?;
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub fn iter(self) -> impl Iterator<Item = Element<'a>> {
        let mut rest = self.0;
        core::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let len = usize::from(rest[ELEMENT_LENGTH_OFFSET]);
            let element = Element {
                id: rest[0],
                body: &rest[ELEMENT_HEADER_LEN..ELEMENT_HEADER_LEN + len],
            };
            rest = &rest[ELEMENT_HEADER_LEN + len..];
            Some(element)
        })
    }

    pub fn unique(self, id: u8) -> Result<Option<&'a [u8]>, WireError> {
        let mut found = None;
        for element in self.iter().filter(|element| element.id == id) {
            if found.replace(element.body).is_some() {
                return Err(WireError::DuplicateElement(id));
            }
        }
        Ok(found)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub id: u8,
    pub body: &'a [u8],
}
