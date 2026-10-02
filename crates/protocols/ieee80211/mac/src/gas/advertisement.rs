use super::*;
use crate::management::{
    VENDOR_ELEMENT_ID,
    elements::{ELEMENT_HEADER_LEN, Elements, MAX_ELEMENT_BODY_LEN},
};

pub const ADVERTISEMENT_PROTOCOL_ELEMENT_ID: u8 = 108;
pub const ANQP_PROTOCOL_ID: u8 = 0;
/// Query Response Length Limit is expressed in 256-octet units.
pub const RESPONSE_LIMIT_UNIT: usize = 256;
pub const MAX_RESPONSE_LIMIT_UNITS: u8 = 127;
const PAME_BI_BIT: u8 = 0x80;
const MIN_VENDOR_PROTOCOL_BODY_LEN: usize = 3;

/// The protocol identity includes the entire vendor-specific body when used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolId<'a> {
    Standard(u8),
    Vendor(&'a [u8]),
}
impl ProtocolId<'_> {
    fn encoded_len(self) -> Result<usize, WireError> {
        match self {
            Self::Standard(id) if id != VENDOR_ELEMENT_ID => Ok(1),
            Self::Vendor(body)
                if (MIN_VENDOR_PROTOCOL_BODY_LEN..=MAX_ELEMENT_BODY_LEN).contains(&body.len()) =>
            {
                Ok(ELEMENT_HEADER_LEN + body.len())
            }
            _ => Err(WireError::InvalidAdvertisement),
        }
    }
    fn encode(self, writer: &mut BodyWriter<'_>) {
        match self {
            Self::Standard(id) => writer.put(&[id]),
            Self::Vendor(body) => {
                writer.put(&[VENDOR_ELEMENT_ID, body.len() as u8]);
                writer.put(body);
            }
        }
    }
}

/// IEEE 802.11 Query Response Info. Zero units is used by requests; 127
/// leaves the limit to the maximum representable GAS fragment sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseInfo(u8);
impl ResponseInfo {
    pub fn new(limit_units: u8, bssid_independent: bool) -> Result<Self, WireError> {
        if limit_units > MAX_RESPONSE_LIMIT_UNITS {
            return Err(WireError::InvalidAdvertisement);
        }
        Ok(Self(
            limit_units | if bssid_independent { PAME_BI_BIT } else { 0 },
        ))
    }
    pub const fn limit_units(self) -> u8 {
        self.0 & MAX_RESPONSE_LIMIT_UNITS
    }
    pub const fn bssid_independent(self) -> bool {
        self.0 & PAME_BI_BIT != 0
    }
    pub const fn limit_octets(self) -> Option<usize> {
        match self.limit_units() {
            0 | MAX_RESPONSE_LIMIT_UNITS => None,
            units => Some(units as usize * RESPONSE_LIMIT_UNIT),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisementProtocol<'a> {
    pub info: ResponseInfo,
    pub protocol: ProtocolId<'a>,
}
impl<'a> AdvertisementProtocol<'a> {
    pub fn encoded_len(self) -> Result<usize, WireError> {
        let body = 1 + self.protocol.encoded_len()?;
        if body > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::InvalidAdvertisement);
        }
        Ok(ELEMENT_HEADER_LEN + body)
    }
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = self.encoded_len()?;
        self.write(&mut BodyWriter::new(output(bytes, length)?));
        Ok(length)
    }
    pub(super) fn write(self, writer: &mut BodyWriter<'_>) {
        let length = self.encoded_len().expect("validated advertisement");
        writer.put(&[
            ADVERTISEMENT_PROTOCOL_ELEMENT_ID,
            (length - ELEMENT_HEADER_LEN) as u8,
            self.info.0,
        ]);
        self.protocol.encode(writer);
    }
    /// GAS Action frames carry exactly one advertisement protocol tuple.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let list = AdvertisementProtocols::parse(bytes)?;
        let mut tuples = list.iter();
        let tuple = tuples.next().ok_or(WireError::InvalidAdvertisement)?;
        if tuples.next().is_some() {
            return Err(WireError::InvalidAdvertisement);
        }
        Ok(tuple)
    }
}

/// A complete Advertisement Protocol IE, also usable in beacon/probe bodies.
/// All tuples, including vendor identities, are validated and retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisementProtocols<'a>(&'a [u8]);
impl<'a> AdvertisementProtocols<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let elements = Elements::parse(bytes)?;
        let mut iter = elements.iter();
        let element = iter.next().ok_or(WireError::InvalidAdvertisement)?;
        if element.id != ADVERTISEMENT_PROTOCOL_ELEMENT_ID
            || iter.next().is_some()
            || element.body.is_empty()
        {
            return Err(WireError::InvalidAdvertisement);
        }
        let mut fields = BodyReader::new(element.body);
        while !fields.remaining().is_empty() {
            read_tuple(&mut fields)?;
        }
        Ok(Self(bytes))
    }
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
    pub fn iter(self) -> impl Iterator<Item = AdvertisementProtocol<'a>> {
        let mut fields = BodyReader::new(&self.0[ELEMENT_HEADER_LEN..]);
        core::iter::from_fn(move || {
            if fields.remaining().is_empty() {
                None
            } else {
                Some(read_tuple(&mut fields).expect("validated tuples"))
            }
        })
    }
}
fn read_tuple<'a>(fields: &mut BodyReader<'a>) -> Result<AdvertisementProtocol<'a>, WireError> {
    let info = ResponseInfo(fields.u8()?);
    let id = fields.u8()?;
    let protocol = if id == VENDOR_ELEMENT_ID {
        let length = usize::from(fields.u8()?);
        if length < MIN_VENDOR_PROTOCOL_BODY_LEN {
            return Err(WireError::InvalidAdvertisement);
        }
        ProtocolId::Vendor(fields.take(length)?)
    } else {
        ProtocolId::Standard(id)
    };
    Ok(AdvertisementProtocol { info, protocol })
}
