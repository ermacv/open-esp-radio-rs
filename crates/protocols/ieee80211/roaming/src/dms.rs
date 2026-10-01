//! DMS subscriptions, transactions and multicast classification.
use crate::tclas::matches_network_rule;
pub use crate::tclas::{ClassifierPacket as DmsPacket, IpFields, Ports};
use crate::{Body, Error, LinkIdentity, OperationId};
use oer_ieee80211_mac::management::is_group_address;
use oer_ieee80211_mac::qos::WmmUserPriority;
use oer_ieee80211_mac::roaming::*;
mod ap;
mod registry;
mod station;
pub use ap::*;
pub use registry::*;
pub use station::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssIdentity {
    pub bssid: oer_ieee80211_mac::roaming::MacAddress,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmsError {
    Protocol(Error),
    Full,
    OperationCapacity,
    UnknownStream(u8),
    ConflictingStream(u8),
    ConflictingRequest,
    UnsupportedRequest(u8),
    UnsupportedResponse(u8),
    UnsupportedClassifier(u8),
    UnsupportedMask(u8),
    UnsupportedPriority(u8),
    UnsupportedProcessing(u8),
    NotMulticast,
    MissingDestination,
    NoChange,
    StalePeer,
    StaleTransaction,
    UnexpectedResponse,
    IncompleteResponse,
    InvalidSequence,
    AmbiguousSequence,
    RecoveryRequired,
}
impl From<Error> for DmsError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
impl From<WireError> for DmsError {
    fn from(value: WireError) -> Self {
        Self::Protocol(Error::Wire(value))
    }
}

pub fn admit_classifiers(attributes: Elements<'_>) -> Result<(), DmsError> {
    validate_dms_attributes(attributes)?;
    let mut count = 0;
    for element in attributes
        .iter()
        .filter(|element| element.id == element_id::TCLAS)
    {
        let rule = TclasRule::parse(element.body)?;
        if WmmUserPriority::new(rule.user_priority).is_none() {
            return Err(DmsError::UnsupportedPriority(rule.user_priority));
        }
        match rule.parameters {
            TclasParameters::Ethernet { destination, .. } => {
                if rule.mask & !ethernet_fields::ALL != 0 {
                    return Err(DmsError::UnsupportedMask(rule.mask));
                }
                if rule.mask & ethernet_fields::DESTINATION == 0 {
                    return Err(DmsError::MissingDestination);
                }
                if !is_group_address(destination) {
                    return Err(DmsError::NotMulticast);
                }
            }
            TclasParameters::Ipv4 { destination, .. } => {
                if rule.mask & !ip_fields::IPV4_ALL != 0 {
                    return Err(DmsError::UnsupportedMask(rule.mask));
                }
                if rule.mask & ip_fields::DESTINATION_REQUIRED != ip_fields::DESTINATION_REQUIRED {
                    return Err(DmsError::MissingDestination);
                }
                if !core::net::Ipv4Addr::from(destination).is_multicast() {
                    return Err(DmsError::NotMulticast);
                }
            }
            TclasParameters::Ipv6 {
                kind, destination, ..
            } => {
                if kind == classifier_type::IP && rule.mask & !ip_fields::BASIC_IPV6_ALL != 0 {
                    return Err(DmsError::UnsupportedMask(rule.mask));
                }
                if rule.mask & ip_fields::DESTINATION_REQUIRED != ip_fields::DESTINATION_REQUIRED {
                    return Err(DmsError::MissingDestination);
                }
                if !core::net::Ipv6Addr::from(destination).is_multicast() {
                    return Err(DmsError::NotMulticast);
                }
            }
            TclasParameters::Other { kind, .. } => {
                return Err(DmsError::UnsupportedClassifier(kind));
            }
            TclasParameters::VlanTci(_) => {
                return Err(DmsError::UnsupportedClassifier(classifier_type::VLAN_TCI));
            }
            TclasParameters::Filter { .. } => {
                return Err(DmsError::UnsupportedClassifier(classifier_type::FILTER));
            }
            TclasParameters::Vlan { .. } => {
                return Err(DmsError::UnsupportedClassifier(classifier_type::VLAN));
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err(DmsError::MissingDestination);
    }
    if let Some([processing]) = attributes.unique(element_id::TCLAS_PROCESSING)?
        && !tclas_processing::supported(*processing)
    {
        return Err(DmsError::UnsupportedProcessing(*processing));
    }
    Ok(())
}

/// Match the multicast MSDU for AP delivery. IPv6 extension headers do not
/// support port classification (IEEE 802.11, TCLAS type 4); absent ports never
/// manufacture a match. DSCP/flow-label reserved high bits are ignored as defined.
pub fn matches_classifiers(attributes: Elements<'_>, packet: DmsPacket) -> Result<bool, DmsError> {
    admit_classifiers(attributes)?;
    if !is_group_address(packet.destination) {
        return Ok(false);
    }
    let any = attributes
        .unique(element_id::TCLAS_PROCESSING)?
        .is_some_and(|value| value[0] == tclas_processing::ANY);
    let mut matched = !any;
    for element in attributes
        .iter()
        .filter(|element| element.id == element_id::TCLAS)
    {
        let rule = TclasRule::parse(element.body)?;
        let result = matches_network_rule(rule, packet);
        if any {
            matched |= result;
        } else {
            matched &= result;
        }
    }
    Ok(matched)
}
// The STA's accepted group-address list ignores source/port/QoS parameters,
// as required by the DMS receive procedure, rather than treating it as AP routing.
fn group_address_matches(attributes: Elements<'_>, packet: DmsPacket) -> Result<bool, DmsError> {
    for element in attributes
        .iter()
        .filter(|element| element.id == element_id::TCLAS)
    {
        match (TclasRule::parse(element.body)?.parameters, packet.ip) {
            (TclasParameters::Ethernet { destination, .. }, _)
                if destination == packet.destination =>
            {
                return Ok(true);
            }
            (
                TclasParameters::Ipv4 { destination, .. },
                Some(IpFields::V4 {
                    destination: dst, ..
                }),
            ) if destination == dst => return Ok(true),
            (
                TclasParameters::Ipv6 { destination, .. },
                Some(IpFields::V6 {
                    destination: dst, ..
                }),
            ) if destination == dst => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

fn attributes_body<const BYTES: usize>(attributes: Elements<'_>) -> Result<Body<BYTES>, DmsError> {
    Ok(Body::copy(attributes.as_bytes())?)
}
fn merge_attributes<const BYTES: usize>(
    existing: Elements<'_>,
    changed: Elements<'_>,
) -> Result<Body<BYTES>, DmsError> {
    validate_dms_attributes(changed)?;
    let mut result = Body::<BYTES>::empty();
    for element in existing
        .iter()
        .filter(|element| matches!(element.id, element_id::TCLAS | element_id::TCLAS_PROCESSING))
        .chain(changed.iter())
    {
        let end = result.len + ELEMENT_HEADER_LEN + element.body.len();
        if end > BYTES {
            return Err(Error::FrameTooLarge {
                required: end,
                capacity: BYTES,
            }
            .into());
        }
        result.bytes[result.len..result.len + ELEMENT_HEADER_LEN]
            .copy_from_slice(&[element.id, element.body.len() as u8]);
        result.bytes[result.len + ELEMENT_HEADER_LEN..end].copy_from_slice(element.body);
        result.len = end;
    }
    Ok(result)
}
fn append_status<const BYTES: usize>(
    body: &mut Body<BYTES>,
    outer: &mut Option<usize>,
    status: DmsStatus<'_>,
) -> Result<(), DmsError> {
    status.validate()?;
    let len = DmsStatus::FIXED_ENCODED_LEN + status.attributes.as_bytes().len();
    if len > MAX_ELEMENT_BODY_LEN {
        return Err(WireError::ElementTooLong.into());
    }
    let need_header = outer.is_none_or(|offset| {
        usize::from(body.bytes[offset + ELEMENT_LENGTH_OFFSET]) + len > MAX_ELEMENT_BODY_LEN
    });
    let end = body.len + len + if need_header { ELEMENT_HEADER_LEN } else { 0 };
    if end > BYTES {
        return Err(Error::FrameTooLarge {
            required: end,
            capacity: BYTES,
        }
        .into());
    }
    if need_header {
        let offset = body.len;
        body.bytes[offset..offset + ELEMENT_HEADER_LEN]
            .copy_from_slice(&[element_id::DMS_RESPONSE, 0]);
        body.len += ELEMENT_HEADER_LEN;
        *outer = Some(offset);
    }
    let length = status.encode(&mut body.bytes[body.len..])?;
    body.len += length;
    body.bytes[outer.expect("status element") + ELEMENT_LENGTH_OFFSET] += length as u8;
    Ok(())
}

#[cfg(test)]
mod tests;
