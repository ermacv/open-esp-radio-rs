extern crate std;

use core::{
    cell::Cell,
    num::{NonZeroU16, NonZeroU32},
};
use std::{vec, vec::Vec};

use open_esp_radio_wifi_datapath::{BatchWriteError, RadioPeer};

use super::*;

type Engine = ResearchNetworkEngine<4, 8, 1472>;

fn config() -> ResearchNetworkConfig {
    ResearchNetworkConfig {
        interface: NetworkInterfaceId::new(2),
        mac: MacAddress::new([2, 0, 0, 0, 0, 1]),
        ipv4: Ipv4Address::new([192, 168, 7, 1]),
    }
}

const SOURCE: MacAddress = MacAddress::new([2, 0, 0, 0, 0, 7]);
const SOURCE_IP: [u8; 4] = [192, 168, 7, 7];

#[derive(Default)]
struct Classifier(Vec<MacAddress>);

impl RadioRouteClassifier for Classifier {
    fn classify_unicast(
        &mut self,
        interface: NetworkInterfaceId,
        destination: MacAddress,
        traffic_identifier: TrafficIdentifier,
    ) -> Option<RadioEgressKey> {
        self.0.push(destination);
        Some(RadioEgressKey::new(
            interface,
            9,
            RadioPeer::Unicast {
                slot: 7,
                generation: 4,
            },
            traffic_identifier,
        ))
    }
}

#[derive(Default)]
struct Batch(Vec<Vec<u8>>);

impl ReservedTxBatch for Batch {
    fn remaining(&self) -> usize {
        1 - self.0.len()
    }

    fn try_write<E>(
        &mut self,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<(), BatchWriteError<E>> {
        let mut frame = vec![0; length];
        write(&mut frame).map_err(BatchWriteError::Write)?;
        self.0.push(frame);
        Ok(())
    }
}

fn replies(engine: &mut Engine) -> Vec<Vec<u8>> {
    let mut demand = None;
    engine.visit_demands(|next| {
        assert!(demand.replace(next).is_none());
    });
    let Some(demand) = demand else {
        return Vec::new();
    };
    let mut batch = Batch::default();
    engine
        .fill_selected(
            EgressSelection {
                key: demand.key,
                max_frames: NonZeroU16::new(1).unwrap(),
                max_bytes: NonZeroU32::new(u32::MAX).unwrap(),
            },
            &mut batch,
        )
        .unwrap();
    batch.0
}

fn ipv4(protocol: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0; 20];
    packet[0] = 0x45;
    packet[2..4].copy_from_slice(&u16::try_from(20 + payload.len()).unwrap().to_be_bytes());
    packet[8] = 64;
    packet[9] = protocol;
    packet[12..16].copy_from_slice(&SOURCE_IP);
    packet[16..20].copy_from_slice(&config().ipv4.bytes());
    fix_ipv4_checksum(&mut packet);
    packet.extend_from_slice(payload);
    packet
}

fn fix_ipv4_checksum(packet: &mut [u8]) {
    packet[10..12].fill(0);
    let checksum = internet_checksum(&[&packet[..20]]);
    packet[10..12].copy_from_slice(&checksum.to_be_bytes());
}

fn udp_packet() -> Vec<u8> {
    let mut udp = vec![0x13, 0x88, 0x0f, 0xa0, 0, 13, 0, 0];
    udp.extend_from_slice(b"hello");
    let checksum = udp_ipv4_checksum(SOURCE_IP, config().ipv4.bytes(), &udp[..8], &udp[8..]);
    udp[6..8].copy_from_slice(&checksum.to_be_bytes());
    ipv4(17, &udp)
}

fn arp_packet() -> Vec<u8> {
    let mut arp = vec![0; 28];
    arp[..8].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
    arp[8..14].copy_from_slice(&SOURCE.bytes());
    arp[14..18].copy_from_slice(&SOURCE_IP);
    arp[24..28].copy_from_slice(&config().ipv4.bytes());
    arp
}

fn icmp_packet() -> Vec<u8> {
    let mut icmp = vec![8, 0, 0, 0, 0, 1, 0, 2, 3];
    let checksum = internet_checksum(&[&icmp]);
    icmp[2..4].copy_from_slice(&checksum.to_be_bytes());
    ipv4(1, &icmp)
}

#[derive(Debug, Eq, PartialEq)]
struct Observation {
    report: IngressReport,
    counters: EngineCounters,
    udp: Vec<(UdpEndpoint, UdpEndpoint, Vec<u8>)>,
    classified: Vec<MacAddress>,
    replies: Vec<Vec<u8>>,
}

fn observe(destination: MacAddress, ether_type: u16, payload: &[u8], parts: bool) -> Observation {
    let mut engine = Engine::new(config());
    let mut classifier = Classifier::default();
    let mut udp = Vec::new();
    let callback = |datagram: UdpDatagram<'_>| {
        udp.push((
            datagram.source,
            datagram.destination,
            datagram.payload.to_vec(),
        ));
    };
    let report = if parts {
        engine.receive_parts(
            17,
            destination,
            SOURCE,
            ether_type,
            payload,
            &mut classifier,
            callback,
        )
    } else {
        let mut frame = Vec::new();
        frame.extend_from_slice(&destination.bytes());
        frame.extend_from_slice(&SOURCE.bytes());
        frame.extend_from_slice(&ether_type.to_be_bytes());
        frame.extend_from_slice(payload);
        engine.receive(17, &frame, &mut classifier, callback)
    };
    Observation {
        report,
        counters: engine.counters(),
        udp,
        classified: classifier.0,
        replies: replies(&mut engine),
    }
}

fn parity(ether_type: u16, payload: &[u8], expected: IngressDisposition) {
    let contiguous = observe(config().mac, ether_type, payload, false);
    let parts = observe(config().mac, ether_type, payload, true);
    assert_eq!(parts.report.disposition, expected);
    assert_eq!(parts.report.frame_length, 14 + payload.len());
    assert_eq!(parts, contiguous);
}

#[test]
fn split_packets_preserve_delivery_replies_and_ethernet_padding() {
    for (ether_type, mut packet, expected) in [
        (0x0800, udp_packet(), IngressDisposition::UdpDelivered),
        (0x0800, icmp_packet(), IngressDisposition::ResponseQueued),
        (0x0806, arp_packet(), IngressDisposition::ResponseQueued),
    ] {
        parity(ether_type, &packet, expected);
        packet.extend_from_slice(&[0x5a; 20]);
        parity(ether_type, &packet, expected);
    }
}

#[test]
fn truncated_and_malformed_protocols_have_the_same_disposition_and_accounting() {
    for (ether_type, packet) in [
        (0x0800, udp_packet()),
        (0x0800, icmp_packet()),
        (0x0806, arp_packet()),
    ] {
        for length in 0..packet.len() {
            parity(ether_type, &packet[..length], IngressDisposition::Malformed);
        }
    }
    for value in [0x44, 0x46, 0x65] {
        let mut packet = ipv4(17, &[]);
        packet[0] = value;
        parity(0x0800, &packet, IngressDisposition::Malformed);
    }
    for length in [19u16, 200] {
        let mut packet = udp_packet();
        packet[2..4].copy_from_slice(&length.to_be_bytes());
        parity(0x0800, &packet, IngressDisposition::Malformed);
    }
    for length in [0u16, 7, 14] {
        let mut packet = udp_packet();
        packet[24..26].copy_from_slice(&length.to_be_bytes());
        parity(0x0800, &packet, IngressDisposition::Malformed);
    }
    parity(0x0800, &ipv4(17, &[0; 7]), IngressDisposition::Malformed);
    parity(0x0800, &ipv4(1, &[0; 7]), IngressDisposition::Malformed);
    let mut arp = arp_packet();
    arp[8] ^= 1;
    parity(0x0806, &arp, IngressDisposition::Malformed);
}

#[test]
fn checksum_fragment_and_unsupported_protocol_rejections_are_preserved() {
    let mut packet = udp_packet();
    packet[10] ^= 1;
    parity(0x0800, &packet, IngressDisposition::ChecksumFailed);
    let mut packet = udp_packet();
    *packet.last_mut().unwrap() ^= 1;
    parity(0x0800, &packet, IngressDisposition::ChecksumFailed);
    let mut packet = icmp_packet();
    *packet.last_mut().unwrap() ^= 1;
    parity(0x0800, &packet, IngressDisposition::ChecksumFailed);
    let mut packet = udp_packet();
    packet[6] = 0x20;
    fix_ipv4_checksum(&mut packet);
    parity(0x0800, &packet, IngressDisposition::Unsupported);
    let mut arp = arp_packet();
    arp[4] = 4;
    parity(0x0806, &arp, IngressDisposition::Unsupported);
    parity(0x0800, &ipv4(6, &[0; 20]), IngressDisposition::Unsupported);
}

#[test]
fn eapol_remains_outside_the_engine_and_destination_filtering_precedes_parsing() {
    for destination in [config().mac, MacAddress::BROADCAST] {
        let parts = observe(destination, 0x888e, b"security-owner", true);
        assert_eq!(
            parts,
            observe(destination, 0x888e, b"security-owner", false)
        );
        assert_eq!(parts.report.disposition, IngressDisposition::Unsupported);
        assert!(parts.classified.is_empty() && parts.udp.is_empty() && parts.replies.is_empty());
    }
    let other = MacAddress::new([2, 0, 0, 0, 0, 99]);
    let parts = observe(other, 0x0800, &[], true);
    assert_eq!(parts, observe(other, 0x0800, &[], false));
    assert_eq!(
        parts.report.disposition,
        IngressDisposition::NotForInterface
    );
}

#[test]
fn udp_callback_borrows_the_callers_storage_and_releases_it_on_return() {
    struct ReceiveOwner<'a> {
        packet: Vec<u8>,
        dropped: &'a Cell<bool>,
    }
    impl Drop for ReceiveOwner<'_> {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }
    let dropped = Cell::new(false);
    let mut owner = ReceiveOwner {
        packet: udp_packet(),
        dropped: &dropped,
    };
    let mut engine = Engine::new(config());
    let mut called = false;
    let report = engine.receive_parts(
        1,
        config().mac,
        SOURCE,
        0x0800,
        &owner.packet,
        &mut Classifier::default(),
        |datagram| {
            assert!(!dropped.get());
            assert_eq!(datagram.payload.as_ptr(), owner.packet[28..].as_ptr());
            assert_eq!(datagram.payload, b"hello");
            called = true;
        },
    );
    assert!(called);
    assert_eq!(report.disposition, IngressDisposition::UdpDelivered);
    owner.packet.fill(0);
    drop(owner);
    assert!(dropped.get());
    assert_eq!(engine.queued_work(), 0);
}

#[test]
fn queued_replies_do_not_retain_the_borrowed_receive_storage() {
    for (ether_type, mut packet) in [(0x0806, arp_packet()), (0x0800, icmp_packet())] {
        let expected = observe(config().mac, ether_type, &packet, false).replies;
        let mut engine = Engine::new(config());
        let report = engine.receive_parts(
            17,
            config().mac,
            SOURCE,
            ether_type,
            &packet,
            &mut Classifier::default(),
            |_| panic!("control packet delivered as UDP"),
        );
        assert_eq!(report.disposition, IngressDisposition::ResponseQueued);
        packet.fill(0);
        drop(packet);
        assert_eq!(replies(&mut engine), expected);
    }
}

#[test]
fn incomplete_ethernet_headers_are_rejected_without_double_counting() {
    for length in 0..14 {
        let mut engine = Engine::new(config());
        let report = engine.receive(
            0,
            &vec![0; length],
            &mut Classifier::default(),
            |_| panic!(),
        );
        assert_eq!(report.disposition, IngressDisposition::Malformed);
        assert_eq!(report.frame_length, length);
        assert_eq!(engine.counters().rx_malformed, 1);
    }
}
