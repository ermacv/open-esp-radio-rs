//! Normalized packet facts and network classifier matching shared by DMS/TFS.
//! Matching carries no subscription, delivery or resource-admission policy.
use oer_ieee80211_mac::data::{ETHER_TYPE_IPV4, ETHER_TYPE_IPV6};
use oer_ieee80211_mac::roaming::{
    IPV4_ADDRESS_LEN, IPV6_ADDRESS_LEN, MacAddress, TclasParameters, TclasRule, classifier_type,
    ethernet_fields, ip_fields,
};
// The TCLAS DSCP and flow-label fields retain reserved high bits on the wire;
// comparison ignores them (IEEE 802.11-2012, 8.4.2.33).
const DSCP_MASK: u8 = 0x3f;
const FLOW_LABEL_MASK: u32 = 0x000f_ffff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ports {
    pub source: u16,
    pub destination: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpFields {
    V4 {
        source: [u8; IPV4_ADDRESS_LEN],
        destination: [u8; IPV4_ADDRESS_LEN],
        dscp: u8,
        protocol: u8,
        ports: Option<Ports>,
    },
    V6 {
        source: [u8; IPV6_ADDRESS_LEN],
        destination: [u8; IPV6_ADDRESS_LEN],
        dscp: u8,
        next_header: u8,
        flow_label: u32,
        ports: Option<Ports>,
        extension_headers: bool,
    },
}
/// Normalized original MSDU fields supplied by the existing packet decoder.
/// Port data is absent for noninitial fragments and unsupported headers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassifierPacket {
    pub source: MacAddress,
    pub destination: MacAddress,
    pub ether_type: u16,
    pub ip: Option<IpFields>,
}

fn field(mask: u8, selected: u8, equal: bool) -> bool {
    mask & selected == 0 || equal
}
pub(crate) fn matches_network_rule(rule: TclasRule<'_>, packet: ClassifierPacket) -> bool {
    let mask = rule.mask;
    match (rule.parameters, packet.ip) {
        (
            TclasParameters::Ethernet {
                source,
                destination,
                ether_type,
            },
            _,
        ) => {
            field(mask, ethernet_fields::SOURCE, source == packet.source)
                && field(
                    mask,
                    ethernet_fields::DESTINATION,
                    destination == packet.destination,
                )
                && field(
                    mask,
                    ethernet_fields::ETHER_TYPE,
                    ether_type == packet.ether_type,
                )
        }
        (
            TclasParameters::Ipv4 {
                source,
                destination,
                source_port,
                destination_port,
                dscp,
                protocol,
                ..
            },
            Some(IpFields::V4 {
                source: src,
                destination: dst,
                dscp: ds,
                protocol: proto,
                ports,
            }),
        ) => {
            packet.ether_type == ETHER_TYPE_IPV4
                && field(mask, ip_fields::SOURCE, source == src)
                && field(mask, ip_fields::DESTINATION, destination == dst)
                && field(
                    mask,
                    ip_fields::SOURCE_PORT,
                    ports.is_some_and(|ports| ports.source == source_port),
                )
                && field(
                    mask,
                    ip_fields::DESTINATION_PORT,
                    ports.is_some_and(|ports| ports.destination == destination_port),
                )
                && field(mask, ip_fields::DSCP, dscp & DSCP_MASK == ds & DSCP_MASK)
                && field(mask, ip_fields::PROTOCOL, protocol == proto)
        }
        (
            TclasParameters::Ipv6 {
                kind,
                source,
                destination,
                source_port,
                destination_port,
                dscp,
                next_header,
                flow_label,
            },
            Some(IpFields::V6 {
                source: src,
                destination: dst,
                dscp: ds,
                next_header: next,
                flow_label: flow,
                ports,
                extension_headers,
            }),
        ) => {
            if packet.ether_type != ETHER_TYPE_IPV6
                || (extension_headers && mask & ip_fields::PORTS != 0)
            {
                return false;
            }
            let label = u32::from_be_bytes([0, flow_label[0], flow_label[1], flow_label[2]])
                & FLOW_LABEL_MASK;
            field(mask, ip_fields::SOURCE, source == src)
                && field(mask, ip_fields::DESTINATION, destination == dst)
                && field(
                    mask,
                    ip_fields::SOURCE_PORT,
                    ports.is_some_and(|ports| ports.source == source_port),
                )
                && field(
                    mask,
                    ip_fields::DESTINATION_PORT,
                    ports.is_some_and(|ports| ports.destination == destination_port),
                )
                && if kind == classifier_type::IP {
                    field(mask, ip_fields::BASIC_IPV6_FLOW_LABEL, label == flow)
                } else {
                    field(
                        mask,
                        ip_fields::DSCP,
                        dscp.is_some_and(|dscp| dscp & DSCP_MASK == ds & DSCP_MASK),
                    ) && field(mask, ip_fields::PROTOCOL, next_header == Some(next))
                        && field(mask, ip_fields::HIGHER_IPV6_FLOW_LABEL, label == flow)
                }
        }
        _ => false,
    }
}
