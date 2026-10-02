use super::*;
use oer_ieee80211_mac::management::{STATUS_SUCCESS, STATUS_UNSUPPORTED_FINITE_CYCLIC_GROUP};

pub(super) fn context() -> AssociationContext<'static> {
    AssociationContext {
        id: ExchangeId {
            addresses: scope().addresses,
            generation: 7,
        },
        ssid: scope().ssid,
        advertisement: profile(OWE_RSN),
        retry: RetryPolicy {
            exchange_us: 1000,
            retry_us: 10,
            max_retries: 3,
        },
    }
}
pub(super) fn groups(group: Group) -> GroupPolicy<'static> {
    GroupPolicy::new(match group {
        Group::P256 => &[Group::P256],
        Group::P384 => &[Group::P384, Group::P256],
        Group::P521 => &[Group::P521, Group::P256],
    })
    .unwrap()
}
fn key(group: Group, value: u8) -> KeyPair {
    KeyPair::from_scalar(group, &scalar(group, value)).unwrap()
}
fn sta(group: Group) -> Station<'static, 512> {
    Station::new(context(), profile(OWE_RSN), groups(group), None, 0).unwrap()
}
fn selected_cache(group: Group) -> std::vec::Vec<u8> {
    let mut bytes = [0; 128];
    let length = RsnElement::parse(OWE_RSN)
        .unwrap()
        .encode_with_pmkids(&[fixture_key(group).pmkid()], &mut bytes)
        .unwrap();
    bytes[..length].to_vec()
}
fn cache(group: Group) -> CachedPmk {
    CachedPmk::new(scope(), fixture_key(group), 0, 900).unwrap()
}
fn ptk_context() -> crate::PtkContext {
    crate::PtkContext {
        authenticator_address: scope().addresses.access_point,
        supplicant_address: scope().addresses.station,
        authenticator_nonce: [1; 32],
        supplicant_nonce: [2; 32],
    }
}

#[test]
fn both_association_roles_complete_dh_once_for_all_groups() {
    for (group, _, _, _, _, ptk) in vectors() {
        let mut sta = sta(group);
        assert!(sta.transmission().is_none());
        sta.provide_key(key(group, 1), 1).unwrap();
        let tx = sta.transmission().unwrap();
        let ticket = tx.ticket;
        let request = tx.bytes.to_vec();
        let request_ticket = ticket;
        assert_eq!(
            sta.response(
                request_ticket,
                STATUS_SUCCESS,
                Elements::parse(OWE_RSN).unwrap(),
                1
            )
            .err(),
            Some(Error::WrongPhase)
        );
        sta.admitted(ticket, 2).unwrap();
        let mut ap = AccessPoint::<512>::new(
            context(),
            Elements::parse(&request).unwrap(),
            groups(group),
            None,
            3,
        )
        .unwrap();
        assert_eq!(ap.phase(), AccessPointPhase::NeedsKey(group));
        ap.provide_key(key(group, 2), 3).unwrap();
        let tx = ap.transmission().unwrap();
        let ticket = tx.ticket;
        let response = tx.bytes.to_vec();
        assert_ne!(ticket, request_ticket);
        assert_eq!(
            sta.response(
                ticket,
                STATUS_SUCCESS,
                Elements::parse(&response).unwrap(),
                3
            )
            .err(),
            Some(Error::WrongContext)
        );
        assert_eq!(ap.admitted(request_ticket, 3), Err(Error::StaleCompletion));
        assert_eq!(
            ap.tx_completed(ticket, true, 3).err(),
            Some(Error::WrongPhase)
        );
        ap.admitted(ticket, 4).unwrap();
        let sta_binding = sta
            .response(
                request_ticket,
                STATUS_SUCCESS,
                Elements::parse(&response).unwrap(),
                5,
            )
            .unwrap()
            .unwrap();
        let ap_binding = ap.tx_completed(ticket, true, 6).unwrap().unwrap();
        assert_eq!(sta.phase(), StationPhase::Completed);
        assert_eq!(ap.phase(), AccessPointPhase::Established);
        assert_eq!(
            sta_binding.derive_ptk(ptk_context()).unwrap().test_bytes(),
            hex(ptk)
        );
        assert_eq!(
            ap_binding.derive_ptk(ptk_context()).unwrap().test_bytes(),
            hex(ptk)
        );
        assert_eq!(
            ap.tx_completed(ticket, true, 7).err(),
            Some(Error::WrongPhase)
        );
        assert!(sta.transmission().is_none());
        assert!(!sta.tick(2000).unwrap());
        assert_eq!(sta.phase(), StationPhase::Completed);
        assert_eq!(ap.tick(1003), Err(Error::TimedOut));
        assert_eq!(ap.phase(), AccessPointPhase::Retired);
        assert!(ap.next_deadline_us().is_none());
    }
}

#[test]
fn retries_keep_the_public_transcript_and_reject_stale_completions() {
    let group = Group::P256;
    let mut sta = sta(group);
    sta.provide_key(key(group, 1), 1).unwrap();
    let first = sta.transmission().unwrap().ticket;
    let request = sta.transmission().unwrap().bytes.to_vec();
    sta.admitted(first, 2).unwrap();
    sta.tx_completed(first, 3).unwrap();
    assert_eq!(sta.next_deadline_us(), Some(13));
    assert!(!sta.tick(12).unwrap());
    assert!(sta.tick(13).unwrap());
    let second = sta.transmission().unwrap();
    assert!(second.retransmission);
    assert_ne!(first, second.ticket);
    assert_eq!(second.bytes, request);
    let second = second.ticket;
    sta.admitted(second, 14).unwrap();
    assert_eq!(sta.tx_completed(first, 15), Err(Error::StaleCompletion));
    let mut ap = AccessPoint::<512>::new(
        context(),
        Elements::parse(&request).unwrap(),
        groups(group),
        None,
        15,
    )
    .unwrap();
    ap.provide_key(key(group, 2), 16).unwrap();
    let first = ap.transmission().unwrap().ticket;
    let response = ap.transmission().unwrap().bytes.to_vec();
    ap.admitted(first, 17).unwrap();
    assert!(
        !ap.duplicate_request(context().id, Elements::parse(&request).unwrap(), 18)
            .unwrap()
    );
    assert!(ap.tx_completed(first, false, 19).unwrap().is_none());
    assert!(ap.tick(29).unwrap());
    let second = ap.transmission().unwrap().ticket;
    assert_eq!(ap.transmission().unwrap().bytes, response);
    ap.admitted(second, 30).unwrap();
    assert_eq!(
        ap.tx_completed(first, true, 31).err(),
        Some(Error::StaleCompletion)
    );
    assert!(ap.tx_completed(second, true, 32).unwrap().is_some());
    // A duplicate after success resends the same response and yields no key.
    assert!(
        ap.duplicate_request(context().id, Elements::parse(&request).unwrap(), 42)
            .unwrap()
    );
    let ticket = ap.transmission().unwrap().ticket;
    assert_eq!(ap.transmission().unwrap().bytes, response);
    ap.admitted(ticket, 43).unwrap();
    assert!(ap.tx_completed(ticket, true, 44).unwrap().is_none());
}

#[test]
fn group_77_advances_explicit_policy_with_fresh_entropy_and_a_new_ticket() {
    let groups = GroupPolicy::new(&[Group::P521, Group::P384, Group::P256]).unwrap();
    let mut sta = Station::<512>::new(context(), profile(OWE_RSN), groups, None, 0).unwrap();
    sta.provide_key(key(Group::P521, 1), 1).unwrap();
    let old = sta.transmission().unwrap().ticket;
    sta.admitted(old, 2).unwrap();
    assert!(
        sta.response(
            old,
            STATUS_UNSUPPORTED_FINITE_CYCLIC_GROUP,
            Elements::parse(&[]).unwrap(),
            3
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(sta.phase(), StationPhase::NeedsKey(Group::P384));
    assert!(sta.transmission().is_none());
    assert_eq!(
        sta.provide_key(key(Group::P521, 2), 4),
        Err(Error::WrongPhase)
    );
    sta.provide_key(key(Group::P384, 1), 5).unwrap();
    let ticket = sta.transmission().unwrap().ticket;
    assert_ne!(ticket, old);
    assert!(!sta.transmission().unwrap().retransmission);
    sta.admitted(ticket, 6).unwrap();
    assert_eq!(sta.tx_completed(old, 7), Err(Error::StaleCompletion));
    assert_eq!(
        sta.response(
            old,
            STATUS_UNSUPPORTED_FINITE_CYCLIC_GROUP,
            Elements::parse(&[]).unwrap(),
            8
        )
        .err(),
        Some(Error::StaleCompletion)
    );
    let mut foreign = context();
    foreign.id.generation += 1;
    let mut other = Station::<512>::new(foreign, profile(OWE_RSN), groups, None, 0).unwrap();
    other.provide_key(key(Group::P521, 1), 1).unwrap();
    let stale = other.transmission().unwrap().ticket;
    assert_eq!(
        sta.response(stale, STATUS_SUCCESS, Elements::parse(&[]).unwrap(), 8)
            .err(),
        Some(Error::WrongContext)
    );
    assert_eq!(sta.phase(), StationPhase::AwaitingResponse);
    // Exhausted group preferences report failure and release the scalar.
    let mut sta = self::sta(Group::P256);
    sta.provide_key(key(Group::P256, 1), 1).unwrap();
    let ticket = sta.transmission().unwrap().ticket;
    sta.admitted(ticket, 2).unwrap();
    assert_eq!(
        sta.response(
            ticket,
            STATUS_UNSUPPORTED_FINITE_CYCLIC_GROUP,
            Elements::parse(&[]).unwrap(),
            3
        )
        .err(),
        Some(Error::UnsupportedSecurity)
    );
    assert_eq!(sta.phase(), StationPhase::Failed);
    assert!(sta.transmission().is_none());
}

#[test]
fn timeout_cancellation_and_invalid_peer_reset_the_attempt() {
    let mut sta = sta(Group::P256);
    sta.provide_key(key(Group::P256, 1), 1).unwrap();
    assert_eq!(sta.tick(1000), Err(Error::TimedOut));
    assert_eq!(sta.phase(), StationPhase::Failed);
    assert!(sta.transmission().is_none());
    let mut sta = self::sta(Group::P256);
    sta.provide_key(key(Group::P256, 1), 1).unwrap();
    let ticket = sta.transmission().unwrap().ticket;
    sta.admitted(ticket, 2).unwrap();
    assert_eq!(
        sta.response(ticket, STATUS_SUCCESS, Elements::parse(OWE_RSN).unwrap(), 3)
            .err(),
        Some(Error::InvalidKey)
    );
    assert_eq!(sta.phase(), StationPhase::Failed);
    assert_eq!(
        sta.provide_key(key(Group::P256, 1), 4),
        Err(Error::WrongPhase)
    );
    let mut sta = self::sta(Group::P256);
    sta.provide_key(key(Group::P256, 1), 1).unwrap();
    sta.cancel();
    assert!(sta.transmission().is_none());
    assert_eq!(
        sta.provide_key(key(Group::P256, 1), 2),
        Err(Error::WrongPhase)
    );
}

#[test]
fn zero_retries_preserve_the_original_full_response_window() {
    let mut context = context();
    context.retry.max_retries = 0;
    let mut sta =
        Station::<512>::new(context, profile(OWE_RSN), groups(Group::P256), None, 0).unwrap();
    sta.provide_key(key(Group::P256, 1), 1).unwrap();
    let ticket = sta.transmission().unwrap().ticket;
    let request = sta.transmission().unwrap().bytes.to_vec();
    sta.admitted(ticket, 2).unwrap();
    sta.tx_completed(ticket, 3).unwrap();
    assert_eq!(sta.next_deadline_us(), Some(1000));
    assert!(!sta.tick(13).unwrap());
    assert_eq!(sta.phase(), StationPhase::AwaitingResponse);
    let mut ap = AccessPoint::<512>::new(
        context,
        Elements::parse(&request).unwrap(),
        groups(Group::P256),
        None,
        14,
    )
    .unwrap();
    ap.provide_key(key(Group::P256, 2), 15).unwrap();
    let response = ap.transmission().unwrap().bytes.to_vec();
    assert!(
        sta.response(
            ticket,
            STATUS_SUCCESS,
            Elements::parse(&response).unwrap(),
            999
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn cached_association_omits_ap_dh_and_rechecks_lease_before_release() {
    let group = Group::P384;
    let selected = selected_cache(group);
    let mut sta = Station::<512>::new(
        context(),
        profile(&selected),
        groups(group),
        Some(cache(group)),
        0,
    )
    .unwrap();
    sta.provide_key(key(group, 1), 1).unwrap();
    let request = sta.transmission().unwrap().bytes.to_vec();
    let request_ticket = sta.transmission().unwrap().ticket;
    sta.admitted(request_ticket, 2).unwrap();
    let mut ap = AccessPoint::<512>::new(
        context(),
        Elements::parse(&request).unwrap(),
        groups(group),
        Some(cache(group)),
        3,
    )
    .unwrap();
    assert_eq!(ap.phase(), AccessPointPhase::Responding);
    let response = ap.transmission().unwrap().bytes.to_vec();
    assert!(
        oer_ieee80211_mac::owe::DhParameter::from_elements(Elements::parse(&response).unwrap())
            .unwrap()
            .is_none()
    );
    let sta_binding = sta
        .response(
            request_ticket,
            STATUS_SUCCESS,
            Elements::parse(&response).unwrap(),
            4,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        sta_binding.derive_ptk(ptk_context()).unwrap().test_bytes(),
        fixture_key(group)
            .derive_ptk(ptk_context())
            .unwrap()
            .test_bytes()
    );
    let ticket = ap.transmission().unwrap().ticket;
    ap.admitted(ticket, 5).unwrap();
    assert_eq!(
        ap.tx_completed(ticket, true, 900).err(),
        Some(Error::KeyExpired)
    );
    assert_eq!(ap.phase(), AccessPointPhase::Failed);
    assert!(ap.transmission().is_none());
}

#[test]
fn ap_admission_rejects_disallowed_groups_and_changed_duplicate_transcripts() {
    assert!(GroupPolicy::new(&[Group::P384]).is_err());
    assert!(GroupPolicy::new(&[Group::P256, Group::P256]).is_err());
    let mut sta = sta(Group::P384);
    sta.provide_key(key(Group::P384, 1), 1).unwrap();
    let request = sta.transmission().unwrap().bytes.to_vec();
    assert!(matches!(
        AccessPoint::<512>::new(
            context(),
            Elements::parse(&request).unwrap(),
            groups(Group::P256),
            None,
            2
        ),
        Err(Error::Wire(
            oer_ieee80211_mac::owe::WireError::UnsupportedGroup(20)
        ))
    ));
    let mut ap = AccessPoint::<512>::new(
        context(),
        Elements::parse(&request).unwrap(),
        groups(Group::P384),
        None,
        2,
    )
    .unwrap();
    ap.provide_key(key(Group::P384, 2), 3).unwrap();
    let mut changed = request.clone();
    *changed.last_mut().unwrap() ^= 1;
    assert_eq!(
        ap.duplicate_request(context().id, Elements::parse(&changed).unwrap(), 4),
        Err(Error::WrongContext)
    );
    assert!(ap.transmission().is_some());
}

#[test]
fn completion_near_clock_limit_respects_deadline_without_overflowing() {
    let mut context = context();
    context.retry.exchange_us = 10;
    context.retry.retry_us = u64::MAX;
    let start = u64::MAX - 20;
    let mut sta =
        Station::<512>::new(context, profile(OWE_RSN), groups(Group::P256), None, start).unwrap();
    sta.provide_key(key(Group::P256, 1), start).unwrap();
    let request = sta.transmission().unwrap().bytes.to_vec();
    let ticket = sta.transmission().unwrap().ticket;
    sta.admitted(ticket, start + 1).unwrap();
    sta.tx_completed(ticket, start + 8).unwrap();
    assert_eq!(sta.next_deadline_us(), Some(start + 10));
    assert_eq!(sta.tick(start + 7), Err(Error::TimeWentBackwards));
    let mut ap = AccessPoint::<512>::new(
        context,
        Elements::parse(&request).unwrap(),
        groups(Group::P256),
        None,
        start,
    )
    .unwrap();
    ap.provide_key(key(Group::P256, 2), start + 1).unwrap();
    let ticket = ap.transmission().unwrap().ticket;
    ap.admitted(ticket, start + 2).unwrap();
    assert!(ap.tx_completed(ticket, true, start + 8).unwrap().is_some());
    assert_eq!(ap.next_deadline_us(), Some(start + 10));
    assert!(matches!(
        Station::<512>::new(
            context,
            profile(OWE_RSN),
            groups(Group::P256),
            None,
            u64::MAX - 5
        ),
        Err(Error::DeadlineOverflow)
    ));
}

#[test]
fn an_ap_cache_miss_uses_fresh_dh_instead_of_the_offered_pmk() {
    let group = Group::P256;
    let selected = selected_cache(group);
    let mut sta = Station::<512>::new(
        context(),
        profile(&selected),
        groups(group),
        Some(cache(group)),
        0,
    )
    .unwrap();
    sta.provide_key(key(group, 3), 1).unwrap();
    let ticket = sta.transmission().unwrap().ticket;
    let request = sta.transmission().unwrap().bytes.to_vec();
    sta.admitted(ticket, 2).unwrap();
    let mut ap = AccessPoint::<512>::new(
        context(),
        Elements::parse(&request).unwrap(),
        groups(group),
        None,
        3,
    )
    .unwrap();
    assert_eq!(ap.phase(), AccessPointPhase::NeedsKey(group));
    ap.provide_key(key(group, 2), 4).unwrap();
    let response = ap.transmission().unwrap().bytes.to_vec();
    let ap_ticket = ap.transmission().unwrap().ticket;
    ap.admitted(ap_ticket, 5).unwrap();
    let sta_binding = sta
        .response(
            ticket,
            STATUS_SUCCESS,
            Elements::parse(&response).unwrap(),
            6,
        )
        .unwrap()
        .unwrap();
    let ap_binding = ap.tx_completed(ap_ticket, true, 7).unwrap().unwrap();
    assert_ne!(sta_binding.pmkid(), fixture_key(group).pmkid());
    assert_eq!(sta_binding.pmkid(), ap_binding.pmkid());
    assert_eq!(
        sta_binding.derive_ptk(ptk_context()).unwrap().test_bytes(),
        ap_binding.derive_ptk(ptk_context()).unwrap().test_bytes()
    );
}

#[test]
fn an_invalid_ap_request_point_fails_without_a_response_or_pmk() {
    let mut sta = sta(Group::P256);
    sta.provide_key(key(Group::P256, 1), 1).unwrap();
    let mut request = sta.transmission().unwrap().bytes.to_vec();
    // OpenSSL rejects the compact P256 point x=1. Its IE framing stays valid.
    let coordinate = Group::P256.coordinate_len();
    let start = request.len() - coordinate;
    request[start..].fill(0);
    *request.last_mut().unwrap() = 1;
    let mut ap = AccessPoint::<512>::new(
        context(),
        Elements::parse(&request).unwrap(),
        groups(Group::P256),
        None,
        2,
    )
    .unwrap();
    assert_eq!(
        ap.provide_key(key(Group::P256, 2), 3),
        Err(Error::InvalidKey)
    );
    assert_eq!(ap.phase(), AccessPointPhase::Failed);
    assert!(ap.transmission().is_none());
    assert_eq!(
        ap.provide_key(key(Group::P256, 2), 4),
        Err(Error::WrongPhase)
    );
}
