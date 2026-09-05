extern crate std;

use core::num::{NonZeroU16, NonZeroU32};
use std::vec::Vec;

use open_esp_radio_wifi_datapath::{BatchWriteError, EgressDemand, RadioPeer, ReservedTxBatch};

use super::*;
use crate::checksum::internet_checksum;

struct TestClassifier;

impl RadioRouteClassifier for TestClassifier {
    fn classify_unicast(
        &mut self,
        interface: NetworkInterfaceId,
        destination: MacAddress,
        traffic_identifier: TrafficIdentifier,
    ) -> Option<RadioEgressKey> {
        Some(RadioEgressKey::new(
            interface,
            9,
            RadioPeer::Unicast {
                slot: destination.bytes()[5],
                generation: 4,
            },
            traffic_identifier,
        ))
    }
}

struct WrongInterfaceClassifier;

impl RadioRouteClassifier for WrongInterfaceClassifier {
    fn classify_unicast(
        &mut self,
        _interface: NetworkInterfaceId,
        destination: MacAddress,
        traffic_identifier: TrafficIdentifier,
    ) -> Option<RadioEgressKey> {
        Some(RadioEgressKey::new(
            NetworkInterfaceId::new(99),
            9,
            RadioPeer::Unicast {
                slot: destination.bytes()[5],
                generation: 4,
            },
            traffic_identifier,
        ))
    }
}

struct TestBatch {
    capacity: usize,
    frames: Vec<Vec<u8>>,
}

impl ReservedTxBatch for TestBatch {
    fn remaining(&self) -> usize {
        self.capacity - self.frames.len()
    }

    fn try_write<WriteError>(
        &mut self,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> Result<(), WriteError>,
    ) -> Result<(), BatchWriteError<WriteError>> {
        if self.remaining() == 0 {
            return Err(BatchWriteError::Exhausted);
        }
        let mut frame = std::vec![0; length];
        write(&mut frame).map_err(BatchWriteError::Write)?;
        self.frames.push(frame);
        Ok(())
    }
}

fn config() -> ResearchNetworkConfig {
    ResearchNetworkConfig {
        interface: NetworkInterfaceId::new(2),
        mac: MacAddress::new([2, 0, 0, 0, 0, 1]),
        ipv4: Ipv4Address::new([192, 168, 7, 1]),
    }
}

fn unicast_key(slot: u8) -> RadioEgressKey {
    RadioEgressKey::new(
        config().interface,
        9,
        RadioPeer::Unicast {
            slot,
            generation: 4,
        },
        TrafficIdentifier::new(0).unwrap(),
    )
}

fn fill_only_demand(engine: &mut ResearchNetworkEngine<4, 8, 1472>) -> (EgressDemand, Vec<u8>) {
    let mut demand = None;
    engine.visit_demands(|observed| {
        assert!(demand.replace(observed).is_none());
    });
    let demand = demand.unwrap();
    let mut batch = TestBatch {
        capacity: 1,
        frames: Vec::new(),
    };
    let outcome = engine
        .fill_selected(
            EgressSelection {
                key: demand.key,
                max_frames: NonZeroU16::new(1).unwrap(),
                max_bytes: NonZeroU32::new(u32::MAX).unwrap(),
            },
            &mut batch,
        )
        .unwrap();
    assert_eq!(outcome.frames, 1);
    (demand, batch.frames.pop().unwrap())
}

#[test]
fn udp_work_is_built_directly_in_the_reserved_final_frame() {
    let mut engine = ResearchNetworkEngine::<4, 8, 1472>::new(config());
    engine
        .enqueue_udp(
            17,
            ResolvedIpv4Route {
                destination_mac: MacAddress::new([2, 0, 0, 0, 0, 7]),
                destination_ip: Ipv4Address::new([192, 168, 7, 7]),
                radio: unicast_key(7),
            },
            4000,
            5000,
            AdmissionClass::Bulk,
            b"research",
        )
        .unwrap();

    let (demand, frame) = fill_only_demand(&mut engine);
    assert_eq!(demand.key.admission, AdmissionClass::Bulk);
    assert_eq!(&frame[..6], &[2, 0, 0, 0, 0, 7]);
    assert_eq!(&frame[6..12], &config().mac.bytes());
    assert_eq!(internet_checksum(&[&frame[14..34]]), 0);
    assert_eq!(&frame[42..], b"research");
    assert_eq!(engine.queued_work(), 0);
}

#[test]
fn enqueue_rejects_a_radio_route_owned_by_another_interface() {
    let mut engine = ResearchNetworkEngine::<4, 8, 1472>::new(config());
    let wrong_interface = RadioEgressKey::new(
        NetworkInterfaceId::new(99),
        9,
        RadioPeer::Unicast {
            slot: 7,
            generation: 4,
        },
        TrafficIdentifier::new(0).unwrap(),
    );

    assert_eq!(
        engine.enqueue_udp(
            17,
            ResolvedIpv4Route {
                destination_mac: MacAddress::new([2, 0, 0, 0, 0, 7]),
                destination_ip: Ipv4Address::new([192, 168, 7, 7]),
                radio: wrong_interface,
            },
            4000,
            5000,
            AdmissionClass::Bulk,
            b"wrong-vif",
        ),
        Err(TxEnqueueError::InterfaceMismatch)
    );
    assert_eq!(engine.queued_work(), 0);
}

#[test]
fn received_udp_is_delivered_synchronously_without_an_rx_queue() {
    let mut sender = ResearchNetworkEngine::<4, 8, 1472>::new(ResearchNetworkConfig {
        interface: config().interface,
        mac: MacAddress::new([2, 0, 0, 0, 0, 7]),
        ipv4: Ipv4Address::new([192, 168, 7, 7]),
    });
    sender
        .enqueue_udp(
            1,
            ResolvedIpv4Route {
                destination_mac: config().mac,
                destination_ip: config().ipv4,
                radio: unicast_key(1),
            },
            5000,
            4000,
            AdmissionClass::Bulk,
            b"payload",
        )
        .unwrap();
    let (_, frame) = fill_only_demand(&mut sender);

    let mut receiver = ResearchNetworkEngine::<4, 8, 1472>::new(config());
    let mut delivered = None;
    let report = receiver.receive(2, &frame, &mut TestClassifier, |datagram| {
        delivered = Some((
            datagram.source,
            datagram.destination,
            datagram.payload.to_vec(),
        ));
    });
    assert_eq!(report.disposition, IngressDisposition::UdpDelivered);
    assert_eq!(delivered.unwrap().2, b"payload");
    assert_eq!(receiver.counters().rx_udp_delivered, 1);
}

#[test]
fn arp_request_creates_typed_link_control_work() {
    let mut frame = [0u8; ARP_FRAME_LEN];
    let sender_mac = MacAddress::new([2, 0, 0, 0, 0, 7]);
    frame[..6].copy_from_slice(&MacAddress::BROADCAST.bytes());
    frame[6..12].copy_from_slice(&sender_mac.bytes());
    frame[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    frame[14..16].copy_from_slice(&1u16.to_be_bytes());
    frame[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&1u16.to_be_bytes());
    frame[22..28].copy_from_slice(&sender_mac.bytes());
    frame[28..32].copy_from_slice(&[192, 168, 7, 7]);
    frame[38..42].copy_from_slice(&config().ipv4.bytes());

    let mut engine = ResearchNetworkEngine::<4, 8, 1472>::new(config());
    let report = engine.receive(9, &frame, &mut TestClassifier, |_| {});
    assert_eq!(report.disposition, IngressDisposition::ResponseQueued);
    let (demand, reply) = fill_only_demand(&mut engine);
    assert_eq!(demand.key.admission, AdmissionClass::LinkControl);
    assert_eq!(&reply[..6], &sender_mac.bytes());
    assert_eq!(&reply[20..22], &2u16.to_be_bytes());
    assert_eq!(&reply[22..28], &config().mac.bytes());
}

#[test]
fn reply_is_dropped_when_radio_classification_crosses_interfaces() {
    let mut frame = [0u8; ARP_FRAME_LEN];
    let sender_mac = MacAddress::new([2, 0, 0, 0, 0, 7]);
    frame[..6].copy_from_slice(&MacAddress::BROADCAST.bytes());
    frame[6..12].copy_from_slice(&sender_mac.bytes());
    frame[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    frame[14..16].copy_from_slice(&1u16.to_be_bytes());
    frame[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&1u16.to_be_bytes());
    frame[22..28].copy_from_slice(&sender_mac.bytes());
    frame[28..32].copy_from_slice(&[192, 168, 7, 7]);
    frame[38..42].copy_from_slice(&config().ipv4.bytes());

    let mut engine = ResearchNetworkEngine::<4, 8, 1472>::new(config());
    let report = engine.receive(9, &frame, &mut WrongInterfaceClassifier, |_| {});
    assert_eq!(report.disposition, IngressDisposition::ResponseDropped);
    assert_eq!(engine.queued_work(), 0);
}

#[test]
fn icmp_echo_is_queued_and_rechecksummed_as_a_reply() {
    let mut sender = ResearchNetworkEngine::<4, 8, 1472>::new(ResearchNetworkConfig {
        interface: config().interface,
        mac: MacAddress::new([2, 0, 0, 0, 0, 7]),
        ipv4: Ipv4Address::new([192, 168, 7, 7]),
    });
    let icmp = [8, 0, 0xf7, 0xfd, 0, 1, 0, 2];
    let reply = IcmpEchoReplyWork::new(
        Ipv4TxPath {
            key: EgressFlowKey {
                radio: unicast_key(1),
                admission: AdmissionClass::LinkControl,
            },
            enqueue_micros: 1,
            source_mac: sender.config.mac,
            destination_mac: config().mac,
            source_ip: sender.config.ipv4,
            destination_ip: config().ipv4,
            identification: 1,
        },
        &icmp,
    )
    .unwrap();
    sender
        .try_enqueue(ResearchTxWork::IcmpEchoReply(reply))
        .unwrap();
    let (_, mut request) = fill_only_demand(&mut sender);
    request[34] = 8;
    request[36..38].fill(0);
    let checksum = internet_checksum(&[&request[34..]]);
    request[36..38].copy_from_slice(&checksum.to_be_bytes());

    let mut engine = ResearchNetworkEngine::<4, 8, 1472>::new(config());
    let report = engine.receive(2, &request, &mut TestClassifier, |_| {});
    assert_eq!(report.disposition, IngressDisposition::ResponseQueued);
    let (_, response) = fill_only_demand(&mut engine);
    assert_eq!(response[34], 0);
    assert_eq!(internet_checksum(&[&response[34..]]), 0);
}
