use core::cell::Cell;
use std::{boxed::Box, rc::Rc};

use super::*;

#[derive(Debug)]
struct Lease {
    bytes: Box<[u8]>,
    returns: Rc<Cell<usize>>,
    length: Rc<Cell<usize>>,
}

impl Lease {
    fn new(bytes: &[u8], returns: &Rc<Cell<usize>>) -> Self {
        Self {
            bytes: bytes.into(),
            returns: returns.clone(),
            length: Rc::new(Cell::new(bytes.len())),
        }
    }
}

impl AsRef<[u8]> for Lease {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..self.length.get()]
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.returns.set(self.returns.get() + 1);
    }
}

fn route(slot: u8) -> ResolvedIpv4Route {
    ResolvedIpv4Route {
        destination_mac: MacAddress::new([2, 0, 0, 0, 0, slot]),
        destination_ip: Ipv4Address::new([192, 168, 7, slot]),
        radio: unicast_key(slot),
    }
}

fn selection() -> EgressSelection {
    EgressSelection {
        key: EgressFlowKey {
            radio: unicast_key(7),
            admission: AdmissionClass::Bulk,
        },
        max_frames: NonZeroU16::new(2).unwrap(),
        max_bytes: NonZeroU32::new(3200).unwrap(),
    }
}

#[test]
fn lease_survives_backpressure_and_is_released_after_final_construction() {
    let returns = Rc::new(Cell::new(0));
    // Inline capacity bounds ICMP here; it must not truncate a UDP lease.
    let mut engine = ResearchNetworkEngine::<2, 2, 8, Lease>::new(config());
    engine
        .enqueue_udp_owned(
            1,
            route(7),
            4000,
            5000,
            AdmissionClass::Bulk,
            Lease::new(b"external payload", &returns),
        )
        .unwrap();
    let mut batch = TestBatch {
        capacity: 0,
        frames: Vec::new(),
    };
    let blocked = engine.fill_selected(selection(), &mut batch).unwrap();
    assert_eq!(blocked.frames, 0);
    assert_eq!(blocked.source_remaining, 1);
    assert_eq!(returns.get(), 0);

    batch.capacity = 1;
    assert_eq!(
        engine
            .fill_selected(selection(), &mut batch)
            .unwrap()
            .frames,
        1
    );
    assert_eq!(returns.get(), 1);
    // Validate the final UDP checksum and payload using the receive path.
    let mut peer = ResearchNetworkEngine::<2, 2, 8>::new(ResearchNetworkConfig {
        mac: route(7).destination_mac,
        ipv4: route(7).destination_ip,
        ..config()
    });
    let report = peer.receive(2, &batch.frames[0], &mut TestClassifier, |datagram| {
        assert_eq!(datagram.payload, b"external payload");
        assert_eq!(datagram.source.port, 4000);
        assert_eq!(datagram.destination.port, 5000);
    });
    assert_eq!(report.disposition, IngressDisposition::UdpDelivered);
    drop(engine);
    assert_eq!(returns.get(), 1);
}

#[test]
fn every_admission_failure_returns_the_original_lease() {
    for error in [
        TxEnqueueError::InterfaceMismatch,
        TxEnqueueError::PayloadTooLong,
        TxEnqueueError::WorkCapacity,
        TxEnqueueError::FlowCapacity,
    ] {
        let returns = Rc::new(Cell::new(0));
        let mut engine = ResearchNetworkEngine::<1, 2, 8, Lease>::new(config());
        if matches!(
            error,
            TxEnqueueError::WorkCapacity | TxEnqueueError::FlowCapacity
        ) {
            engine
                .enqueue_udp_owned(
                    1,
                    route(7),
                    1,
                    2,
                    AdmissionClass::Bulk,
                    Lease::new(b"first", &returns),
                )
                .unwrap();
        }
        if error == TxEnqueueError::WorkCapacity {
            engine
                .enqueue_udp_owned(
                    1,
                    route(7),
                    1,
                    2,
                    AdmissionClass::Bulk,
                    Lease::new(b"second", &returns),
                )
                .unwrap();
        }
        let payload = if error == TxEnqueueError::PayloadTooLong {
            Lease::new(&std::vec![0; usize::from(u16::MAX)], &returns)
        } else {
            Lease::new(b"retryable", &returns)
        };
        let address = payload.bytes.as_ptr();
        let mut route = route(if error == TxEnqueueError::FlowCapacity {
            8
        } else {
            7
        });
        if error == TxEnqueueError::InterfaceMismatch {
            route.radio = RadioEgressKey::new(
                NetworkInterfaceId::new(99),
                9,
                RadioPeer::Group { generation: 1 },
                TrafficIdentifier::new(0).unwrap(),
            );
        }
        let queued = engine.queued_work();
        let failure = engine
            .enqueue_udp_owned(2, route, 1, 2, AdmissionClass::Bulk, payload)
            .unwrap_err();
        assert_eq!(failure.error, error);
        assert_eq!(failure.payload.bytes.as_ptr(), address);
        assert_eq!(engine.queued_work(), queued);
        assert_eq!(returns.get(), 0);
        drop(failure);
        assert_eq!(returns.get(), 1);
        drop(engine);
        assert_eq!(returns.get(), 1 + queued);
    }
}

#[test]
fn a_rejected_owner_can_be_retried_after_queue_space_returns() {
    let returns = Rc::new(Cell::new(0));
    let mut engine = ResearchNetworkEngine::<1, 1, 8, Lease>::new(config());
    engine
        .enqueue_udp_owned(
            1,
            route(7),
            1,
            2,
            AdmissionClass::Bulk,
            Lease::new(b"first", &returns),
        )
        .unwrap();
    let failure = engine
        .enqueue_udp_owned(
            2,
            route(7),
            1,
            2,
            AdmissionClass::Bulk,
            Lease::new(b"second", &returns),
        )
        .unwrap_err();
    let mut batch = TestBatch {
        capacity: 2,
        frames: Vec::new(),
    };
    engine.fill_selected(selection(), &mut batch).unwrap();
    engine
        .enqueue_udp_owned(3, route(7), 1, 2, AdmissionClass::Bulk, failure.payload)
        .unwrap();
    engine.fill_selected(selection(), &mut batch).unwrap();
    assert_eq!(&batch.frames[1][42..], b"second");
    assert_eq!(
        u16::from_be_bytes(batch.frames[1][18..20].try_into().unwrap()),
        1
    );
    assert_eq!(returns.get(), 2);
}

#[test]
fn length_change_fails_transactionally_and_retains_the_owner() {
    let returns = Rc::new(Cell::new(0));
    let mut engine = ResearchNetworkEngine::<1, 2, 8, Lease>::new(config());
    engine
        .enqueue_udp_owned(
            1,
            route(7),
            1,
            2,
            AdmissionClass::Bulk,
            Lease::new(b"first", &returns),
        )
        .unwrap();
    let payload = Lease::new(b"second", &returns);
    let length = payload.length.clone();
    engine
        .enqueue_udp_owned(2, route(7), 1, 2, AdmissionClass::Bulk, payload)
        .unwrap();
    length.set(1);
    let mut batch = TestBatch {
        capacity: 2,
        frames: Vec::new(),
    };
    let failure = engine.fill_selected(selection(), &mut batch).unwrap_err();
    assert_eq!(failure.error, FrameWriteError::PayloadLengthChanged);
    assert_eq!(failure.committed_frames, 1);
    assert_eq!(failure.source_remaining, 1);
    assert_eq!(batch.frames.len(), 1);
    assert_eq!(returns.get(), 1);
    length.set(6);
    engine.fill_selected(selection(), &mut batch).unwrap();
    assert_eq!(&batch.frames[1][42..], b"second");
    assert_eq!(returns.get(), 2);
}

#[test]
fn dropping_a_backlogged_engine_releases_all_payloads_once() {
    let returns = Rc::new(Cell::new(0));
    let mut engine = ResearchNetworkEngine::<1, 2, 8, Lease>::new(config());
    for payload in [b"one", b"two"] {
        engine
            .enqueue_udp_owned(
                1,
                route(7),
                1,
                2,
                AdmissionClass::Bulk,
                Lease::new(payload, &returns),
            )
            .unwrap();
    }
    assert_eq!(returns.get(), 0);
    drop(engine);
    assert_eq!(returns.get(), 2);
}
