//! TCLAS wire formats shared by TFS, WNM Sleep and DMS.
use super::codec::{BodyReader, BodyWriter};
use super::management::encode_element;
use super::{MAC_ADDRESS_LEN, MAX_ELEMENT_BODY_LEN, MacAddress, WireError, element_id};
use core::mem::size_of;
pub const IPV4_ADDRESS_LEN: usize = 4;
pub const IPV6_ADDRESS_LEN: usize = 16;
const FLOW_LABEL_LEN: usize = 3;
const IPV4_VERSION: u8 = 4;
const IPV6_VERSION: u8 = 6;
const RULE_FIXED_LEN: usize = 3 * size_of::<u8>();
const ETHERNET_LEN: usize = 2 * MAC_ADDRESS_LEN + size_of::<u16>();
const IPV4_LEN: usize = 2 * IPV4_ADDRESS_LEN + 2 * size_of::<u16>() + 4 * size_of::<u8>();
const IPV6_BASE_LEN: usize =
    size_of::<u8>() + 2 * IPV6_ADDRESS_LEN + 2 * size_of::<u16>() + FLOW_LABEL_LEN;
const IPV6_HIGHER_LEN: usize = IPV6_BASE_LEN + 2 * size_of::<u8>();
const VLAN_LEN: usize = 2 * size_of::<u8>() + size_of::<u16>();
pub mod classifier_type {
    pub const ETHERNET: u8 = 0;
    pub const IP: u8 = 1;
    pub const VLAN_TCI: u8 = 2;
    pub const FILTER: u8 = 3;
    pub const IP_HIGHER_LAYER: u8 = 4;
    pub const VLAN: u8 = 5;
}
pub mod ethernet_fields {
    pub const SOURCE: u8 = 1 << 0;
    pub const DESTINATION: u8 = 1 << 1;
    pub const ETHER_TYPE: u8 = 1 << 2;
    pub const ALL: u8 = SOURCE | DESTINATION | ETHER_TYPE;
}
pub mod ip_fields {
    pub const VERSION: u8 = 1 << 0;
    pub const SOURCE: u8 = 1 << 1;
    pub const DESTINATION: u8 = 1 << 2;
    pub const SOURCE_PORT: u8 = 1 << 3;
    pub const DESTINATION_PORT: u8 = 1 << 4;
    pub const DSCP: u8 = 1 << 5;
    pub const PROTOCOL: u8 = 1 << 6;
    pub const BASIC_IPV6_FLOW_LABEL: u8 = 1 << 5;
    pub const HIGHER_IPV6_FLOW_LABEL: u8 = 1 << 7;
    pub const PORTS: u8 = SOURCE_PORT | DESTINATION_PORT;
    pub const DESTINATION_REQUIRED: u8 = VERSION | DESTINATION;
    pub const IPV4_ALL: u8 = VERSION | SOURCE | DESTINATION | PORTS | DSCP | PROTOCOL;
    pub const BASIC_IPV6_ALL: u8 = VERSION | SOURCE | DESTINATION | PORTS | BASIC_IPV6_FLOW_LABEL;
}
pub mod vlan_fields {
    pub const TCI: u8 = 1;
    pub const PCP: u8 = 1;
    pub const CFI: u8 = 2;
    pub const VID: u8 = 4;
}
pub mod tclas_processing {
    pub const ALL: u8 = 0;
    pub const ANY: u8 = 1;
    pub const fn supported(value: u8) -> bool {
        matches!(value, ALL | ANY)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TclasParameters<'a> {
    VlanTci(u16),
    Filter {
        offset: u16,
        value: &'a [u8],
        mask: &'a [u8],
    },
    Vlan {
        pcp: u8,
        cfi: u8,
        vid: u16,
    },
    Ethernet {
        source: MacAddress,
        destination: MacAddress,
        ether_type: u16,
    },
    Ipv4 {
        kind: u8,
        source: [u8; IPV4_ADDRESS_LEN],
        destination: [u8; IPV4_ADDRESS_LEN],
        source_port: u16,
        destination_port: u16,
        dscp: u8,
        protocol: u8,
        reserved: u8,
    },
    Ipv6 {
        kind: u8,
        source: [u8; IPV6_ADDRESS_LEN],
        destination: [u8; IPV6_ADDRESS_LEN],
        source_port: u16,
        destination_port: u16,
        dscp: Option<u8>,
        next_header: Option<u8>,
        flow_label: [u8; FLOW_LABEL_LEN],
    },
    Other {
        kind: u8,
        body: &'a [u8],
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TclasRule<'a> {
    pub user_priority: u8,
    pub mask: u8,
    pub parameters: TclasParameters<'a>,
}
impl<'a> TclasRule<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        if body.len() < RULE_FIXED_LEN {
            return Err(WireError::InvalidElementLength(element_id::TCLAS));
        }
        let mut fields = BodyReader::new(body);
        let user_priority = fields.u8()?;
        let kind = fields.u8()?;
        let mask = fields.u8()?;
        let parameters = match kind {
            classifier_type::VLAN_TCI => {
                if fields.remaining().len() != size_of::<u16>() {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                TclasParameters::VlanTci(u16::from_be_bytes(fields.array()?))
            }
            classifier_type::FILTER => {
                if fields.remaining().len() < size_of::<u16>() {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                let offset = u16::from_le_bytes(fields.array()?);
                if !fields.remaining().len().is_multiple_of(2) {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                let half = fields.remaining().len() / 2;
                let value = fields.take(half)?;
                TclasParameters::Filter {
                    offset,
                    value,
                    mask: fields.remaining(),
                }
            }
            classifier_type::VLAN => {
                if fields.remaining().len() != VLAN_LEN {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                TclasParameters::Vlan {
                    pcp: fields.u8()?,
                    cfi: fields.u8()?,
                    vid: u16::from_be_bytes(fields.array()?),
                }
            }
            classifier_type::ETHERNET => {
                if fields.remaining().len() != ETHERNET_LEN {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                TclasParameters::Ethernet {
                    source: fields.array()?,
                    destination: fields.array()?,
                    ether_type: u16::from_be_bytes(fields.array()?),
                }
            }
            classifier_type::IP | classifier_type::IP_HIGHER_LAYER
                if fields.remaining().first() == Some(&IPV4_VERSION) =>
            {
                if fields.remaining().len() != IPV4_LEN {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                fields.u8()?;
                TclasParameters::Ipv4 {
                    kind,
                    source: fields.array()?,
                    destination: fields.array()?,
                    source_port: u16::from_be_bytes(fields.array()?),
                    destination_port: u16::from_be_bytes(fields.array()?),
                    dscp: fields.u8()?,
                    protocol: fields.u8()?,
                    reserved: fields.u8()?,
                }
            }
            classifier_type::IP | classifier_type::IP_HIGHER_LAYER
                if fields.remaining().first() == Some(&IPV6_VERSION) =>
            {
                let expected = if kind == classifier_type::IP {
                    IPV6_BASE_LEN
                } else {
                    IPV6_HIGHER_LEN
                };
                if fields.remaining().len() != expected {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                fields.u8()?;
                let source = fields.array()?;
                let destination = fields.array()?;
                let source_port = u16::from_be_bytes(fields.array()?);
                let destination_port = u16::from_be_bytes(fields.array()?);
                let (dscp, next_header) = if kind == classifier_type::IP_HIGHER_LAYER {
                    (Some(fields.u8()?), Some(fields.u8()?))
                } else {
                    (None, None)
                };
                TclasParameters::Ipv6 {
                    kind,
                    source,
                    destination,
                    source_port,
                    destination_port,
                    dscp,
                    next_header,
                    flow_label: fields.array()?,
                }
            }
            kind => {
                if fields.remaining().is_empty() {
                    return Err(WireError::InvalidElementLength(element_id::TCLAS));
                }
                TclasParameters::Other {
                    kind,
                    body: fields.remaining(),
                }
            }
        };
        Ok(Self {
            user_priority,
            mask,
            parameters,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let (kind, len) = match self.parameters {
            TclasParameters::VlanTci(_) => {
                (classifier_type::VLAN_TCI, RULE_FIXED_LEN + size_of::<u16>())
            }
            TclasParameters::Filter { value, mask, .. } if value.len() == mask.len() => {
                let len = RULE_FIXED_LEN + size_of::<u16>() + value.len() + mask.len();
                if len > MAX_ELEMENT_BODY_LEN {
                    return Err(WireError::InconsistentFields);
                }
                (classifier_type::FILTER, len)
            }
            TclasParameters::Vlan { .. } => (classifier_type::VLAN, RULE_FIXED_LEN + VLAN_LEN),
            TclasParameters::Ethernet { .. } => {
                (classifier_type::ETHERNET, RULE_FIXED_LEN + ETHERNET_LEN)
            }
            TclasParameters::Ipv4 {
                kind: kind @ (classifier_type::IP | classifier_type::IP_HIGHER_LAYER),
                ..
            } => (kind, RULE_FIXED_LEN + IPV4_LEN),
            TclasParameters::Ipv6 {
                kind: classifier_type::IP,
                dscp: None,
                next_header: None,
                ..
            } => (classifier_type::IP, RULE_FIXED_LEN + IPV6_BASE_LEN),
            TclasParameters::Ipv6 {
                kind: classifier_type::IP_HIGHER_LAYER,
                dscp: Some(_),
                next_header: Some(_),
                ..
            } => (
                classifier_type::IP_HIGHER_LAYER,
                RULE_FIXED_LEN + IPV6_HIGHER_LEN,
            ),
            TclasParameters::Other { kind, body }
                if !body.is_empty() && body.len() <= MAX_ELEMENT_BODY_LEN - RULE_FIXED_LEN =>
            {
                let mut validated = [0; MAX_ELEMENT_BODY_LEN];
                let mut fields = BodyWriter::new(&mut validated);
                fields.put(&[self.user_priority, kind, self.mask]);
                fields.put(body);
                TclasRule::parse(&validated[..RULE_FIXED_LEN + body.len()])?;
                (kind, RULE_FIXED_LEN + body.len())
            }
            _ => return Err(WireError::InconsistentFields),
        };
        // All fallible field validation precedes output admission.
        let mut body = [0; MAX_ELEMENT_BODY_LEN];
        let mut fields = BodyWriter::new(&mut body[..len]);
        fields.put(&[self.user_priority, kind, self.mask]);
        match self.parameters {
            TclasParameters::VlanTci(tci) => fields.put(&tci.to_be_bytes()),
            TclasParameters::Filter {
                offset,
                value,
                mask,
            } => {
                fields.put(&offset.to_le_bytes());
                fields.put(value);
                fields.put(mask);
            }
            TclasParameters::Vlan { pcp, cfi, vid } => {
                fields.put(&[pcp, cfi]);
                fields.put(&vid.to_be_bytes());
            }
            TclasParameters::Ethernet {
                source,
                destination,
                ether_type,
            } => {
                fields.put(&source);
                fields.put(&destination);
                fields.put(&ether_type.to_be_bytes());
            }
            TclasParameters::Ipv4 {
                source,
                destination,
                source_port,
                destination_port,
                dscp,
                protocol,
                reserved,
                ..
            } => {
                fields.put(&[IPV4_VERSION]);
                fields.put(&source);
                fields.put(&destination);
                fields.put(&source_port.to_be_bytes());
                fields.put(&destination_port.to_be_bytes());
                fields.put(&[dscp, protocol, reserved]);
            }
            TclasParameters::Ipv6 {
                source,
                destination,
                source_port,
                destination_port,
                dscp,
                next_header,
                flow_label,
                ..
            } => {
                fields.put(&[IPV6_VERSION]);
                fields.put(&source);
                fields.put(&destination);
                fields.put(&source_port.to_be_bytes());
                fields.put(&destination_port.to_be_bytes());
                if let (Some(dscp), Some(next)) = (dscp, next_header) {
                    fields.put(&[dscp, next]);
                }
                fields.put(&flow_label);
            }
            TclasParameters::Other { body, .. } => fields.put(body),
        }
        encode_element(element_id::TCLAS, &body[..len], buffer)
    }
}

/// Validate TCLAS and Processing syntax while retaining other complete IEs.
/// Whether Processing is required is decided by the enclosing service.
pub fn validate_tclas_elements(elements: super::Elements<'_>) -> Result<(), WireError> {
    for element in elements
        .iter()
        .filter(|element| element.id == element_id::TCLAS)
    {
        TclasRule::parse(element.body)?;
    }
    if elements
        .unique(element_id::TCLAS_PROCESSING)?
        .is_some_and(|body| body.len() != 1)
    {
        return Err(WireError::InvalidElementLength(
            element_id::TCLAS_PROCESSING,
        ));
    }
    Ok(())
}
