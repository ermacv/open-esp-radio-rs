use super::*;
use oer_ieee80211_mac::security::{AssociationAkm, GroupManagementCipher, RsnAssociation};
fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
fn at(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn timeout() -> Duration {
    Duration::from_secs(1)
}
fn timing(now: Instant) -> SleepTiming {
    SleepTiming {
        beacon_interval_tu: 100,
        dtim_period: 2,
        next_dtim: now,
        maximum_idle: None,
    }
}
fn security(pmf: bool) -> AssociationSecurity {
    AssociationSecurity::Rsn(RsnAssociation {
        akm: AssociationAkm::Psk,
        management: pmf.then_some(GroupManagementCipher::BipCmac128),
        pmkid: None,
    })
}
fn request(action: u8, token: u8, interval: u16) -> std::vec::Vec<u8> {
    let [low, high] = interval.to_le_bytes();
    [
        std::vec![10, 16, token, 93, 4, action, 0, low, high],
        traffic::tests::request(),
    ]
    .concat()
}
fn response(action: u8, token: u8, status: u8, interval: u16, keys: &[u8]) -> std::vec::Vec<u8> {
    let [low, high] = interval.to_le_bytes();
    let len = (keys.len() as u16).to_le_bytes();
    [
        std::vec![10, 17, token, len[0], len[1]],
        keys.to_vec(),
        std::vec![93, 4, action, status, low, high, 92, 4, 1, 2, 0, 7],
    ]
    .concat()
}
fn keys() -> std::vec::Vec<u8> {
    let mut keys = std::vec![0, 27, 1, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0];
    keys.extend_from_slice(&[0xab; 16]);
    keys.extend_from_slice(&[1, 24, 4, 0, 0, 0, 0, 0, 0, 0]);
    keys.extend_from_slice(&[0xcd; 16]);
    keys
}
fn enter(owner: &mut WnmSleepStation<256>) -> OperationId {
    let bytes = request(0, 1, 3);
    let id = owner
        .request(
            WnmSleepRequest::parse(&bytes).unwrap(),
            timing(at(0)),
            at(0),
        )
        .unwrap();
    owner.admitted(id, at(0)).unwrap();
    let bytes = response(0, 1, 0, 3, &[]);
    let SleepEvent::ApplyRequired { services, .. } = owner.receive(link(), &bytes, at(1)).unwrap()
    else {
        panic!()
    };
    owner.services_applied(id, services, at(2)).unwrap();
    id
}
#[test]
fn entering_sleep_requires_key_retirement_and_filters_before_state_changes() {
    let mut owner = WnmSleepStation::<256>::new(link(), security(true), timeout()).unwrap();
    let bytes = request(0, 1, 3);
    let id = owner
        .request(
            WnmSleepRequest::parse(&bytes).unwrap(),
            timing(at(0)),
            at(0),
        )
        .unwrap();
    owner.admitted(id, at(0)).unwrap();
    let bytes = response(0, 1, 0, 3, &[]);
    let SleepEvent::ApplyRequired { services, .. } = owner.receive(link(), &bytes, at(1)).unwrap()
    else {
        panic!()
    };
    assert!(services.contains(SleepServices::RETIRE_GTK));
    assert!(services.contains(SleepServices::RETIRE_IGTK));
    assert_eq!(owner.state(), SleepState::Awake);
    assert_eq!(
        owner.services_applied(id, SleepServices::FILTERS_AND_POWER, at(2)),
        Err(SleepError::ServicesNotApplied)
    );
    assert_eq!(
        owner.services_applied(id, services, at(2)).unwrap(),
        SleepEvent::Applied {
            id,
            state: SleepState::Asleep { interval_dtim: 3 }
        }
    );
    assert_eq!(
        owner.receive(link(), &bytes, at(3)).unwrap(),
        SleepEvent::Ignored
    );
    assert_eq!(
        owner.services_applied(id, services, at(3)).unwrap(),
        SleepEvent::Ignored
    );
    assert_eq!(owner.next_deadline(), Some(at(614_400)));
    assert_eq!(
        owner.poll(at(614_400)).unwrap(),
        SleepEvent::CheckTim { link: link() }
    );
    assert_eq!(owner.next_deadline(), Some(at(1_228_800)));
}
#[test]
fn protected_exit_requires_complete_keys_and_ignores_late_tx_and_old_epoch() {
    let mut owner = WnmSleepStation::<256>::new(link(), security(true), timeout()).unwrap();
    let first = enter(&mut owner);
    let bytes = request(1, 2, 0);
    let id = owner
        .request(
            WnmSleepRequest::parse(&bytes).unwrap(),
            timing(at(10)),
            at(10),
        )
        .unwrap();
    owner.admitted(id, at(10)).unwrap();
    assert_ne!(first, id);
    assert_eq!(
        owner.receive(link(), &response(1, 2, 1, 0, &[]), at(11)),
        Err(SleepError::MissingKeys)
    );
    let bytes = response(1, 2, 1, 0, &keys());
    assert_eq!(
        owner
            .receive(
                LinkIdentity {
                    generation: 0,
                    ..link()
                },
                &bytes,
                at(11)
            )
            .unwrap(),
        SleepEvent::Ignored
    );
    let SleepEvent::ApplyRequired { services, .. } = owner.receive(link(), &bytes, at(11)).unwrap()
    else {
        panic!()
    };
    assert!(services.contains(SleepServices::INSTALL_GTK));
    assert!(services.contains(SleepServices::INSTALL_IGTK));
    assert_eq!(owner.application().unwrap().2.keys.keys().count(), 2);
    assert_eq!(
        owner.tx_completed(id, TxOutcome::Failed, at(12)).unwrap(),
        SleepEvent::Ignored
    );
    owner.services_applied(id, services, at(13)).unwrap();
    assert_eq!(owner.state(), SleepState::Awake);
}
#[test]
fn rsn_exit_without_pmf_requires_group_rekey_and_rejects_injected_keys() {
    let mut owner = WnmSleepStation::<256>::new(link(), security(false), timeout()).unwrap();
    enter(&mut owner);
    let bytes = request(1, 2, 0);
    let id = owner
        .request(
            WnmSleepRequest::parse(&bytes).unwrap(),
            timing(at(10)),
            at(10),
        )
        .unwrap();
    owner.admitted(id, at(10)).unwrap();
    assert_eq!(
        owner.receive(link(), &response(1, 2, 1, 0, &keys()), at(11)),
        Err(SleepError::KeyDataWithoutProtection)
    );
    let SleepEvent::ApplyRequired { services, .. } = owner
        .receive(link(), &response(1, 2, 1, 0, &[]), at(11))
        .unwrap()
    else {
        panic!()
    };
    assert!(services.contains(SleepServices::GROUP_REKEY));
    assert_eq!(
        owner.services_applied(id, SleepServices::FILTERS_AND_POWER, at(12)),
        Err(SleepError::ServicesNotApplied)
    );
    owner.services_applied(id, services, at(13)).unwrap();
    assert_eq!(owner.state(), SleepState::Awake);
}
#[test]
fn delivered_request_or_unapplied_acceptance_timeout_requires_recovery() {
    let mut owner =
        WnmSleepStation::<256>::new(link(), AssociationSecurity::Open, timeout()).unwrap();
    let bytes = request(0, 1, 0);
    let id = owner
        .request(
            WnmSleepRequest::parse(&bytes).unwrap(),
            timing(at(0)),
            at(0),
        )
        .unwrap();
    assert_eq!(
        owner.poll(at(1_000_000)).unwrap(),
        SleepEvent::RequestCancelled { id }
    );
    assert_eq!(owner.state(), SleepState::Awake);
    let id = owner
        .request(
            WnmSleepRequest::parse(&bytes).unwrap(),
            timing(at(1_000_000)),
            at(1_000_000),
        )
        .unwrap();
    owner.admitted(id, at(1_000_000)).unwrap();
    owner
        .receive(link(), &response(0, 1, 0, 0, &[]), at(1_000_001))
        .unwrap();
    assert_eq!(
        owner.poll(at(2_000_000)).unwrap(),
        SleepEvent::RecoveryRequired { id }
    );
    assert_eq!(owner.state(), SleepState::Unknown);
    let bytes = request(1, 2, 0);
    assert!(
        owner
            .request(
                WnmSleepRequest::parse(&bytes).unwrap(),
                timing(at(2_000_001)),
                at(2_000_001)
            )
            .is_ok()
    );
}
#[test]
fn sleep_interval_must_fit_maximum_idle_and_invalid_response_preserves_request() {
    let mut owner = WnmSleepStation::<256>::new(link(), security(true), timeout()).unwrap();
    let bytes = request(0, 1, 6);
    let policy = SleepTiming {
        maximum_idle: Some(BssMaxIdle {
            period: 1,
            options: 1,
        }),
        ..timing(at(0))
    };
    assert_eq!(
        owner.request(WnmSleepRequest::parse(&bytes).unwrap(), policy, at(0)),
        Err(SleepError::IdleConflict)
    );
    let bytes = request(0, 1, 3);
    let id = owner
        .request(WnmSleepRequest::parse(&bytes).unwrap(), policy, at(0))
        .unwrap();
    owner.admitted(id, at(0)).unwrap();
    assert_eq!(
        owner.receive(link(), &response(0, 1, 1, 3, &[]), at(1)),
        Err(SleepError::UnsupportedStatus(1))
    );
    assert_eq!(
        owner.receive(link(), &response(1, 1, 0, 3, &[]), at(1)),
        Err(SleepError::Protocol(Error::ConflictingDialog))
    );
    assert!(matches!(
        owner
            .receive(link(), &response(0, 1, 0, 3, &[]), at(2))
            .unwrap(),
        SleepEvent::ApplyRequired { .. }
    ));
}
#[test]
fn ap_applies_services_before_response_and_replays_without_reapplication() {
    let mut owner = WnmSleepAccessPoint::<256>::new(link(), security(true), timeout()).unwrap();
    let bytes = request(0, 7, 3);
    let SleepApEvent::Requested { id } = owner.receive(link(), &bytes, at(0)).unwrap() else {
        panic!()
    };
    let response_bytes = response(0, 7, 0, 3, &[]);
    let reply = WnmSleepResponse::parse(&response_bytes).unwrap();
    let plan = owner
        .prepare_response(id, reply, timing(at(1)), at(1))
        .unwrap();
    assert_eq!(owner.state(), SleepState::Awake);
    assert!(owner.transmission().is_none());
    let tx = owner.services_applied(plan, at(1)).unwrap();
    assert_ne!(id, tx);
    assert_eq!(owner.state(), SleepState::Asleep { interval_dtim: 3 });
    owner.admitted(tx, at(1)).unwrap();
    owner
        .tx_completed(tx, TxOutcome::Acknowledged, at(2))
        .unwrap();
    let SleepApEvent::ReplayQueued { id: replay } = owner.receive(link(), &bytes, at(10)).unwrap()
    else {
        panic!()
    };
    assert_ne!(replay, tx);
    assert_eq!(owner.transmission().unwrap().body, response_bytes);
    assert_eq!(owner.next_deadline(), Some(at(1_000_000)));
    let changed = request(0, 7, 4);
    assert_eq!(
        owner.receive(link(), &changed, at(11)),
        Err(SleepError::Protocol(Error::ConflictingDialog))
    );
}
#[test]
fn conflicting_key_ids_and_unknown_key_types_fail_before_application() {
    let keys = keys();
    let duplicate = [keys.clone(), keys].concat();
    assert_eq!(
        validate_response(
            WnmSleepElement {
                action: SleepAction::EXIT,
                status: SleepStatus::ACCEPT,
                interval_dtim: 0
            },
            SleepKeyData::parse(&duplicate).unwrap(),
            security(true)
        ),
        Err(SleepError::ConflictingKeys)
    );
    assert_eq!(
        validate_response(
            WnmSleepElement {
                action: SleepAction::EXIT,
                status: SleepStatus::ACCEPT,
                interval_dtim: 0
            },
            SleepKeyData::parse(&[9, 1, 0]).unwrap(),
            security(true)
        ),
        Err(SleepError::UnsupportedKey(9))
    );
}

#[test]
fn ap_preparation_checks_capacity_timing_and_expiry_before_or_after_application() {
    let mut owner =
        WnmSleepAccessPoint::<256>::new(link(), AssociationSecurity::Open, timeout()).unwrap();
    let wire = request(0, 1, 3);
    let SleepApEvent::Requested { id } = owner.receive(link(), &wire, at(0)).unwrap() else {
        panic!()
    };
    let reply = response(0, 1, 0, 3, &[]);
    let mut invalid = timing(at(1));
    invalid.maximum_idle = Some(BssMaxIdle {
        period: 1,
        options: 0,
    });
    invalid.dtim_period = 6;
    assert!(matches!(
        owner.prepare_response(id, WnmSleepResponse::parse(&reply).unwrap(), invalid, at(1)),
        Err(SleepError::IdleConflict)
    ));
    assert!(owner.transmission().is_none());
    assert_eq!(owner.state(), SleepState::Awake);
    let plan = owner
        .prepare_response(
            id,
            WnmSleepResponse::parse(&reply).unwrap(),
            timing(at(1)),
            at(1),
        )
        .unwrap();
    owner.cancel_response(plan, at(2)).unwrap();
    assert_eq!(owner.state(), SleepState::Awake);
    let plan = owner
        .prepare_response(
            id,
            WnmSleepResponse::parse(&reply).unwrap(),
            timing(at(3)),
            at(3),
        )
        .unwrap();
    assert_eq!(
        owner.poll(at(1_000_000)).unwrap(),
        SleepApEvent::RecoveryRequired { id }
    );
    assert_eq!(
        owner.services_applied(plan, at(1_000_000)),
        Err(SleepError::RecoveryRequired)
    );
    assert_eq!(owner.state(), SleepState::Unknown);
    let mut tiny =
        WnmSleepAccessPoint::<64>::new(link(), AssociationSecurity::Open, timeout()).unwrap();
    let SleepApEvent::Requested { id } = tiny.receive(link(), &wire, at(0)).unwrap() else {
        panic!()
    };
    let mut large = reply.clone();
    large.extend_from_slice(&[221, 64]);
    large.extend_from_slice(&[7; 64]);
    assert!(matches!(
        tiny.prepare_response(
            id,
            WnmSleepResponse::parse(&large).unwrap(),
            timing(at(1)),
            at(1)
        ),
        Err(SleepError::Protocol(Error::FrameTooLarge { .. }))
    ));
    assert_eq!(tiny.state(), SleepState::Awake);
}

#[test]
fn ap_application_reuses_absolute_lease_without_overflowing_a_new_relative_timeout() {
    let mut owner = WnmSleepAccessPoint::<256>::new(
        link(),
        AssociationSecurity::Open,
        Duration::from_micros(1000),
    )
    .unwrap();
    let wire = request(1, 1, 0);
    let SleepApEvent::Requested { id } = owner.receive(link(), &wire, at(u64::MAX - 1000)).unwrap()
    else {
        panic!()
    };
    let reply = response(1, 1, 0, 0, &[]);
    let plan = owner
        .prepare_response(
            id,
            WnmSleepResponse::parse(&reply).unwrap(),
            timing(at(u64::MAX - 2)),
            at(u64::MAX - 2),
        )
        .unwrap();
    owner.services_applied(plan, at(u64::MAX - 1)).unwrap();
    assert_eq!(owner.next_deadline(), Some(at(u64::MAX)));
    assert_eq!(owner.state(), SleepState::Awake);
}
