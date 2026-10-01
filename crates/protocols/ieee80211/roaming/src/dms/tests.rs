use super::*;
use crate::{Identities, TxOutcome};
use oer_time::{Duration, Instant};

fn peer(n: u8) -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, n],
        generation: 7,
    }
}
fn bss() -> BssIdentity {
    BssIdentity {
        bssid: [2, 0, 0, 0, 1, 1],
        generation: 3,
    }
}
fn now(n: u64) -> Instant {
    Instant::from_micros(n)
}
fn attributes(destination: u8) -> std::vec::Vec<u8> {
    let mut body = [0; 32];
    let n = TclasRule {
        user_priority: 0,
        mask: 2,
        parameters: TclasParameters::Ethernet {
            source: [0; 6],
            destination: [1, 0, 0, 0, 0, destination],
            ether_type: 0x0800,
        },
    }
    .encode(&mut body)
    .unwrap();
    body[..n].to_vec()
}
fn request(token: u8, descriptors: &[(u8, DmsRequestType, &[u8])]) -> std::vec::Vec<u8> {
    let mut bytes = std::vec![10, 23, token];
    for &(dms_id, request_type, attributes) in descriptors {
        let mut descriptor = [0; 255];
        let n = DmsDescriptor {
            dms_id,
            request_type,
            attributes: Elements::parse(attributes).unwrap(),
        }
        .encode(&mut descriptor)
        .unwrap();
        bytes.extend_from_slice(&[99, n as u8]);
        bytes.extend_from_slice(&descriptor[..n]);
    }
    bytes
}
fn response(
    token: u8,
    id: u8,
    kind: DmsResponseType,
    sequence: LastSequenceControl,
    attributes: &[u8],
) -> std::vec::Vec<u8> {
    let mut body = [0; 255];
    let n = DmsStatus {
        dms_id: id,
        response_type: kind,
        last_sequence_control: sequence.encode().unwrap(),
        attributes: Elements::parse(attributes).unwrap(),
    }
    .encode(&mut body)
    .unwrap();
    let mut bytes = std::vec![10, 24, token, 100, n as u8];
    bytes.extend_from_slice(&body[..n]);
    bytes
}
fn packet(n: u8) -> DmsPacket {
    DmsPacket {
        source: [2, 0, 0, 0, 0, 8],
        destination: [1, 0, 0, 0, 0, n],
        ether_type: 0x0800,
        ip: None,
    }
}
fn grant(station: &mut DmsStation<256, 128, 4>, attrs: &[u8]) {
    let wire = request(1, &[(0, DmsRequestType::ADD, attrs)]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(0))
        .unwrap();
    station.admitted(id, now(0)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                1,
                9,
                DmsResponseType::ACCEPT,
                LastSequenceControl::Unsupported,
                attrs,
            ),
            now(1),
        )
        .unwrap();
}

#[test]
fn registry_shares_identity_but_retains_each_peers_parameters() {
    let mut registry = DmsRegistry::<4, 128>::new(bss()).unwrap();
    let a = attributes(1);
    let mut b = attributes(2);
    let mut first = a.clone();
    first.extend_from_slice(&b);
    first.extend_from_slice(&[44, 1, 1, 221, 1, 9]);
    b.extend_from_slice(&a);
    b.extend_from_slice(&[44, 1, 1, 221, 1, 10]);
    let mut assigned = None;
    for (link, attrs) in [(peer(1), &first), (peer(2), &b)] {
        let id = Identities::new(link).issue().unwrap();
        let wire = request(1, &[(0, DmsRequestType::ADD, attrs)]);
        let transaction = registry
            .prepare::<4, 256>(id, DmsRequest::parse(&wire).unwrap(), |_| {
                DmsAdmission::Accept
            })
            .unwrap();
        let new = transaction
            .response()
            .statuses()
            .unwrap()
            .next()
            .unwrap()
            .dms_id;
        if let Some(assigned) = assigned {
            assert_eq!(assigned, new);
        }
        assigned = Some(new);
        registry.commit(transaction).unwrap();
    }
    assert_eq!(registry.subscriptions().count(), 2);
    assert_eq!(
        registry
            .recipients(packet(1))
            .unwrap()
            .collect::<std::vec::Vec<_>>(),
        [peer(1), peer(2)]
    );
    assert!(
        !registry
            .group_delivery_required(packet(1), &[peer(1), peer(2)])
            .unwrap()
    );
    assert!(
        registry
            .group_delivery_required(packet(1), &[peer(1), peer(2), peer(3)])
            .unwrap()
    );
    assert_eq!(
        registry
            .remove_peer(LinkIdentity {
                generation: 8,
                ..peer(1)
            })
            .unwrap(),
        0
    );
    assert_eq!(registry.remove_peer(peer(1)).unwrap(), 1);
}

#[test]
fn transaction_capacity_failure_and_stale_revision_do_not_partially_commit() {
    let mut registry = DmsRegistry::<1, 128>::new(bss()).unwrap();
    let mut ids = Identities::new(peer(1));
    let a = attributes(1);
    let b = attributes(2);
    let wire = request(
        1,
        &[(0, DmsRequestType::ADD, &a), (0, DmsRequestType::ADD, &b)],
    );
    assert!(matches!(
        registry.prepare::<2, 256>(
            ids.issue().unwrap(),
            DmsRequest::parse(&wire).unwrap(),
            |_| DmsAdmission::Accept
        ),
        Err(DmsError::Full)
    ));
    assert_eq!(registry.subscriptions().count(), 0);
    let wire = request(2, &[(0, DmsRequestType::ADD, &a)]);
    let first = registry
        .prepare::<1, 256>(
            ids.issue().unwrap(),
            DmsRequest::parse(&wire).unwrap(),
            |_| DmsAdmission::Accept,
        )
        .unwrap();
    let stale = registry
        .prepare::<1, 256>(
            ids.issue().unwrap(),
            DmsRequest::parse(&wire).unwrap(),
            |_| DmsAdmission::Accept,
        )
        .unwrap();
    registry.commit(first).unwrap();
    assert_eq!(registry.commit(stale), Err(DmsError::StaleTransaction));
    assert_eq!(registry.subscriptions().count(), 1);
}

#[test]
fn ap_publication_and_replay_commit_only_once() {
    let mut registry = DmsRegistry::<4, 128>::new(bss()).unwrap();
    let mut ap = DmsAccessPoint::<256>::new(bss(), peer(1), Duration::from_micros(100)).unwrap();
    let attrs = attributes(1);
    let wire = request(1, &[(0, DmsRequestType::ADD, &attrs)]);
    let DmsApEvent::Requested { id } = ap.receive(peer(1), &wire, now(0)).unwrap() else {
        panic!()
    };
    let transaction = registry
        .prepare::<4, 256>(id, ap.request().unwrap().1, |_| DmsAdmission::Accept)
        .unwrap();
    let tx = ap.respond(&mut registry, transaction, now(1)).unwrap();
    assert_ne!(tx, id);
    ap.admitted(tx, now(1)).unwrap();
    ap.tx_completed(tx, TxOutcome::Acknowledged, now(2))
        .unwrap();
    assert!(matches!(
        ap.receive(peer(1), &wire, now(3)).unwrap(),
        DmsApEvent::ReplayQueued { .. }
    ));
    assert_eq!(registry.subscriptions().count(), 1);
    assert_eq!(ap.next_deadline(), Some(now(100)));
    let mut changed = wire.clone();
    *changed.last_mut().unwrap() = 2;
    assert_eq!(
        ap.receive(peer(1), &changed, now(4)),
        Err(DmsError::Protocol(Error::ConflictingDialog))
    );
}

#[test]
fn client_termination_preserves_sequence_evidence_across_wrap() {
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let attrs = attributes(1);
    grant(&mut station, &attrs);
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 4090, now(2))
            .unwrap(),
        GroupReception::Discard
    );
    let wire = request(2, &[(9, DmsRequestType::REMOVE, &[])]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(3))
        .unwrap();
    station.admitted(id, now(3)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                2,
                9,
                DmsResponseType::TERMINATE,
                LastSequenceControl::Sequence(4094),
                &[],
            ),
            now(4),
        )
        .unwrap();
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 4093, now(5))
            .unwrap(),
        GroupReception::Discard
    );
    assert_eq!(
        station.receive_group(peer(1), packet(1), 2046, now(6)),
        Err(DmsError::AmbiguousSequence)
    );
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 1, now(7))
            .unwrap(),
        GroupReception::Keep
    );
    assert_eq!(station.entries().count(), 0);
}

#[test]
fn lost_removal_requires_exact_retry_without_discarding_group_delivery() {
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let attrs = attributes(1);
    grant(&mut station, &attrs);
    let wire = request(2, &[(9, DmsRequestType::REMOVE, &[])]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(3))
        .unwrap();
    station.admitted(id, now(3)).unwrap();
    assert_eq!(
        station.poll(now(103)).unwrap(),
        DmsStationEvent::RecoveryRequired { id }
    );
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 3, now(104))
            .unwrap(),
        GroupReception::RecoveryRequired
    );
    let retry = station.retry(now(105)).unwrap();
    assert_ne!(retry, id);
    assert_eq!(station.transmission().unwrap().body, wire);
    station.admitted(retry, now(105)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                2,
                9,
                DmsResponseType::TERMINATE,
                LastSequenceControl::Unsupported,
                &[],
            ),
            now(106),
        )
        .unwrap();
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 4, now(107))
            .unwrap(),
        GroupReception::SequenceUnavailable {
            last_sequence: LastSequenceControl::Unsupported
        }
    );
    station.poll(now(306)).unwrap();
    assert_eq!(station.entries().count(), 0);
}

#[test]
fn incomplete_response_is_atomic_and_full_denial_hint_is_retained() {
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let a = attributes(1);
    let b = attributes(2);
    let wire = request(
        1,
        &[(0, DmsRequestType::ADD, &a), (0, DmsRequestType::ADD, &b)],
    );
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(0))
        .unwrap();
    station.admitted(id, now(0)).unwrap();
    let mut reply = response(
        1,
        9,
        DmsResponseType::ACCEPT,
        LastSequenceControl::Unsupported,
        &a,
    );
    assert_eq!(
        station.receive(peer(1), &reply, now(1)),
        Err(DmsError::IncompleteResponse)
    );
    assert_eq!(station.entries().count(), 0);
    let hint = [221, 2, 5, 6];
    let denied = response(
        1,
        0,
        DmsResponseType::DENIED,
        LastSequenceControl::Unsupported,
        &hint,
    );
    reply.extend_from_slice(&denied[3..]);
    station.receive(peer(1), &reply, now(2)).unwrap();
    assert_eq!(station.entries().count(), 1);
    assert_eq!(
        station
            .response()
            .unwrap()
            .statuses()
            .unwrap()
            .last()
            .unwrap()
            .attributes
            .as_bytes(),
        hint
    );
}

#[test]
fn classifier_requires_destination_and_never_invents_fragment_ports() {
    let mut bytes = [0; 64];
    let n = TclasRule {
        user_priority: 0,
        mask: 5 | 16 | 32,
        parameters: TclasParameters::Ipv4 {
            kind: 4,
            source: [0; 4],
            destination: [239, 1, 2, 3],
            source_port: 0,
            destination_port: 5353,
            dscp: 0xc8,
            protocol: 17,
            reserved: 0,
        },
    }
    .encode(&mut bytes)
    .unwrap();
    let mut attrs = bytes[..n].to_vec();
    let elements = Elements::parse(&attrs).unwrap();
    let mut p = packet(1);
    p.ip = Some(IpFields::V4 {
        source: [1; 4],
        destination: [239, 1, 2, 3],
        dscp: 8,
        protocol: 17,
        ports: None,
    });
    assert!(!matches_classifiers(elements, p).unwrap());
    p.ip = Some(IpFields::V4 {
        source: [1; 4],
        destination: [239, 1, 2, 3],
        dscp: 8,
        protocol: 17,
        ports: Some(Ports {
            source: 123,
            destination: 5353,
        }),
    });
    assert!(matches_classifiers(elements, p).unwrap());
    attrs[4] = 1;
    assert_eq!(
        admit_classifiers(Elements::parse(&attrs).unwrap()),
        Err(DmsError::MissingDestination)
    );
}

#[test]
fn autonomous_termination_retains_response_and_does_not_extend_duplicate_history() {
    let attrs = attributes(1);
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    grant(&mut station, &attrs);
    let mut registry = DmsRegistry::<4, 128>::new(bss()).unwrap();
    let id = Identities::new(peer(1)).issue().unwrap();
    let wire = request(1, &[(0, DmsRequestType::ADD, &attrs)]);
    let transaction = registry
        .prepare::<4, 256>(id, DmsRequest::parse(&wire).unwrap(), |_| {
            DmsAdmission::Accept
        })
        .unwrap();
    let assigned = transaction
        .response()
        .statuses()
        .unwrap()
        .next()
        .unwrap()
        .dms_id;
    registry.commit(transaction).unwrap();
    let mut ap = DmsAccessPoint::<256>::new(bss(), peer(1), Duration::from_micros(100)).unwrap();
    ap.terminate(
        &mut registry,
        assigned,
        LastSequenceControl::NotGroupTransmitted,
        now(2),
    )
    .unwrap();
    let bytes = ap.transmission().unwrap().body;
    assert_eq!(bytes[2], 0);
    assert_eq!(registry.subscriptions().count(), 0);
    // The station granted id 9, independently of this registry's allocation.
    let mut notice = response(
        0,
        9,
        DmsResponseType::TERMINATE,
        LastSequenceControl::NotGroupTransmitted,
        &[],
    );
    notice.extend_from_slice(&[221, 2, 1, 2]);
    station.receive(peer(1), &notice, now(3)).unwrap();
    assert_eq!(
        station.response().unwrap().elements.as_bytes(),
        &notice[3..]
    );
    let until = station.next_deadline();
    station.receive(peer(1), &notice, now(4)).unwrap();
    assert_eq!(station.next_deadline(), until);
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 1, now(5))
            .unwrap(),
        GroupReception::SequenceUnavailable {
            last_sequence: LastSequenceControl::NotGroupTransmitted
        }
    );
}

#[test]
fn change_and_denial_keep_classifier_membership_and_cancel_requires_recovery() {
    let attrs = attributes(1);
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    grant(&mut station, &attrs);
    let parameters = [221, 2, 4, 5];
    let wire = request(2, &[(9, DmsRequestType::CHANGE, &parameters)]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(2))
        .unwrap();
    station.admitted(id, now(2)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                2,
                9,
                DmsResponseType::ACCEPT,
                LastSequenceControl::Unsupported,
                &parameters,
            ),
            now(3),
        )
        .unwrap();
    let (_, _, attributes) = station.entries().next().unwrap();
    assert_eq!(attributes.unique(221).unwrap(), Some(&[4, 5][..]));
    assert!(equivalent_dms_classifiers(attributes, Elements::parse(&attrs).unwrap()).unwrap());
    let parameters = [221, 1, 6];
    let wire = request(3, &[(9, DmsRequestType::CHANGE, &parameters)]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(4))
        .unwrap();
    station.admitted(id, now(4)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                3,
                9,
                DmsResponseType::DENIED,
                LastSequenceControl::Unsupported,
                &parameters,
            ),
            now(5),
        )
        .unwrap();
    assert_eq!(
        station.entries().next().unwrap().2.unique(221).unwrap(),
        Some(&[4, 5][..])
    );
    let wire = request(4, &[(9, DmsRequestType::REMOVE, &[])]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(6))
        .unwrap();
    station.admitted(id, now(6)).unwrap();
    assert_eq!(
        station.cancel(now(7)).unwrap(),
        DmsStationEvent::RecoveryRequired { id }
    );
}

#[test]
fn autonomous_notice_during_remove_completes_without_extending_sequence_history() {
    let attrs = attributes(1);
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    grant(&mut station, &attrs);
    let wire = request(2, &[(9, DmsRequestType::REMOVE, &[])]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(2))
        .unwrap();
    station.admitted(id, now(2)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                0,
                9,
                DmsResponseType::TERMINATE,
                LastSequenceControl::Sequence(10),
                &[],
            ),
            now(3),
        )
        .unwrap();
    station
        .receive(
            peer(1),
            &response(
                2,
                9,
                DmsResponseType::TERMINATE,
                LastSequenceControl::Unsupported,
                &[],
            ),
            now(4),
        )
        .unwrap();
    assert_eq!(station.next_deadline(), Some(now(203)));
    assert_eq!(
        station.entries().next().unwrap().1,
        DmsEntryState::Retired {
            last_sequence: LastSequenceControl::Sequence(10),
            until: now(203)
        }
    );
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 9, now(5))
            .unwrap(),
        GroupReception::Discard
    );
}

#[test]
fn lost_add_does_not_claim_a_known_multicast_membership() {
    let attrs = attributes(1);
    let mut station = DmsStation::<256, 128, 4>::new(
        peer(1),
        Duration::from_micros(100),
        Duration::from_micros(200),
    )
    .unwrap();
    let wire = request(1, &[(0, DmsRequestType::ADD, &attrs)]);
    let id = station
        .request(DmsRequest::parse(&wire).unwrap(), now(0))
        .unwrap();
    station.admitted(id, now(0)).unwrap();
    station.poll(now(100)).unwrap();
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 1, now(101))
            .unwrap(),
        GroupReception::RecoveryRequired
    );
    assert_eq!(station.entries().count(), 0);
    let retry = station.retry(now(102)).unwrap();
    station.admitted(retry, now(102)).unwrap();
    station
        .receive(
            peer(1),
            &response(
                1,
                9,
                DmsResponseType::ACCEPT,
                LastSequenceControl::Unsupported,
                &attrs,
            ),
            now(103),
        )
        .unwrap();
    assert_eq!(
        station
            .receive_group(peer(1), packet(1), 2, now(104))
            .unwrap(),
        GroupReception::Discard
    );
}
