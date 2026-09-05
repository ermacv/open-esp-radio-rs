use open_esp_radio_network::NetworkInterfaceId;
use open_esp_radio_wifi_datapath::{
    AdmissionClass, EgressFlowKey, EgressSelection, EgressWorkProvider, EnqueueError, FillFailure,
    FillOutcome, FixedEgressQueue, RadioEgressKey, ReservedTxBatch, TrafficIdentifier,
};

use crate::{
    Ipv4Address, MacAddress, ResolvedIpv4Route, UdpEndpoint,
    checksum::{internet_checksum, udp_ipv4_checksum},
    work::{ArpTxWork, FrameWriteError, IcmpEchoReplyWork, Ipv4TxPath, ResearchTxWork, UdpTxWork},
};

const ETHERNET_HEADER_LEN: usize = 14;
const IPV4_MIN_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;
const ARP_FRAME_LEN: usize = 42;

/// Immutable interface identity and addresses owned by one research engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResearchNetworkConfig {
    pub interface: NetworkInterfaceId,
    pub mac: MacAddress,
    pub ipv4: Ipv4Address,
}

/// Radio-owned route-to-current-peer classification used by reply paths.
pub trait RadioRouteClassifier {
    fn classify_unicast(
        &mut self,
        interface: NetworkInterfaceId,
        destination: MacAddress,
        traffic_identifier: TrafficIdentifier,
    ) -> Option<RadioEgressKey>;
}

/// Borrowed UDP delivery valid only for the synchronous receive call.
pub struct UdpDatagram<'frame> {
    pub source: UdpEndpoint,
    pub destination: UdpEndpoint,
    pub payload: &'frame [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngressDisposition {
    UdpDelivered,
    ResponseQueued,
    Accepted,
    NotForInterface,
    Unsupported,
    Malformed,
    ChecksumFailed,
    ResponseDropped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngressReport {
    pub disposition: IngressDisposition,
    pub frame_length: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineCounters {
    pub tx_enqueued: u64,
    pub tx_queue_rejected: u64,
    pub rx_udp_delivered: u64,
    pub rx_responses_queued: u64,
    pub rx_responses_dropped: u64,
    pub rx_malformed: u64,
    pub rx_checksum_failed: u64,
    pub rx_unsupported: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxEnqueueError {
    PayloadTooLong,
    InterfaceMismatch,
    WorkCapacity,
    FlowCapacity,
}

/// Synchronous Ethernet/ARP/IPv4/ICMP/UDP research engine.
///
/// `PAYLOAD_CAPACITY` and `WORK_CAPACITY` describe general-memory backlog.
/// Physical SRAM is supplied only to `fill_selected` and is never embedded in
/// this object, so its size is independent of peer count and queue depth.
pub struct ResearchNetworkEngine<
    const FLOW_CAPACITY: usize,
    const WORK_CAPACITY: usize,
    const PAYLOAD_CAPACITY: usize,
> {
    config: ResearchNetworkConfig,
    queue: FixedEgressQueue<ResearchTxWork<PAYLOAD_CAPACITY>, FLOW_CAPACITY, WORK_CAPACITY>,
    next_ipv4_identification: u16,
    counters: EngineCounters,
}

impl<const FLOW_CAPACITY: usize, const WORK_CAPACITY: usize, const PAYLOAD_CAPACITY: usize>
    ResearchNetworkEngine<FLOW_CAPACITY, WORK_CAPACITY, PAYLOAD_CAPACITY>
{
    pub const fn new(config: ResearchNetworkConfig) -> Self {
        Self {
            config,
            queue: FixedEgressQueue::new(),
            next_ipv4_identification: 0,
            counters: EngineCounters {
                tx_enqueued: 0,
                tx_queue_rejected: 0,
                rx_udp_delivered: 0,
                rx_responses_queued: 0,
                rx_responses_dropped: 0,
                rx_malformed: 0,
                rx_checksum_failed: 0,
                rx_unsupported: 0,
            },
        }
    }

    pub const fn config(&self) -> ResearchNetworkConfig {
        self.config
    }

    pub const fn counters(&self) -> EngineCounters {
        self.counters
    }

    pub const fn queued_work(&self) -> usize {
        self.queue.len()
    }

    pub fn enqueue_udp(
        &mut self,
        now_micros: u64,
        route: ResolvedIpv4Route,
        source_port: u16,
        destination_port: u16,
        admission: AdmissionClass,
        payload: &[u8],
    ) -> Result<(), TxEnqueueError> {
        if route.radio.interface() != self.config.interface {
            return Err(TxEnqueueError::InterfaceMismatch);
        }
        let work = UdpTxWork::new(
            Ipv4TxPath {
                key: EgressFlowKey {
                    radio: route.radio,
                    admission,
                },
                enqueue_micros: now_micros,
                source_mac: self.config.mac,
                destination_mac: route.destination_mac,
                source_ip: self.config.ipv4,
                destination_ip: route.destination_ip,
                identification: self.next_ipv4_identification,
            },
            source_port,
            destination_port,
            payload,
        )
        .map_err(|_| TxEnqueueError::PayloadTooLong)?;
        self.try_enqueue(ResearchTxWork::Udp(work))?;
        self.next_ipv4_identification = self.next_ipv4_identification.wrapping_add(1);
        Ok(())
    }

    pub fn enqueue_arp_request(
        &mut self,
        now_micros: u64,
        group_radio: RadioEgressKey,
        target: Ipv4Address,
    ) -> Result<(), TxEnqueueError> {
        if group_radio.interface() != self.config.interface {
            return Err(TxEnqueueError::InterfaceMismatch);
        }
        self.try_enqueue(ResearchTxWork::Arp(ArpTxWork::request(
            EgressFlowKey {
                radio: group_radio,
                admission: AdmissionClass::LinkControl,
            },
            now_micros,
            self.config.mac,
            self.config.ipv4,
            target,
        )))
    }

    pub fn visit_demands(&self, visitor: impl FnMut(open_esp_radio_wifi_datapath::EgressDemand)) {
        self.queue.visit_demands(visitor);
    }

    pub fn fill_selected<Batch: ReservedTxBatch>(
        &mut self,
        selection: EgressSelection,
        batch: &mut Batch,
    ) -> Result<FillOutcome, FillFailure<FrameWriteError>> {
        self.queue.fill_selected(selection, batch)
    }

    pub fn receive<C: RadioRouteClassifier>(
        &mut self,
        now_micros: u64,
        frame: &[u8],
        classifier: &mut C,
        mut udp: impl FnMut(UdpDatagram<'_>),
    ) -> IngressReport {
        let disposition = self.receive_inner(now_micros, frame, classifier, &mut udp);
        match disposition {
            IngressDisposition::UdpDelivered => self.counters.rx_udp_delivered += 1,
            IngressDisposition::ResponseQueued => self.counters.rx_responses_queued += 1,
            IngressDisposition::ResponseDropped => self.counters.rx_responses_dropped += 1,
            IngressDisposition::Malformed => self.counters.rx_malformed += 1,
            IngressDisposition::ChecksumFailed => self.counters.rx_checksum_failed += 1,
            IngressDisposition::Unsupported => self.counters.rx_unsupported += 1,
            IngressDisposition::Accepted | IngressDisposition::NotForInterface => {}
        }
        IngressReport {
            disposition,
            frame_length: frame.len(),
        }
    }

    fn try_enqueue(
        &mut self,
        work: ResearchTxWork<PAYLOAD_CAPACITY>,
    ) -> Result<(), TxEnqueueError> {
        match self.queue.try_enqueue(work) {
            Ok(()) => {
                self.counters.tx_enqueued += 1;
                Ok(())
            }
            Err(EnqueueError::WorkCapacity(_)) => {
                self.counters.tx_queue_rejected += 1;
                Err(TxEnqueueError::WorkCapacity)
            }
            Err(EnqueueError::FlowCapacity(_)) => {
                self.counters.tx_queue_rejected += 1;
                Err(TxEnqueueError::FlowCapacity)
            }
        }
    }

    fn receive_inner<C: RadioRouteClassifier>(
        &mut self,
        now_micros: u64,
        frame: &[u8],
        classifier: &mut C,
        udp: &mut impl FnMut(UdpDatagram<'_>),
    ) -> IngressDisposition {
        if frame.len() < ETHERNET_HEADER_LEN {
            return IngressDisposition::Malformed;
        }
        let destination_mac = MacAddress::new(frame[..6].try_into().unwrap());
        if destination_mac != self.config.mac && !destination_mac.is_broadcast() {
            return IngressDisposition::NotForInterface;
        }
        let source_mac = MacAddress::new(frame[6..12].try_into().unwrap());
        match u16::from_be_bytes(frame[12..14].try_into().unwrap()) {
            0x0806 => self.receive_arp(now_micros, frame, source_mac, classifier),
            0x0800 => self.receive_ipv4(now_micros, frame, source_mac, classifier, udp),
            _ => IngressDisposition::Unsupported,
        }
    }

    fn receive_arp<C: RadioRouteClassifier>(
        &mut self,
        now_micros: u64,
        frame: &[u8],
        source_mac: MacAddress,
        classifier: &mut C,
    ) -> IngressDisposition {
        if frame.len() < ARP_FRAME_LEN {
            return IngressDisposition::Malformed;
        }
        let arp = &frame[ETHERNET_HEADER_LEN..ARP_FRAME_LEN];
        if arp[0..2] != 1u16.to_be_bytes()
            || arp[2..4] != 0x0800u16.to_be_bytes()
            || arp[4] != 6
            || arp[5] != 4
        {
            return IngressDisposition::Unsupported;
        }
        let operation = u16::from_be_bytes(arp[6..8].try_into().unwrap());
        let sender_mac = MacAddress::new(arp[8..14].try_into().unwrap());
        if sender_mac != source_mac {
            return IngressDisposition::Malformed;
        }
        let sender_ip = Ipv4Address::new(arp[14..18].try_into().unwrap());
        let target_ip = Ipv4Address::new(arp[24..28].try_into().unwrap());
        if operation != 1 || target_ip != self.config.ipv4 {
            return IngressDisposition::Accepted;
        }
        let tid = TrafficIdentifier::new(0).expect("zero is a valid traffic identifier");
        let Some(radio) = classifier.classify_unicast(self.config.interface, sender_mac, tid)
        else {
            return IngressDisposition::ResponseDropped;
        };
        if radio.interface() != self.config.interface {
            return IngressDisposition::ResponseDropped;
        }
        let reply = ResearchTxWork::Arp(ArpTxWork::reply(
            EgressFlowKey {
                radio,
                admission: AdmissionClass::LinkControl,
            },
            now_micros,
            self.config.mac,
            self.config.ipv4,
            sender_mac,
            sender_ip,
        ));
        if self.try_enqueue(reply).is_ok() {
            IngressDisposition::ResponseQueued
        } else {
            IngressDisposition::ResponseDropped
        }
    }

    fn receive_ipv4<C: RadioRouteClassifier>(
        &mut self,
        now_micros: u64,
        frame: &[u8],
        source_mac: MacAddress,
        classifier: &mut C,
        udp: &mut impl FnMut(UdpDatagram<'_>),
    ) -> IngressDisposition {
        let packet = &frame[ETHERNET_HEADER_LEN..];
        if packet.len() < IPV4_MIN_HEADER_LEN || packet[0] >> 4 != 4 {
            return IngressDisposition::Malformed;
        }
        let header_length = usize::from(packet[0] & 0x0f) * 4;
        if header_length < IPV4_MIN_HEADER_LEN || packet.len() < header_length {
            return IngressDisposition::Malformed;
        }
        let total_length = usize::from(u16::from_be_bytes(packet[2..4].try_into().unwrap()));
        if total_length < header_length || total_length > packet.len() {
            return IngressDisposition::Malformed;
        }
        if internet_checksum(&[&packet[..header_length]]) != 0 {
            return IngressDisposition::ChecksumFailed;
        }
        let fragment = u16::from_be_bytes(packet[6..8].try_into().unwrap());
        if fragment & 0x3fff != 0 {
            return IngressDisposition::Unsupported;
        }
        let source_ip = Ipv4Address::new(packet[12..16].try_into().unwrap());
        let destination_ip = Ipv4Address::new(packet[16..20].try_into().unwrap());
        if destination_ip != self.config.ipv4 {
            return IngressDisposition::NotForInterface;
        }
        let payload = &packet[header_length..total_length];
        match packet[9] {
            1 => self.receive_icmp(
                now_micros, source_mac, source_ip, packet, payload, classifier,
            ),
            17 => self.receive_udp(source_ip, destination_ip, payload, udp),
            _ => IngressDisposition::Unsupported,
        }
    }

    fn receive_udp(
        &mut self,
        source_ip: Ipv4Address,
        destination_ip: Ipv4Address,
        packet: &[u8],
        udp: &mut impl FnMut(UdpDatagram<'_>),
    ) -> IngressDisposition {
        if packet.len() < UDP_HEADER_LEN {
            return IngressDisposition::Malformed;
        }
        let length = usize::from(u16::from_be_bytes(packet[4..6].try_into().unwrap()));
        if length < UDP_HEADER_LEN || length > packet.len() {
            return IngressDisposition::Malformed;
        }
        let checksum = u16::from_be_bytes(packet[6..8].try_into().unwrap());
        if checksum != 0
            && udp_ipv4_checksum(
                source_ip.bytes(),
                destination_ip.bytes(),
                &packet[..UDP_HEADER_LEN],
                &packet[UDP_HEADER_LEN..length],
            ) != 0xffff
        {
            return IngressDisposition::ChecksumFailed;
        }
        udp(UdpDatagram {
            source: UdpEndpoint {
                address: source_ip,
                port: u16::from_be_bytes(packet[0..2].try_into().unwrap()),
            },
            destination: UdpEndpoint {
                address: destination_ip,
                port: u16::from_be_bytes(packet[2..4].try_into().unwrap()),
            },
            payload: &packet[UDP_HEADER_LEN..length],
        });
        IngressDisposition::UdpDelivered
    }

    fn receive_icmp<C: RadioRouteClassifier>(
        &mut self,
        now_micros: u64,
        source_mac: MacAddress,
        source_ip: Ipv4Address,
        ipv4_packet: &[u8],
        icmp: &[u8],
        classifier: &mut C,
    ) -> IngressDisposition {
        if icmp.len() < 8 {
            return IngressDisposition::Malformed;
        }
        if internet_checksum(&[icmp]) != 0 {
            return IngressDisposition::ChecksumFailed;
        }
        if icmp[0] != 8 || icmp[1] != 0 {
            return IngressDisposition::Unsupported;
        }
        let tid = TrafficIdentifier::new(0).expect("zero is a valid traffic identifier");
        let Some(radio) = classifier.classify_unicast(self.config.interface, source_mac, tid)
        else {
            return IngressDisposition::ResponseDropped;
        };
        if radio.interface() != self.config.interface {
            return IngressDisposition::ResponseDropped;
        }
        let identification = u16::from_be_bytes(ipv4_packet[4..6].try_into().unwrap());
        let Ok(reply) = IcmpEchoReplyWork::new(
            Ipv4TxPath {
                key: EgressFlowKey {
                    radio,
                    admission: AdmissionClass::LinkControl,
                },
                enqueue_micros: now_micros,
                source_mac: self.config.mac,
                destination_mac: source_mac,
                source_ip: self.config.ipv4,
                destination_ip: source_ip,
                identification,
            },
            icmp,
        ) else {
            return IngressDisposition::ResponseDropped;
        };
        if self
            .try_enqueue(ResearchTxWork::IcmpEchoReply(reply))
            .is_ok()
        {
            IngressDisposition::ResponseQueued
        } else {
            IngressDisposition::ResponseDropped
        }
    }
}

#[cfg(test)]
mod tests;
