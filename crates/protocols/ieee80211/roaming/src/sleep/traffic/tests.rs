use super::*;
use crate::Error;
use crate::tclas::ClassifierPacket;
use crate::tfs::{admit_filter, filter_matches};
use oer_ieee80211_mac::roaming::*;
fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
pub(crate) fn request() -> std::vec::Vec<u8> {
    let mut body = [0; 32];
    let n = TclasRule {
        user_priority: 0,
        mask: 4,
        parameters: TclasParameters::Ethernet {
            source: [0; 6],
            destination: [0; 6],
            ether_type: 0x0800,
        },
    }
    .encode(&mut body)
    .unwrap();
    let mut bytes = std::vec![91, (4 + n) as u8, 7, 3, 1, n as u8];
    bytes.extend_from_slice(&body[..n]);
    bytes
}
fn reply() -> Elements<'static> {
    Elements::parse(&[92, 4, 1, 2, 0, 7, 221, 1, 8]).unwrap()
}
fn input() -> TrafficInput<'static> {
    TrafficInput {
        fields: ClassifierPacket {
            source: [2, 0, 0, 0, 0, 2],
            destination: link().peer,
            ether_type: 0x0800,
            ip: None,
        },
        payload: &[],
        vlan_tci: None,
        eapol_key: false,
    }
}
#[test]
fn sleep_filter_triggers_are_deleted_only_after_notifications_and_data_are_queued() {
    let mut owner = SleepTrafficFilters::<128, 4>::new(link());
    let request = request();
    owner
        .install(link(), Elements::parse(&request).unwrap(), reply())
        .unwrap();
    let decision = owner.evaluate(link(), input()).unwrap();
    assert!(!decision.discard_individual);
    assert!(decision.set_tim);
    assert_eq!(decision.notifications[0], Some(7));
    let mut notify = [0; 4];
    decision.encode_notification(&mut notify).unwrap();
    assert_eq!(notify, [10, 15, 1, 7]);
    assert_eq!(
        owner.applied(decision.id, false, true),
        Err(TrafficError::Protocol(Error::NotAdmitted))
    );
    owner.cancel(decision.id).unwrap();
    let retry = owner.evaluate(link(), input()).unwrap();
    assert_eq!(retry.notifications[0], Some(7));
    owner.applied(retry.id, true, true).unwrap();
    let next = owner.evaluate(link(), input()).unwrap();
    assert!(next.discard_individual);
    assert!(!next.set_tim);
    owner.cancel(next.id).unwrap();
    assert_eq!(owner.negotiation().1.unique(221).unwrap(), Some(&[8][..]));
}
#[test]
fn mandatory_key_filter_and_group_delivery_survive_filtering() {
    let mut owner = SleepTrafficFilters::<128, 4>::new(link());
    let request = request();
    owner
        .install(link(), Elements::parse(&request).unwrap(), reply())
        .unwrap();
    let mut key = input();
    key.fields.ether_type = 0x888e;
    key.eapol_key = true;
    let decision = owner.evaluate(link(), key).unwrap();
    assert!(!decision.discard_individual);
    assert!(decision.set_tim);
    assert!(decision.notifications.iter().all(Option::is_none));
    owner.applied(decision.id, false, true).unwrap();
    key.eapol_key = false;
    let decision = owner.evaluate(link(), key).unwrap();
    assert!(decision.discard_individual);
    owner.applied(decision.id, false, false).unwrap();
    key.fields.destination = [1; 6];
    let decision = owner.evaluate(link(), key).unwrap();
    assert!(!decision.discard_individual);
    owner.cancel(decision.id).unwrap();
    assert_eq!(
        owner.evaluate(
            LinkIdentity {
                generation: 0,
                ..link()
            },
            input()
        ),
        Err(TrafficError::Protocol(Error::WrongOperation))
    );
}
#[test]
fn accepted_filter_negotiation_is_atomic_and_requires_exact_status_coverage() {
    let mut owner = SleepTrafficFilters::<128, 4>::new(link());
    let request = request();
    owner
        .install(link(), Elements::parse(&request).unwrap(), reply())
        .unwrap();
    let missing = Elements::parse(&[92, 4, 1, 2, 0, 8]).unwrap();
    assert_eq!(
        owner.install(link(), Elements::parse(&request).unwrap(), missing),
        Err(TrafficError::IncompleteResponse)
    );
    assert_eq!(owner.negotiation().1.as_bytes(), reply().as_bytes());
    let duplicate = Elements::parse(&[92, 8, 1, 2, 0, 7, 1, 2, 0, 7]).unwrap();
    assert_eq!(
        validate_sleep_filters(Elements::parse(&request).unwrap(), duplicate),
        Err(TrafficError::IncompleteResponse)
    );
}
#[test]
fn offset_masks_and_vlan_fields_use_plaintext_and_network_order() {
    let mut body = [0; 32];
    let n = TclasRule {
        user_priority: 0,
        mask: 0,
        parameters: TclasParameters::Filter {
            offset: 1,
            value: &[0xa0, 0x12],
            mask: &[0xf0, 0xff],
        },
    }
    .encode(&mut body)
    .unwrap();
    let mut wire = std::vec![7, 0, 1, n as u8];
    wire.extend_from_slice(&body[..n]);
    let filter = TfsRequest::parse(&wire).unwrap();
    admit_filter(filter).unwrap();
    let mut frame = input();
    frame.payload = &[0, 0xab, 0x12];
    assert!(filter_matches(filter, frame).unwrap());
    frame.payload = &[0, 0xab];
    assert!(!filter_matches(filter, frame).unwrap());
    let n = TclasRule {
        user_priority: 0,
        mask: 7,
        parameters: TclasParameters::Vlan {
            pcp: 3,
            cfi: 1,
            vid: 42,
        },
    }
    .encode(&mut body)
    .unwrap();
    let mut wire = std::vec![7, 0, 1, n as u8];
    wire.extend_from_slice(&body[..n]);
    let filter = TfsRequest::parse(&wire).unwrap();
    frame.vlan_tci = Some((3 << 13) | (1 << 12) | 42);
    assert!(filter_matches(filter, frame).unwrap());
    frame.vlan_tci = None;
    assert!(!filter_matches(filter, frame).unwrap());
}
