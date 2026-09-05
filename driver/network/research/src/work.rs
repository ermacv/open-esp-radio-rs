use open_esp_radio_wifi_datapath::{DeferredTxWork, EgressFlowKey};

use crate::{
    Ipv4Address, MacAddress, UdpEndpoint,
    checksum::{internet_checksum, udp_ipv4_checksum},
};

const ETHERNET_HEADER_LEN: usize = 14;
const IPV4_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;
const ARP_PACKET_LEN: usize = 28;
const ARP_FRAME_LEN: usize = ETHERNET_HEADER_LEN + ARP_PACKET_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameWriteError {
    WrongDestinationLength,
    PayloadTooLong,
    PayloadLengthChanged,
}

pub(crate) struct Ipv4TxPath {
    pub key: EgressFlowKey,
    pub enqueue_micros: u64,
    pub source_mac: MacAddress,
    pub destination_mac: MacAddress,
    pub source_ip: Ipv4Address,
    pub destination_ip: Ipv4Address,
    pub identification: u16,
}

pub(crate) struct UdpTxWork<Payload> {
    key: EgressFlowKey,
    enqueue_micros: u64,
    source_mac: MacAddress,
    destination_mac: MacAddress,
    source: UdpEndpoint,
    destination: UdpEndpoint,
    identification: u16,
    payload_length: u16,
    payload: Payload,
}

impl<Payload: AsRef<[u8]>> UdpTxWork<Payload> {
    pub(crate) fn new(
        path: Ipv4TxPath,
        source_port: u16,
        destination_port: u16,
        payload: Payload,
    ) -> Result<Self, Payload> {
        let length = payload.as_ref().len();
        if length > usize::from(u16::MAX) - ETHERNET_HEADER_LEN - IPV4_HEADER_LEN - UDP_HEADER_LEN {
            return Err(payload);
        }
        Ok(Self {
            key: path.key,
            enqueue_micros: path.enqueue_micros,
            source_mac: path.source_mac,
            destination_mac: path.destination_mac,
            source: UdpEndpoint {
                address: path.source_ip,
                port: source_port,
            },
            destination: UdpEndpoint {
                address: path.destination_ip,
                port: destination_port,
            },
            identification: path.identification,
            payload_length: length as u16,
            payload,
        })
    }

    pub(crate) fn into_payload(self) -> Payload {
        self.payload
    }

    fn frame_length(&self) -> usize {
        ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN + usize::from(self.payload_length)
    }

    fn write(&self, destination: &mut [u8]) -> Result<(), FrameWriteError> {
        if destination.len() != self.frame_length() {
            return Err(FrameWriteError::WrongDestinationLength);
        }
        let payload = self.payload.as_ref();
        if payload.len() != usize::from(self.payload_length) {
            return Err(FrameWriteError::PayloadLengthChanged);
        }
        write_ethernet_header(destination, self.destination_mac, self.source_mac, 0x0800);
        let ip_total_length = IPV4_HEADER_LEN + UDP_HEADER_LEN + usize::from(self.payload_length);
        write_ipv4_header(
            &mut destination[ETHERNET_HEADER_LEN..ETHERNET_HEADER_LEN + IPV4_HEADER_LEN],
            self.source.address,
            self.destination.address,
            self.identification,
            17,
            ip_total_length,
        );
        let udp_start = ETHERNET_HEADER_LEN + IPV4_HEADER_LEN;
        let payload_start = udp_start + UDP_HEADER_LEN;
        let udp_length = UDP_HEADER_LEN + payload.len();
        destination[udp_start..udp_start + 2].copy_from_slice(&self.source.port.to_be_bytes());
        destination[udp_start + 2..udp_start + 4]
            .copy_from_slice(&self.destination.port.to_be_bytes());
        destination[udp_start + 4..udp_start + 6]
            .copy_from_slice(&(udp_length as u16).to_be_bytes());
        destination[udp_start + 6..udp_start + 8].fill(0);
        destination[payload_start..].copy_from_slice(payload);
        let checksum = udp_ipv4_checksum(
            self.source.address.bytes(),
            self.destination.address.bytes(),
            &destination[udp_start..udp_start + UDP_HEADER_LEN],
            &destination[payload_start..],
        );
        destination[udp_start + 6..udp_start + 8].copy_from_slice(&checksum.to_be_bytes());
        Ok(())
    }
}

pub(crate) struct ArpTxWork {
    key: EgressFlowKey,
    enqueue_micros: u64,
    source_mac: MacAddress,
    source_ip: Ipv4Address,
    destination_mac: MacAddress,
    destination_ip: Ipv4Address,
    operation: u16,
}

impl ArpTxWork {
    pub(crate) fn request(
        key: EgressFlowKey,
        enqueue_micros: u64,
        source_mac: MacAddress,
        source_ip: Ipv4Address,
        destination_ip: Ipv4Address,
    ) -> Self {
        Self {
            key,
            enqueue_micros,
            source_mac,
            source_ip,
            destination_mac: MacAddress::BROADCAST,
            destination_ip,
            operation: 1,
        }
    }

    pub(crate) fn reply(
        key: EgressFlowKey,
        enqueue_micros: u64,
        source_mac: MacAddress,
        source_ip: Ipv4Address,
        destination_mac: MacAddress,
        destination_ip: Ipv4Address,
    ) -> Self {
        Self {
            key,
            enqueue_micros,
            source_mac,
            source_ip,
            destination_mac,
            destination_ip,
            operation: 2,
        }
    }

    fn write(&self, destination: &mut [u8]) -> Result<(), FrameWriteError> {
        if destination.len() != ARP_FRAME_LEN {
            return Err(FrameWriteError::WrongDestinationLength);
        }
        write_ethernet_header(destination, self.destination_mac, self.source_mac, 0x0806);
        let arp = &mut destination[ETHERNET_HEADER_LEN..];
        arp[0..2].copy_from_slice(&1u16.to_be_bytes());
        arp[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
        arp[4] = 6;
        arp[5] = 4;
        arp[6..8].copy_from_slice(&self.operation.to_be_bytes());
        arp[8..14].copy_from_slice(&self.source_mac.bytes());
        arp[14..18].copy_from_slice(&self.source_ip.bytes());
        if self.operation == 1 {
            arp[18..24].fill(0);
        } else {
            arp[18..24].copy_from_slice(&self.destination_mac.bytes());
        }
        arp[24..28].copy_from_slice(&self.destination_ip.bytes());
        Ok(())
    }
}

pub(crate) struct IcmpEchoReplyWork<const PAYLOAD_CAPACITY: usize> {
    key: EgressFlowKey,
    enqueue_micros: u64,
    source_mac: MacAddress,
    destination_mac: MacAddress,
    source_ip: Ipv4Address,
    destination_ip: Ipv4Address,
    identification: u16,
    icmp_length: u16,
    icmp: [u8; PAYLOAD_CAPACITY],
}

impl<const PAYLOAD_CAPACITY: usize> IcmpEchoReplyWork<PAYLOAD_CAPACITY> {
    pub(crate) fn new(path: Ipv4TxPath, request: &[u8]) -> Result<Self, FrameWriteError> {
        if request.len() > PAYLOAD_CAPACITY
            || ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + request.len() > usize::from(u16::MAX)
        {
            return Err(FrameWriteError::PayloadTooLong);
        }
        let mut icmp = [0; PAYLOAD_CAPACITY];
        icmp[..request.len()].copy_from_slice(request);
        Ok(Self {
            key: path.key,
            enqueue_micros: path.enqueue_micros,
            source_mac: path.source_mac,
            destination_mac: path.destination_mac,
            source_ip: path.source_ip,
            destination_ip: path.destination_ip,
            identification: path.identification,
            icmp_length: request.len() as u16,
            icmp,
        })
    }

    fn frame_length(&self) -> usize {
        ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + usize::from(self.icmp_length)
    }

    fn write(&self, destination: &mut [u8]) -> Result<(), FrameWriteError> {
        if destination.len() != self.frame_length() {
            return Err(FrameWriteError::WrongDestinationLength);
        }
        write_ethernet_header(destination, self.destination_mac, self.source_mac, 0x0800);
        write_ipv4_header(
            &mut destination[ETHERNET_HEADER_LEN..ETHERNET_HEADER_LEN + IPV4_HEADER_LEN],
            self.source_ip,
            self.destination_ip,
            self.identification,
            1,
            IPV4_HEADER_LEN + usize::from(self.icmp_length),
        );
        let icmp = &mut destination[ETHERNET_HEADER_LEN + IPV4_HEADER_LEN..];
        icmp.copy_from_slice(&self.icmp[..usize::from(self.icmp_length)]);
        icmp[0] = 0;
        icmp[1] = 0;
        icmp[2..4].fill(0);
        let checksum = internet_checksum(&[icmp]);
        icmp[2..4].copy_from_slice(&checksum.to_be_bytes());
        Ok(())
    }
}

/// Canonical deferred work retained by the research network engine.
pub(crate) enum ResearchTxWork<const PAYLOAD_CAPACITY: usize, Payload> {
    Udp(UdpTxWork<Payload>),
    Arp(ArpTxWork),
    IcmpEchoReply(IcmpEchoReplyWork<PAYLOAD_CAPACITY>),
}

impl<const PAYLOAD_CAPACITY: usize, Payload: AsRef<[u8]>> DeferredTxWork
    for ResearchTxWork<PAYLOAD_CAPACITY, Payload>
{
    type WriteError = FrameWriteError;

    fn egress_key(&self) -> EgressFlowKey {
        match self {
            Self::Udp(work) => work.key,
            Self::Arp(work) => work.key,
            Self::IcmpEchoReply(work) => work.key,
        }
    }

    fn enqueue_micros(&self) -> u64 {
        match self {
            Self::Udp(work) => work.enqueue_micros,
            Self::Arp(work) => work.enqueue_micros,
            Self::IcmpEchoReply(work) => work.enqueue_micros,
        }
    }

    fn frame_length(&self) -> u16 {
        let length = match self {
            Self::Udp(work) => work.frame_length(),
            Self::Arp(_) => ARP_FRAME_LEN,
            Self::IcmpEchoReply(work) => work.frame_length(),
        };
        length as u16
    }

    fn write_frame(&self, destination: &mut [u8]) -> Result<(), Self::WriteError> {
        match self {
            Self::Udp(work) => work.write(destination),
            Self::Arp(work) => work.write(destination),
            Self::IcmpEchoReply(work) => work.write(destination),
        }
    }
}

fn write_ethernet_header(
    frame: &mut [u8],
    destination: MacAddress,
    source: MacAddress,
    ether_type: u16,
) {
    frame[..6].copy_from_slice(&destination.bytes());
    frame[6..12].copy_from_slice(&source.bytes());
    frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
}

fn write_ipv4_header(
    header: &mut [u8],
    source: Ipv4Address,
    destination: Ipv4Address,
    identification: u16,
    protocol: u8,
    total_length: usize,
) {
    header.fill(0);
    header[0] = 0x45;
    header[2..4].copy_from_slice(&(total_length as u16).to_be_bytes());
    header[4..6].copy_from_slice(&identification.to_be_bytes());
    header[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    header[8] = 64;
    header[9] = protocol;
    header[12..16].copy_from_slice(&source.bytes());
    header[16..20].copy_from_slice(&destination.bytes());
    let checksum = internet_checksum(&[header]);
    header[10..12].copy_from_slice(&checksum.to_be_bytes());
}
