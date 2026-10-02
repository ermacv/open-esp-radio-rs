//! Mobility Domain, Fast Transition, timeout and protected IE views.

use super::*;
use crate::management::elements::MAX_ELEMENT_BODY_LEN;
use crate::management::{MAC_ADDRESS_LEN, MacAddress};
use crate::security::{
    RSN_NONCE_LEN,
    rsn::{RSN_ELEMENT_ID, RSNXE_ELEMENT_ID, RsnElement},
};

pub const MOBILITY_DOMAIN_ELEMENT_ID: u8 = 54;
pub const FAST_TRANSITION_ELEMENT_ID: u8 = 55;
pub const TIMEOUT_INTERVAL_ELEMENT_ID: u8 = 56;
pub const RIC_DATA_ELEMENT_ID: u8 = 57;
pub const MOBILITY_DOMAIN_ID_LEN: usize = 2;
pub const R0KH_ID_MAX_LEN: usize = 48;
/// R1KH-ID is an opaque identifier, not an address with I/G semantics.
pub const R1KH_ID_LEN: usize = MAC_ADDRESS_LEN;
pub const FT_MIC_LEN: usize = 16;
pub const FT_MIC_CONTROL_LEN: usize = 2;
pub const FT_MIC_OFFSET: usize = ELEMENT_HEADER_LEN + FT_MIC_CONTROL_LEN;
pub const FT_ANONCE_OFFSET: usize = FT_MIC_OFFSET + FT_MIC_LEN;
pub const FT_SNONCE_OFFSET: usize = FT_ANONCE_OFFSET + RSN_NONCE_LEN;
pub const FT_FIXED_ELEMENT_LEN: usize = FT_SNONCE_OFFSET + RSN_NONCE_LEN;
pub const FT_FIXED_BODY_LEN: usize = FT_FIXED_ELEMENT_LEN - ELEMENT_HEADER_LEN;
pub const FT_BASE_PROTECTED_ELEMENT_COUNT: u8 = 3;
const MD_OVER_DS: u8 = 1 << 0;
const MD_RESOURCE_REQUEST: u8 = 1 << 1;
const FT_RSNXE_USED: u8 = 1 << 0;

pub mod subelement_id {
    pub const R1KH: u8 = 1;
    pub const GTK: u8 = 2;
    pub const R0KH: u8 = 3;
    pub const IGTK: u8 = 4;
    pub const OCI: u8 = 5;
    pub const BIGTK: u8 = 6;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MobilityDomainId(pub [u8; MOBILITY_DOMAIN_ID_LEN]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct R1khId(pub [u8; R1KH_ID_LEN]);

/// A validated, binary R0KH identity; no string normalization is performed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct R0khId {
    bytes: [u8; R0KH_ID_MAX_LEN],
    len: u8,
}
impl R0khId {
    pub fn new(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > R0KH_ID_MAX_LEN {
            return Err(WireError::InvalidElement(subelement_id::R0KH));
        }
        let mut owned = Self {
            bytes: [0; R0KH_ID_MAX_LEN],
            len: bytes.len() as u8,
        };
        owned.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(owned)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MobilityDomain {
    pub id: MobilityDomainId,
    pub over_ds: bool,
    pub resource_request: bool,
}
impl MobilityDomain {
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        let body = element(bytes, MOBILITY_DOMAIN_ELEMENT_ID)?.body;
        let [a, b, flags] = body else {
            return Err(WireError::InvalidElement(MOBILITY_DOMAIN_ELEMENT_ID));
        };
        Ok(Self {
            id: MobilityDomainId([*a, *b]),
            over_ds: flags & MD_OVER_DS != 0,
            resource_request: flags & MD_RESOURCE_REQUEST != 0,
        })
    }
    pub fn encode(self) -> [u8; ELEMENT_HEADER_LEN + MOBILITY_DOMAIN_ID_LEN + 1] {
        [
            MOBILITY_DOMAIN_ELEMENT_ID,
            (MOBILITY_DOMAIN_ID_LEN + 1) as u8,
            self.id.0[0],
            self.id.0[1],
            (u8::from(self.over_ds) * MD_OVER_DS)
                | (u8::from(self.resource_request) * MD_RESOURCE_REQUEST),
        ]
    }
}

/// Borrowed complete FT element. Unknown subelements remain in `encoded()`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FastTransitionElement<'a> {
    encoded: &'a [u8],
    subelements: Elements<'a>,
}
impl<'a> FastTransitionElement<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let value = element(bytes, FAST_TRANSITION_ELEMENT_ID)?;
        if bytes.len() < FT_FIXED_ELEMENT_LEN {
            return Err(WireError::InvalidElement(FAST_TRANSITION_ELEMENT_ID));
        }
        let subelements = Elements::parse(&value.body[FT_FIXED_BODY_LEN..])?;
        for id in [
            subelement_id::R0KH,
            subelement_id::R1KH,
            subelement_id::GTK,
            subelement_id::IGTK,
            subelement_id::OCI,
            subelement_id::BIGTK,
        ] {
            subelements.unique(id)?;
        }
        if let Some(r0kh) = subelements.unique(subelement_id::R0KH)? {
            R0khId::new(r0kh.body)?;
        }
        if let Some(r1kh) = subelements.unique(subelement_id::R1KH)?
            && r1kh.body.len() != R1KH_ID_LEN
        {
            return Err(WireError::InvalidElement(subelement_id::R1KH));
        }
        Ok(Self {
            encoded: bytes,
            subelements,
        })
    }
    pub const fn encoded(self) -> &'a [u8] {
        self.encoded
    }
    pub fn element_count(self) -> u8 {
        self.encoded[ELEMENT_HEADER_LEN + 1]
    }
    pub fn rsnxe_used(self) -> bool {
        self.encoded[ELEMENT_HEADER_LEN] & FT_RSNXE_USED != 0
    }
    pub fn mic(self) -> &'a [u8; FT_MIC_LEN] {
        self.encoded[FT_MIC_OFFSET..FT_ANONCE_OFFSET]
            .try_into()
            .expect("validated FT MIC")
    }
    pub fn anonce(self) -> &'a [u8; RSN_NONCE_LEN] {
        self.encoded[FT_ANONCE_OFFSET..FT_SNONCE_OFFSET]
            .try_into()
            .expect("validated FT ANonce")
    }
    pub fn snonce(self) -> &'a [u8; RSN_NONCE_LEN] {
        self.encoded[FT_SNONCE_OFFSET..FT_FIXED_ELEMENT_LEN]
            .try_into()
            .expect("validated FT SNonce")
    }
    pub const fn subelements(self) -> Elements<'a> {
        self.subelements
    }
    pub fn r0kh(self) -> Result<Option<R0khId>, WireError> {
        self.subelements
            .unique(subelement_id::R0KH)?
            .map(|value| R0khId::new(value.body))
            .transpose()
    }
    pub fn r1kh(self) -> Result<Option<R1khId>, WireError> {
        Ok(self
            .subelements
            .unique(subelement_id::R1KH)?
            .map(|value| R1khId(value.body.try_into().expect("validated R1KH identity"))))
    }
}

#[derive(Clone, Copy)]
pub struct FastTransitionFields<'a> {
    pub element_count: u8,
    pub rsnxe_used: bool,
    pub mic: [u8; FT_MIC_LEN],
    pub anonce: [u8; RSN_NONCE_LEN],
    pub snonce: [u8; RSN_NONCE_LEN],
    pub subelements: Elements<'a>,
}
impl FastTransitionFields<'_> {
    pub fn encode(self, bytes: &mut [u8]) -> Result<usize, WireError> {
        let length = FT_FIXED_ELEMENT_LEN + self.subelements.as_bytes().len();
        if length - ELEMENT_HEADER_LEN > MAX_ELEMENT_BODY_LEN {
            return Err(WireError::InvalidElement(FAST_TRANSITION_ELEMENT_ID));
        }
        // Validate known singleton lengths before modifying caller storage.
        for id in [
            subelement_id::R0KH,
            subelement_id::R1KH,
            subelement_id::GTK,
            subelement_id::IGTK,
            subelement_id::OCI,
            subelement_id::BIGTK,
        ] {
            self.subelements.unique(id)?;
        }
        if let Some(value) = self.subelements.unique(subelement_id::R0KH)? {
            R0khId::new(value.body)?;
        }
        if let Some(value) = self.subelements.unique(subelement_id::R1KH)?
            && value.body.len() != R1KH_ID_LEN
        {
            return Err(WireError::InvalidElement(subelement_id::R1KH));
        }
        let bytes = output(bytes, length)?;
        bytes[0] = FAST_TRANSITION_ELEMENT_ID;
        bytes[1] = (length - ELEMENT_HEADER_LEN) as u8;
        bytes[ELEMENT_HEADER_LEN] = u8::from(self.rsnxe_used) * FT_RSNXE_USED;
        bytes[ELEMENT_HEADER_LEN + 1] = self.element_count;
        bytes[FT_MIC_OFFSET..FT_ANONCE_OFFSET].copy_from_slice(&self.mic);
        bytes[FT_ANONCE_OFFSET..FT_SNONCE_OFFSET].copy_from_slice(&self.anonce);
        bytes[FT_SNONCE_OFFSET..FT_FIXED_ELEMENT_LEN].copy_from_slice(&self.snonce);
        bytes[FT_FIXED_ELEMENT_LEN..].copy_from_slice(self.subelements.as_bytes());
        Ok(length)
    }
}

/// Timeout Interval type in the FT exchange (the value is in TUs).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TimeoutKind {
    ReassociationDeadline = 1,
    KeyLifetime = 2,
}

/// Complete RIC block: RDIEs followed by their counted resource descriptors.
/// Resource admission remains outside wire syntax.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RicElements<'a> {
    elements: Elements<'a>,
}
impl<'a> RicElements<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let elements = Elements::parse(bytes)?;
        let mut iter = elements.iter();
        let mut identifiers = [false; u8::MAX as usize + 1];
        while let Some(rdie) = iter.next() {
            if rdie.id != RIC_DATA_ELEMENT_ID {
                return Err(WireError::InconsistentFields);
            }
            let [id, count, _, _] = rdie.body else {
                return Err(WireError::InvalidElement(RIC_DATA_ELEMENT_ID));
            };
            if core::mem::replace(&mut identifiers[usize::from(*id)], true) {
                return Err(WireError::InconsistentFields);
            }
            for _ in 0..*count {
                let descriptor = iter.next().ok_or(WireError::InconsistentFields)?;
                if [
                    RSN_ELEMENT_ID,
                    RSNXE_ELEMENT_ID,
                    MOBILITY_DOMAIN_ELEMENT_ID,
                    FAST_TRANSITION_ELEMENT_ID,
                    TIMEOUT_INTERVAL_ELEMENT_ID,
                    RIC_DATA_ELEMENT_ID,
                ]
                .contains(&descriptor.id)
                {
                    return Err(WireError::InconsistentFields);
                }
            }
        }
        Ok(Self { elements })
    }
    pub const fn elements(self) -> Elements<'a> {
        self.elements
    }
}

/// The two FT timeout types have different units on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutInterval {
    ReassociationDeadline { tu: u32 },
    KeyLifetime { seconds: u32 },
}
impl TimeoutInterval {
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        let [kind, a, b, c, d] = element(bytes, TIMEOUT_INTERVAL_ELEMENT_ID)?.body else {
            return Err(WireError::InvalidElement(TIMEOUT_INTERVAL_ELEMENT_ID));
        };
        let value = u32::from_le_bytes([*a, *b, *c, *d]);
        match *kind {
            value_kind if value_kind == TimeoutKind::ReassociationDeadline as u8 => {
                Ok(Self::ReassociationDeadline { tu: value })
            }
            value_kind if value_kind == TimeoutKind::KeyLifetime as u8 => {
                Ok(Self::KeyLifetime { seconds: value })
            }
            _ => Err(WireError::InvalidElement(TIMEOUT_INTERVAL_ELEMENT_ID)),
        }
    }
    pub fn encode(self) -> [u8; ELEMENT_HEADER_LEN + 1 + size_of::<u32>()] {
        let (kind, value) = match self {
            Self::ReassociationDeadline { tu } => (TimeoutKind::ReassociationDeadline, tu),
            Self::KeyLifetime { seconds } => (TimeoutKind::KeyLifetime, seconds),
        };
        let [a, b, c, d] = value.to_le_bytes();
        [
            TIMEOUT_INTERVAL_ELEMENT_ID,
            (1 + size_of::<u32>()) as u8,
            kind as u8,
            a,
            b,
            c,
            d,
        ]
    }
}

/// Fields whose exact encoded bytes take part in the FT MIC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtElements<'a> {
    pub rsn: &'a [u8],
    pub md: &'a [u8],
    pub ft: FastTransitionElement<'a>,
    pub ric: &'a [u8],
    pub rsnxe: &'a [u8],
    pub reassociation_deadline_tu: Option<u32>,
    pub key_lifetime_seconds: Option<u32>,
}
impl<'a> FtElements<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        let elements = Elements::parse(bytes)?;
        let required = |id| -> Result<&'a [u8], WireError> {
            Ok(elements
                .unique(id)?
                .ok_or(WireError::MissingElement(id))?
                .encoded())
        };
        let rsn = required(RSN_ELEMENT_ID)?;
        RsnElement::parse(rsn).map_err(|_| WireError::InvalidElement(RSN_ELEMENT_ID))?;
        let md = required(MOBILITY_DOMAIN_ELEMENT_ID)?;
        MobilityDomain::parse(md)?;
        let ft = FastTransitionElement::parse(required(FAST_TRANSITION_ELEMENT_ID)?)?;
        let rsnxe = elements
            .unique(RSNXE_ELEMENT_ID)?
            .map_or(&[][..], |element| element.encoded());
        if !rsnxe.is_empty()
            && (rsnxe.len() < ELEMENT_HEADER_LEN + 1
                || usize::from(rsnxe[ELEMENT_HEADER_LEN] & crate::security::rsn::RSNXE_LENGTH_MASK)
                    + 1
                    != rsnxe.len() - ELEMENT_HEADER_LEN)
        {
            return Err(WireError::InvalidElement(RSNXE_ELEMENT_ID));
        }
        let mut reassociation_deadline_tu = None;
        let mut key_lifetime_seconds = None;
        for interval in elements
            .iter()
            .filter(|element| element.id == TIMEOUT_INTERVAL_ELEMENT_ID)
        {
            let interval = TimeoutInterval::parse(interval.encoded())?;
            let (destination, value) = match interval {
                TimeoutInterval::ReassociationDeadline { tu } => {
                    (&mut reassociation_deadline_tu, tu)
                }
                TimeoutInterval::KeyLifetime { seconds } => (&mut key_lifetime_seconds, seconds),
            };
            if destination.replace(value).is_some() {
                return Err(WireError::Elements(ElementError::Duplicate(
                    TIMEOUT_INTERVAL_ELEMENT_ID,
                )));
            }
        }
        // A RIC block consists of RDIEs and their counted resource descriptors.
        // Its semantic admission belongs to the caller's resource owner.
        let mut ric = &[][..];
        let mut offset = 0;
        let mut iter = elements.iter();
        while let Some(value) = iter.next() {
            if value.id == RIC_DATA_ELEMENT_ID {
                let start = offset;
                if !ric.is_empty() {
                    return Err(WireError::InconsistentFields);
                }
                let mut descriptor = value;
                loop {
                    let [_, count, _, _] = descriptor.body else {
                        return Err(WireError::InvalidElement(RIC_DATA_ELEMENT_ID));
                    };
                    offset += descriptor.encoded().len();
                    for _ in 0..*count {
                        let resource = iter.next().ok_or(WireError::InconsistentFields)?;
                        if [
                            RSN_ELEMENT_ID,
                            MOBILITY_DOMAIN_ELEMENT_ID,
                            FAST_TRANSITION_ELEMENT_ID,
                            RSNXE_ELEMENT_ID,
                            RIC_DATA_ELEMENT_ID,
                        ]
                        .contains(&resource.id)
                        {
                            return Err(WireError::InconsistentFields);
                        }
                        offset += resource.encoded().len();
                    }
                    if bytes.get(offset) != Some(&RIC_DATA_ELEMENT_ID) {
                        break;
                    }
                    descriptor = iter.next().expect("validated contiguous RDIE");
                }
                ric = &bytes[start..offset];
            } else {
                offset += value.encoded().len();
            }
        }
        RicElements::parse(ric)?;
        let result = Self {
            rsn,
            md,
            ft,
            ric,
            rsnxe,
            reassociation_deadline_tu,
            key_lifetime_seconds,
        };
        if ft.element_count() != 0 && ft.element_count() != result.protected_element_count()? {
            return Err(WireError::InconsistentFields);
        }
        Ok(result)
    }
    pub fn protected_element_count(self) -> Result<u8, WireError> {
        let ric_count = Elements::parse(self.ric)?.iter().count();
        u8::try_from(
            usize::from(FT_BASE_PROTECTED_ELEMENT_COUNT)
                + ric_count
                + usize::from(!self.rsnxe.is_empty()),
        )
        .map_err(|_| WireError::InconsistentFields)
    }
}

/// The two addresses authenticated by an FT MIC, in their standard order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtAddresses {
    pub station: MacAddress,
    pub access_point: MacAddress,
}
