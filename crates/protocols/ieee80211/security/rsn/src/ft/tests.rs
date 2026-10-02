use super::*;
use crate::Pmk;

const STA: MacAddress = [2, 0, 0, 0, 0, 1];
const OLD: MacAddress = [2, 0, 0, 0, 0, 2];
const AP: MacAddress = [2, 0, 0, 0, 0, 3];
const R1: R1khId = R1khId(OLD);
const TIMEOUTS: Timeouts = Timeouts {
    authentication_us: 100_000,
    reassociation_us: 100_000,
    commit_us: 100_000,
    retry_interval_us: 1_000,
    max_retries: 2,
};

fn root(akm: FtAkm) -> PmkR0 {
    let key = Pmk::from_bytes(core::array::from_fn(|index| index as u8));
    let initial = match akm {
        FtAkm::Psk => InitialKey::psk(key),
        FtAkm::Sae => InitialKey::authenticated_sae(
            key,
            oer_ieee80211_mac::security::SaePwe::HuntingAndPecking,
        ),
        FtAkm::Ieee8021X => {
            InitialKey::authenticated_eap_msk(&core::array::from_fn(|index| index as u8))
        }
    };
    initial.derive_r0(RootContext {
        ssid: WifiSsid::new(b"FT-test").unwrap(),
        mobility_domain: MobilityDomainId([0x12, 0x34]),
        r0kh: R0khId::new(b"r0.example").unwrap(),
        station: STA,
    })
}
fn identity() -> SessionIdentity {
    SessionIdentity {
        station: STA,
        current_ap: OLD,
        generation: 1,
    }
}
fn domain() -> MobilityDomain {
    MobilityDomain {
        id: MobilityDomainId([0x12, 0x34]),
        over_ds: true,
        resource_request: true,
    }
}
fn advertised(akm: FtAkm, management: bool) -> std::vec::Vec<u8> {
    let mut bytes = std::vec![48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 1, 0];
    bytes.extend_from_slice(&akm.suite_selector());
    bytes.extend_from_slice(&(if management { 0x00c0_u16 } else { 0 }).to_le_bytes());
    if management {
        bytes.extend_from_slice(&[0, 0, 0, 15, 172, 6]);
        bytes[1] += 6;
    }
    bytes
}
fn groups(management: bool) -> TransitionGroupKeys {
    TransitionGroupKeys {
        gtk: crate::frames::RsnGtk::new(2, false, [0x66; 16]).unwrap(),
        receive_sequence: [7; 8],
        igtk: management.then(|| crate::frames::RsnIgtk::new(4, [9; 6], [0x88; 16]).unwrap()),
    }
}
fn hex<const N: usize>(value: &str) -> [u8; N] {
    assert_eq!(value.len(), N * 2);
    core::array::from_fn(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
}

#[test]
fn independent_sha256_hierarchy_vectors_and_wrong_station_rejection() {
    // Independently computed with Python hashlib/hmac over the standard KDF.
    let r0 = root(FtAkm::Psk);
    assert_eq!(r0.name(), hex("c6670840f3644c44843d80f27cef51a7"));
    let r1 = r0.derive_r1(R1);
    assert_eq!(r1.name(), hex("4e257f7e45273dfeb76b6e54d8322e0c"));
    assert_eq!(
        r1.transfer_key(),
        &hex::<32>("306a4cf5d37d40a2e13599a845217c4e676d4548e116647aca742a420ce1cc5e")
    );
    let context = FtPtkContext {
        addresses: wire::FtAddresses {
            station: STA,
            access_point: AP,
        },
        snonce: core::array::from_fn(|index| 32 + index as u8),
        anonce: core::array::from_fn(|index| 64 + index as u8),
    };
    let ptk = r1.derive_ptk(context).unwrap();
    assert_eq!(ptk.name(), hex("a5e0b5e14d9426fc4abc4db6b4e9f52c"));
    assert_eq!(
        ptk.temporal_key().as_bytes(),
        &hex::<16>("14a98e93bea5eb30df27483a25f5ebb4")
    );
    assert!(matches!(
        r1.derive_ptk(FtPtkContext {
            addresses: wire::FtAddresses {
                station: OLD,
                access_point: AP
            },
            ..context
        }),
        Err(Error::WrongKeyContext)
    ));
    assert_ne!(root(FtAkm::Ieee8021X).name(), r0.name());
}

fn authenticate<'a>(
    station: &mut Station<'a, 512>,
    ap: &mut AccessPoint<'a, 512>,
    r0: &PmkR0,
    transport: Transport,
) {
    let tx = station.transmission().unwrap();
    let id = tx.id;
    let body = tx.bytes.to_vec();
    station.admitted(id, 1).unwrap();
    let event = match transport {
        Transport::OverAir => ap
            .authentication_request(STA, &body, [0x44; 32], 2)
            .unwrap(),
        Transport::OverDs => ap
            .authenticated_ds_request(OLD, &body, [0x44; 32], 2)
            .unwrap(),
    };
    assert_eq!(event, AccessPointEvent::KeyLookupReady);
    let lookup = ap.key_lookup().unwrap();
    ap.key_delivered(
        KeyDelivery {
            request: lookup.id,
            target: AP,
            key: r0.derive_r1(R1),
            expires_at_us: 1_000_000,
        },
        3,
    )
    .unwrap();
    let tx = ap.transmission().unwrap();
    let body = tx.bytes.to_vec();
    let id = tx.id;
    ap.admitted(id, 4).unwrap();
    assert_eq!(
        station
            .authentication_response(
                if transport == Transport::OverAir {
                    AP
                } else {
                    OLD
                },
                &body,
                5
            )
            .unwrap(),
        StationEvent::ReassociationReady
    );
}

#[test]
fn both_transports_and_all_three_suites_complete_with_one_time_key_handoff() {
    for transport in [Transport::OverAir, Transport::OverDs] {
        for akm in [FtAkm::Psk, FtAkm::Sae, FtAkm::Ieee8021X] {
            let r0 = root(akm);
            let management = akm == FtAkm::Sae;
            let rsn = advertised(akm, management);
            let security = SecurityProfile::new(
                &rsn,
                &[],
                akm,
                management,
                management.then_some(oer_ieee80211_mac::security::SaePwe::HuntingAndPecking),
            )
            .unwrap();
            let target = Target {
                bssid: AP,
                ssid: r0.context().ssid,
                domain: domain(),
                security,
            };
            let mut station = Station::<512>::new(&r0, 1_000_000, identity(), TIMEOUTS, 0).unwrap();
            let mut ap = AccessPoint::<512>::new(
                AccessPointProfile {
                    bssid: AP,
                    ssid: target.ssid,
                    domain: domain(),
                    r1kh: R1,
                    security,
                },
                identity(),
                TIMEOUTS,
                0,
            )
            .unwrap();
            station
                .start(target, transport, [0x33; 32], &[], 0)
                .unwrap();
            authenticate(&mut station, &mut ap, &r0, transport);
            let tx = station.transmission().unwrap();
            let id = tx.id;
            let request = tx.bytes.to_vec();
            station.admitted(id, 6).unwrap();
            assert_eq!(
                ap.reassociation_request(STA, &request, 7).unwrap(),
                AccessPointEvent::ResourcesReady
            );
            let (id, ric) = ap.resources().unwrap();
            assert!(ric.is_empty());
            ap.prepare_response(id, &[], &groups(management), 8)
                .unwrap();
            let id = ap.commit().unwrap().id;
            ap.commit_admitted(id, 9).unwrap();
            ap.commit_completed(id, CommitOutcome::Committed, 10)
                .unwrap();
            let tx = ap.transmission().unwrap();
            let id = tx.id;
            let response = tx.bytes.to_vec();
            ap.admitted(id, 11).unwrap();
            assert_eq!(
                station
                    .reassociation_response(AP, 0, &response, 12)
                    .unwrap(),
                StationEvent::CommitReady
            );
            let id_sta = station.commit().unwrap().id;
            station.commit_admitted(id_sta, 13).unwrap();
            station
                .commit_completed(id_sta, CommitOutcome::Committed, 14)
                .unwrap();
            assert_eq!(
                ap.tx_completed(id, TxOutcome::Acknowledged, 14).unwrap(),
                AccessPointEvent::Authorized
            );
            let station_keys = station.take_completed_keys().unwrap();
            let (ap_ptk, _) = ap.take_authorized_keys().unwrap();
            assert_eq!(
                station_keys.pairwise.temporal_key().as_bytes(),
                ap_ptk.temporal_key().as_bytes()
            );
            assert_eq!(station_keys.groups.gtk.key(), &[0x66; 16]);
            assert_eq!(station_keys.groups.igtk.is_some(), management);
            assert!(station.take_completed_keys().is_none());
            assert!(ap.take_authorized_keys().is_none());
            assert_eq!(
                ap.reassociation_request(STA, &request, 15).unwrap(),
                AccessPointEvent::None
            );
            assert!(ap.commit().is_none());
            assert_eq!(ap.transmission().unwrap().bytes, response);
        }
    }
}

fn setup<'a>(
    r0: &'a PmkR0,
    rsn: &'a [u8],
    rsnxe: &'a [u8],
    pwe: Option<oer_ieee80211_mac::security::SaePwe>,
) -> (Station<'a, 512>, AccessPoint<'a, 512>, Target<'a>) {
    let security = SecurityProfile::new(rsn, rsnxe, r0.akm(), r0.akm() == FtAkm::Sae, pwe).unwrap();
    let target = Target {
        bssid: AP,
        ssid: r0.context().ssid,
        domain: domain(),
        security,
    };
    (
        Station::new(r0, 1_000_000, identity(), TIMEOUTS, 0).unwrap(),
        AccessPoint::new(
            AccessPointProfile {
                bssid: AP,
                ssid: target.ssid,
                domain: domain(),
                r1kh: R1,
                security,
            },
            identity(),
            TIMEOUTS,
            0,
        )
        .unwrap(),
        target,
    )
}
fn ready_response(
    station: &mut Station<'_, 512>,
    ap: &mut AccessPoint<'_, 512>,
    management: bool,
) -> std::vec::Vec<u8> {
    let tx = station.transmission().unwrap();
    let id = tx.id;
    let request = tx.bytes.to_vec();
    station.admitted(id, 6).unwrap();
    ap.reassociation_request(STA, &request, 7).unwrap();
    let (id, _) = ap.resources().unwrap();
    ap.prepare_response(id, &[], &groups(management), 8)
        .unwrap();
    let id = ap.commit().unwrap().id;
    ap.commit_admitted(id, 9).unwrap();
    ap.commit_completed(id, CommitOutcome::Committed, 10)
        .unwrap();
    ap.transmission().unwrap().bytes.to_vec()
}

#[test]
fn ft_mics_match_independent_cmac_vectors_and_bind_transaction_and_addresses() {
    use super::protocol::{IeRequest, build_ies};
    let r0 = root(FtAkm::Psk);
    let r1 = r0.derive_r1(R1);
    let rsn = advertised(FtAkm::Psk, false);
    let security = SecurityProfile::new(&rsn, &[], FtAkm::Psk, false, None).unwrap();
    let context = FtPtkContext {
        addresses: wire::FtAddresses {
            station: STA,
            access_point: AP,
        },
        snonce: core::array::from_fn(|i| 32 + i as u8),
        anonce: core::array::from_fn(|i| 64 + i as u8),
    };
    let ptk = r1.derive_ptk(context).unwrap();
    let ies = build_ies::<512>(IeRequest {
        security,
        selected_rsn: true,
        md: domain(),
        root_name: r1.name(),
        r0kh: r0.context().r0kh,
        r1kh: Some(R1),
        snonce: context.snonce,
        anonce: context.anonce,
        ric: &[],
        groups: None,
        protected: Some((&ptk, wire::MicTransaction::ReassociationRequest)),
        reassociation_deadline_tu: None,
    })
    .unwrap();
    let parsed = wire::FtElements::parse(ies.bytes()).unwrap();
    // Independent Python hashlib/hmac KDF and cryptography AES-CMAC, seq 5/6.
    assert_eq!(
        parsed.ft.mic(),
        &hex::<16>("68ac3a232e623188d47096a7058e6a3b")
    );
    assert_eq!(
        ptk.mic(wire::MicTransaction::ReassociationResponse, parsed)
            .unwrap(),
        hex::<16>("951246e6928b368b721e0a92d064df91")
    );
    assert_eq!(
        ptk.verify_mic(wire::MicTransaction::ReassociationResponse, parsed),
        Err(Error::InvalidMic)
    );
    let another = r1
        .derive_ptk(FtPtkContext {
            addresses: wire::FtAddresses {
                access_point: OLD,
                ..context.addresses
            },
            ..context
        })
        .unwrap();
    assert_eq!(
        another.verify_mic(wire::MicTransaction::ReassociationRequest, parsed),
        Err(Error::InvalidMic)
    );
}

#[test]
fn initial_four_way_uses_ft_keys_and_existing_state_ownership_for_all_suites() {
    use crate::state::*;
    use crate::{OwnedEapolFrame, RsnInterface, frames::RsnTxFrame};
    use oer_ieee80211_mac::management::elements::Elements;
    for akm in [FtAkm::Psk, FtAkm::Ieee8021X, FtAkm::Sae] {
        let r0 = root(akm);
        let management = akm == FtAkm::Sae;
        let rsn = advertised(akm, management);
        let security = SecurityProfile::new(
            &rsn,
            &[],
            akm,
            management,
            management.then_some(oer_ieee80211_mac::security::SaePwe::HuntingAndPecking),
        )
        .unwrap();
        let target = Target {
            bssid: AP,
            ssid: r0.context().ssid,
            domain: domain(),
            security,
        };
        let mut sub = std::vec![wire::subelement_id::R1KH, 6];
        sub.extend_from_slice(&R1.0);
        sub.extend_from_slice(&[
            wire::subelement_id::R0KH,
            r0.context().r0kh.as_bytes().len() as u8,
        ]);
        sub.extend_from_slice(r0.context().r0kh.as_bytes());
        let mut response = domain().encode().to_vec();
        let mut ft = [0; 257];
        let len = wire::FastTransitionFields {
            element_count: 0,
            rsnxe_used: false,
            mic: [0; 16],
            anonce: [0; 32],
            snonce: [0; 32],
            subelements: Elements::parse(&sub).unwrap(),
        }
        .encode(&mut ft)
        .unwrap();
        response.extend_from_slice(&ft[..len]);
        let source = || match akm {
            FtAkm::Psk => InitialKey::psk(Pmk::from_bytes(core::array::from_fn(|i| i as u8))),
            FtAkm::Sae => InitialKey::authenticated_sae(
                Pmk::from_bytes(core::array::from_fn(|i| i as u8)),
                oer_ieee80211_mac::security::SaePwe::HuntingAndPecking,
            ),
            FtAkm::Ieee8021X => {
                InitialKey::authenticated_eap_msk(&core::array::from_fn(|i| i as u8))
            }
        };
        let initial_sta =
            InitialAssociation::new(source(), target, STA, &response, security).unwrap();
        let initial_ap =
            InitialAssociation::new(source(), target, STA, &response, security).unwrap();
        let mut sta = RsnStaState::new(akm, STA, AP, [0x33; 32]).unwrap();
        let mut ap = RsnApState::new(akm, AP, STA, [0x44; 32], 10).unwrap();
        let m1 = RsnTxFrame::<512>::message1(akm, STA, 10, [0x44; 32]).unwrap();
        let (ticket, context) = match sta
            .on_frame(
                OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, AP, m1.as_bytes()).unwrap(),
            )
            .unwrap()
        {
            RsnStaAction::DerivePtk { ticket, context } => (ticket, context),
            other => panic!("{other:?}"),
        };
        let sta_ptk = initial_sta.derive_ptk(context).unwrap();
        sta.complete_ptk::<512>(ticket, true).unwrap();
        let m2 = initial_sta.message2::<512>(10, &sta_ptk).unwrap();
        let (ticket, context, retained) = match ap
            .on_frame(
                OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, STA, m2.as_bytes())
                    .unwrap(),
            )
            .unwrap()
        {
            RsnApAction::DerivePtk {
                ticket,
                context,
                message2,
            } => (ticket, context, message2),
            other => panic!("{other:?}"),
        };
        let ap_ptk = initial_ap.derive_ptk(context).unwrap();
        assert!(retained.key_frame().verify_ft_mic(&ap_ptk));
        initial_ap
            .validate_message2(retained.key_frame().key_data())
            .unwrap();
        let (ticket, retained) = match ap.complete_ptk(ticket, retained, true).unwrap() {
            RsnApAction::VerifyMessage2Mic { ticket, message2 } => (ticket, message2),
            other => panic!("{other:?}"),
        };
        let ticket = match ap.complete_message2_mic(ticket, retained, true).unwrap() {
            RsnApAction::PrepareMessage3 { ticket } => ticket,
            other => panic!("{other:?}"),
        };
        let plain = initial_ap
            .message3_key_data::<256>(&groups(management), 1000, 3600)
            .unwrap();
        let wrapped = crate::aes::software_aes128_key_wrap(ap_ptk.kek(), plain.as_bytes()).unwrap();
        ap.complete_message3_preparation::<512>(ticket, true)
            .unwrap();
        let m3 = RsnTxFrame::<512>::message3(akm, STA, 11, [0x44; 32], [7; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate_ft(&ap_ptk)
            .unwrap();
        let m3owned =
            OwnedEapolFrame::<512>::try_copy(RsnInterface::Station, AP, m3.as_bytes()).unwrap();
        let (ticket, retained) = match sta.on_frame(m3owned.clone()).unwrap() {
            RsnStaAction::VerifyMessage3Mic { ticket, frame } => (ticket, frame),
            other => panic!("{other:?}"),
        };
        assert!(retained.key_frame().verify_ft_mic(&sta_ptk));
        let (ticket, retained) = match sta.complete_message3_mic(ticket, retained, true).unwrap() {
            RsnStaAction::DecryptMessage3KeyData { ticket, frame } => (ticket, frame),
            other => panic!("{other:?}"),
        };
        let unwrapped =
            crate::aes::software_aes128_key_unwrap(sta_ptk.kek(), retained.key_frame().key_data())
                .unwrap();
        let keys = initial_sta
            .parse_message3_key_data(unwrapped.as_bytes())
            .unwrap();
        assert_eq!(keys.groups.gtk.key(), &[0x66; 16]);
        assert_eq!(keys.key_lifetime_seconds, 3600);
        assert_eq!(keys.reassociation_deadline_tu, 1000);
        let ticket = match sta.complete_key_data(ticket, retained, true).unwrap() {
            RsnStaAction::InstallKeys { ticket, .. } => ticket,
            other => panic!("{other:?}"),
        };
        sta.complete_key_install::<512>(ticket, true).unwrap();
        let m4 = RsnTxFrame::<512>::message4(akm, AP, 11)
            .unwrap()
            .authenticate_ft(&sta_ptk)
            .unwrap();
        let (ticket, retained) = match ap
            .on_frame(
                OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, STA, m4.as_bytes())
                    .unwrap(),
            )
            .unwrap()
        {
            RsnApAction::VerifyMessage4Mic { ticket, message4 } => (ticket, message4),
            other => panic!("{other:?}"),
        };
        assert!(retained.key_frame().verify_ft_mic(&ap_ptk));
        assert_eq!(
            ap.complete_message4_mic(ticket, retained, true).unwrap(),
            RsnApAction::AuthorizePeer
        );
        assert!(matches!(
            sta.on_frame(m3owned).unwrap(),
            RsnStaAction::Transmit(RsnTransmit {
                retransmission: true,
                ..
            })
        ));
        assert_eq!(
            initial_sta.into_station_root(&sta).unwrap().name(),
            r0.name()
        );
        assert_eq!(
            initial_ap.into_access_point_root(&ap).unwrap().name(),
            r0.name()
        );
        let mut changed = plain.as_bytes().to_vec();
        let index = changed
            .windows(ft[..len].len())
            .position(|v| v == &ft[..len])
            .unwrap();
        changed[index + wire::FT_ANONCE_OFFSET] ^= 1;
        // Association FTIE is authenticated byte-for-byte, not merely parsed.
        let initial = InitialAssociation::new(source(), target, STA, &response, security).unwrap();
        assert!(matches!(
            initial.parse_message3_key_data(&changed),
            Err(Error::Frame(
                crate::frames::RsnFrameError::FtBindingMismatch
            ))
        ));
        assert!(matches!(
            initial.into_station_root(&RsnStaState::new(akm, STA, AP, [1; 32]).unwrap()),
            Err(Error::WrongPhase)
        ));
    }
}

#[test]
fn bad_mic_and_wrong_peer_do_not_admit_keys_or_poison_valid_reassociation() {
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &[], None);
    sta.start(target, Transport::OverAir, [0x33; 32], &[], 0)
        .unwrap();
    authenticate(&mut sta, &mut ap, &r0, Transport::OverAir);
    let tx = sta.transmission().unwrap();
    let id = tx.id;
    let request = tx.bytes.to_vec();
    sta.admitted(id, 6).unwrap();
    let ft = wire::FtElements::parse(&request).unwrap().ft.encoded();
    let offset = request.windows(ft.len()).position(|v| v == ft).unwrap();
    let mut bad = request.clone();
    bad[offset + wire::FT_MIC_OFFSET] ^= 1;
    assert_eq!(
        ap.reassociation_request(STA, &bad, 7),
        Err(Error::InvalidMic)
    );
    assert!(ap.commit().is_none());
    assert_eq!(ap.phase(), AccessPointPhase::Authenticating);
    assert_eq!(
        ap.reassociation_request(OLD, &request, 7),
        Err(Error::WrongPeer)
    );
    ap.reassociation_request(STA, &request, 7).unwrap();
    let (id, _) = ap.resources().unwrap();
    ap.prepare_response(id, &[], &groups(false), 8).unwrap();
    let id = ap.commit().unwrap().id;
    ap.commit_admitted(id, 9).unwrap();
    ap.commit_completed(id, CommitOutcome::Committed, 10)
        .unwrap();
    let response = ap.transmission().unwrap().bytes.to_vec();
    let mut bad = response.clone();
    let ft = wire::FtElements::parse(&response).unwrap().ft.encoded();
    let offset = response.windows(ft.len()).position(|v| v == ft).unwrap();
    bad[offset + wire::FT_MIC_OFFSET] ^= 1;
    assert_eq!(
        sta.reassociation_response(AP, 0, &bad, 11),
        Err(Error::InvalidMic)
    );
    assert!(sta.commit().is_none());
    assert_eq!(sta.phase(), StationPhase::Reassociating);
    assert_eq!(
        sta.reassociation_response(AP, 0, &response, 12).unwrap(),
        StationEvent::CommitReady
    );
}

#[test]
fn cancellation_and_late_commit_require_explicit_rollback_and_reject_stale_completion() {
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &[], None);
    sta.start(target, Transport::OverAir, [0x33; 32], &[], 0)
        .unwrap();
    authenticate(&mut sta, &mut ap, &r0, Transport::OverAir);
    let response = ready_response(&mut sta, &mut ap, false);
    sta.reassociation_response(AP, 0, &response, 12).unwrap();
    let id = sta.commit().unwrap().id;
    sta.commit_admitted(id, 13).unwrap();
    assert_eq!(
        sta.cancel(),
        StationEvent::RollbackRequired(Rollback { id, target: AP })
    );
    assert!(sta.take_completed_keys().is_none());
    assert_eq!(
        sta.commit_completed(id, CommitOutcome::Committed, 14),
        Err(Error::StaleOperation)
    );
    assert_eq!(
        sta.start(target, Transport::OverAir, [0x55; 32], &[], 14),
        Err(Error::Busy)
    );
    sta.rollback_completed(id).unwrap();
    assert_eq!(sta.phase(), StationPhase::Failed);
    sta.start(target, Transport::OverAir, [0x55; 32], &[], 14)
        .unwrap();
    assert_eq!(sta.rollback_completed(id), Err(Error::StaleOperation));
    let ap_id = ap.rollback().map(|v| v.id);
    assert!(ap_id.is_none());
    assert!(matches!(
        ap.poll(100_006).unwrap(),
        AccessPointEvent::RollbackRequired(_)
    ));
    let id = ap.rollback().unwrap().id;
    ap.rollback_completed(id).unwrap();
    assert_eq!(ap.phase(), AccessPointPhase::Failed);
}

#[test]
fn retries_are_bounded_preserve_last_response_window_and_ignore_old_tx_completions() {
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, _, target) = setup(&r0, &rsn, &[], None);
    sta.start(target, Transport::OverAir, [0x33; 32], &[], 0)
        .unwrap();
    let original = sta.transmission().unwrap().id;
    sta.admitted(original, 0).unwrap();
    sta.tx_completed(original, TxOutcome::Failed, 1).unwrap();
    sta.tx_completed(original, TxOutcome::Acknowledged, 2)
        .unwrap(); // duplicated completion cannot cancel retry
    sta.poll(1000).unwrap();
    let retry = sta.transmission().unwrap();
    let id = retry.id;
    assert!(retry.retransmission);
    assert_ne!(id, original);
    sta.tx_completed(original, TxOutcome::Acknowledged, 1000)
        .unwrap();
    sta.admitted(id, 1000).unwrap();
    sta.tx_completed(id, TxOutcome::Failed, 1001).unwrap();
    sta.poll(2000).unwrap();
    let id = sta.transmission().unwrap().id;
    sta.admitted(id, 2000).unwrap();
    assert_eq!(sta.next_deadline_us(), Some(100_000));
    assert_eq!(sta.poll(99_999).unwrap(), StationEvent::None);
    assert!(sta.transmission().is_none());
    assert_eq!(sta.poll(100_000).unwrap(), StationEvent::Failed);
    assert_eq!(sta.poll(99_999), Err(Error::TimeWentBackwards));
}

#[test]
fn root_and_r1_caches_enforce_scope_authorization_capacity_and_expiration() {
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &[], None);
    sta.start(target, Transport::OverAir, [0x33; 32], &[], 0)
        .unwrap();
    let body = sta.transmission().unwrap().bytes.to_vec();
    ap.authentication_request(STA, &body, [0x44; 32], 0)
        .unwrap();
    let lookup = ap.key_lookup().unwrap();
    assert_eq!(
        AuthorizedKeyRequest::from_authenticated_peer(lookup, OLD, R1),
        Err(Error::WrongPeer)
    );
    let authorized = AuthorizedKeyRequest::from_authenticated_peer(lookup, AP, R1).unwrap();
    let mut root_cache = RootKeyHolder::<1>::new();
    root_cache.insert(r0.duplicate(), 10_000, 0).unwrap();
    assert_eq!(
        root_cache.insert(root(FtAkm::Ieee8021X), 10_000, 0),
        Err(Error::CapacityExceeded)
    );
    let delivery =
        KeyDelivery::from_authenticated_grant(root_cache.deliver(authorized, 1).unwrap(), 0, 1)
            .unwrap();
    let mut local = R1KeyHolder::<1>::new();
    local
        .insert_authenticated(delivery.key, delivery.expires_at_us, 1)
        .unwrap();
    let mut wrong = lookup;
    wrong.root.station = OLD;
    assert!(matches!(
        local.lookup(wrong, 2),
        Err(Error::WrongKeyContext)
    ));
    let mut delivery = local.lookup(lookup, 2).unwrap();
    delivery.target = OLD;
    assert_eq!(ap.key_delivered(delivery, 2), Err(Error::WrongPeer));
    ap.key_delivered(local.lookup(lookup, 2).unwrap(), 2)
        .unwrap();
    assert!(ap.key_lookup().is_none());
    assert!(matches!(
        local.lookup(lookup, 10_000),
        Err(Error::WrongKeyContext)
    ));
    assert!(matches!(
        root_cache.deliver(authorized, 10_000),
        Err(Error::WrongKeyContext)
    ));
    assert_eq!(local.expire(9999), Err(Error::TimeWentBackwards));
    root_cache
        .insert(root(FtAkm::Ieee8021X), 20_000, 10_000)
        .unwrap();
    root_cache.remove_station(STA);
    assert!(matches!(
        root_cache.deliver(authorized, 10_001),
        Err(Error::WrongKeyContext)
    ));
}

#[test]
fn missing_key_refusal_is_delivered_for_air_and_ds_and_never_derives_a_ptk() {
    for transport in [Transport::OverAir, Transport::OverDs] {
        let r0 = root(FtAkm::Psk);
        let rsn = advertised(FtAkm::Psk, false);
        let (mut sta, mut ap, target) = setup(&r0, &rsn, &[], None);
        sta.start(target, transport, [0x33; 32], &[], 0).unwrap();
        let tx = sta.transmission().unwrap();
        let id = tx.id;
        let body = tx.bytes.to_vec();
        sta.admitted(id, 0).unwrap();
        if transport == Transport::OverAir {
            ap.authentication_request(STA, &body, [0x44; 32], 1)
                .unwrap();
        } else {
            ap.authenticated_ds_request(OLD, &body, [0x44; 32], 1)
                .unwrap();
        }
        let lookup = ap.key_lookup().unwrap();
        assert_eq!(
            ap.key_unavailable(lookup.id, 0, 2),
            Err(Error::InvalidConfiguration)
        );
        ap.key_unavailable(lookup.id, 53, 2).unwrap();
        let tx = ap.transmission().unwrap();
        let id = tx.id;
        let body = tx.bytes.to_vec();
        ap.admitted(id, 3).unwrap();
        assert_eq!(
            sta.authentication_response(
                if transport == Transport::OverAir {
                    AP
                } else {
                    OLD
                },
                &body,
                4
            ),
            Err(Error::PeerRejected(53))
        );
        assert!(ap.commit().is_none());
        assert!(sta.commit().is_none());
        assert_eq!(
            ap.tx_completed(id, TxOutcome::Acknowledged, 4).unwrap(),
            AccessPointEvent::Failed
        );
    }
}

#[test]
fn ds_relay_binds_authorized_target_operation_and_nonce_without_owning_keys() {
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &[], None);
    let mut relay = DsRelay::<512>::new(identity(), domain().id, TIMEOUTS, 0).unwrap();
    sta.start(target, Transport::OverDs, [0x33; 32], &[], 0)
        .unwrap();
    let tx = sta.transmission().unwrap();
    let id = tx.id;
    let body = tx.bytes.to_vec();
    sta.admitted(id, 0).unwrap();
    assert_eq!(relay.request(STA, &body, OLD, 0), Err(Error::WrongPeer));
    relay.request(STA, &body, AP, 0).unwrap();
    relay.request(STA, &body, AP, 0).unwrap();
    let forward = relay.forward().unwrap();
    let id = forward.id;
    ap.authenticated_ds_request(OLD, forward.action_body, [0x44; 32], 1)
        .unwrap();
    relay.forward_admitted(id, 1).unwrap();
    assert!(relay.forward().is_none());
    let lookup = ap.key_lookup().unwrap();
    ap.key_delivered(
        KeyDelivery {
            request: lookup.id,
            target: AP,
            key: r0.derive_r1(R1),
            expires_at_us: 1_000_000,
        },
        2,
    )
    .unwrap();
    let response = ap.transmission().unwrap().bytes.to_vec();
    assert_eq!(
        relay.authenticated_response(id, OLD, &response, 3),
        Err(Error::WrongPeer)
    );
    let mut bad = response.clone();
    let action = wire::Action::parse(&response).unwrap();
    let ft = wire::FtElements::parse(action.elements.as_bytes())
        .unwrap()
        .ft
        .encoded();
    let offset = response.windows(ft.len()).position(|v| v == ft).unwrap();
    bad[offset + wire::FT_SNONCE_OFFSET] ^= 1;
    assert_eq!(
        relay.authenticated_response(id, AP, &bad, 3),
        Err(Error::WrongKeyContext)
    );
    relay.authenticated_response(id, AP, &response, 3).unwrap();
    let tx = relay.transmission().unwrap();
    let tx_id = tx.id;
    let response = tx.bytes.to_vec();
    relay.admitted(tx_id, 4).unwrap();
    sta.authentication_response(OLD, &response, 5).unwrap();
    assert_eq!(
        relay
            .tx_completed(tx_id, TxOutcome::Acknowledged, 5)
            .unwrap(),
        RelayEvent::Delivered
    );
    assert!(relay.next_deadline_us().is_none());
    assert_eq!(
        relay.authenticated_response(id, AP, &response, 6),
        Err(Error::StaleOperation)
    );
}

#[test]
fn h2e_is_explicit_and_rsnxe_used_is_not_inferred_from_advertisement_presence() {
    use oer_ieee80211_mac::security::SaePwe;
    let r0 = InitialKey::authenticated_sae(
        Pmk::from_bytes(core::array::from_fn(|i| i as u8)),
        SaePwe::HashToElement,
    )
    .derive_r0(root(FtAkm::Sae).context());
    let rsn = advertised(FtAkm::Sae, true);
    let xe = oer_ieee80211_mac::security::AP_SAE_H2E_RSNX_ELEMENT;
    assert!(matches!(
        SecurityProfile::new(&rsn, &[], FtAkm::Sae, true, Some(SaePwe::HashToElement)),
        Err(Error::UnsupportedSecurity)
    ));
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &xe, Some(SaePwe::HashToElement));
    sta.start(target, Transport::OverAir, [0x33; 32], &[], 0)
        .unwrap();
    authenticate(&mut sta, &mut ap, &r0, Transport::OverAir);
    let req = wire::FtElements::parse(sta.transmission().unwrap().bytes).unwrap();
    assert!(req.ft.rsnxe_used());
    assert_eq!(req.ft.element_count(), 4);
    let response = ready_response(&mut sta, &mut ap, true);
    assert!(wire::FtElements::parse(&response).unwrap().ft.rsnxe_used());
    sta.reassociation_response(AP, 0, &response, 12).unwrap();
    let r0 = root(FtAkm::Sae);
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &xe, Some(SaePwe::HuntingAndPecking));
    sta.start(target, Transport::OverAir, [0x33; 32], &[], 0)
        .unwrap();
    authenticate(&mut sta, &mut ap, &r0, Transport::OverAir);
    let req = wire::FtElements::parse(sta.transmission().unwrap().bytes).unwrap();
    assert!(!req.ft.rsnxe_used());
    assert_eq!(req.ft.element_count(), 4);
}

#[test]
fn resource_contract_rejects_missing_extra_failed_or_malformed_answers() {
    use super::station::validate_resource_response;
    let request = [wire::RIC_DATA_ELEMENT_ID, 4, 1, 1, 0, 0, 75, 1, 0];
    let accepted = [wire::RIC_DATA_ELEMENT_ID, 4, 1, 0, 0, 0];
    validate_resource_response(&request, &accepted).unwrap();
    assert!(validate_resource_response(&request, &[]).is_err());
    assert!(validate_resource_response(&[], &accepted).is_err());
    assert!(validate_resource_response(&[], &[75, 1, 0]).is_err());
    let denied = [wire::RIC_DATA_ELEMENT_ID, 4, 1, 0, 37, 0];
    assert_eq!(
        validate_resource_response(&request, &denied),
        Err(Error::PeerRejected(37))
    );
    let mut short = request.to_vec();
    short.pop();
    assert!(wire::RicElements::parse(&short).is_err());
    let mut duplicate = accepted.to_vec();
    duplicate.extend_from_slice(&accepted);
    assert!(wire::RicElements::parse(&duplicate).is_err());
}

#[test]
fn time_and_operation_overflow_fail_without_wrapping_identity_or_admitting_work() {
    use super::session::Session;
    let mut session = Session::new(identity(), TIMEOUTS, 0).unwrap();
    assert_eq!(
        session.deadline(u64::MAX - 1, 10, u64::MAX),
        Err(Error::DeadlineOverflow)
    );
    assert_eq!(session.deadline(10, 1, 10), Err(Error::KeyExpired));
    session.observe(10).unwrap();
    assert_eq!(session.observe(9), Err(Error::TimeWentBackwards));
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, _, target) = setup(&r0, &rsn, &[], None);
    assert!(
        sta.start(
            Target {
                domain: MobilityDomain {
                    id: MobilityDomainId([8, 9]),
                    ..domain()
                },
                ..target
            },
            Transport::OverAir,
            [1; 32],
            &[],
            0
        )
        .is_err()
    );
    assert_eq!(sta.phase(), StationPhase::Idle);
    assert!(
        sta.start(target, Transport::OverAir, [1; 32], &[75, 0], 0)
            .is_err()
    );
    assert!(sta.transmission().is_none());
}

#[test]
fn inter_ap_grants_translate_lifetime_without_comparing_different_clock_epochs() {
    let r0 = root(FtAkm::Psk);
    let rsn = advertised(FtAkm::Psk, false);
    let (mut sta, mut ap, target) = setup(&r0, &rsn, &[], None);
    sta.start(target, Transport::OverAir, [1; 32], &[], 0)
        .unwrap();
    let body = sta.transmission().unwrap().bytes.to_vec();
    ap.authentication_request(STA, &body, [2; 32], 0).unwrap();
    let lookup = ap.key_lookup().unwrap();
    let authorization = AuthorizedKeyRequest::from_authenticated_peer(lookup, AP, R1).unwrap();
    let mut holder = RootKeyHolder::<1>::new();
    holder.insert(r0.duplicate(), 1_010_000, 1_000_000).unwrap();
    // R0KH's local time exceeds the AP's deadline value; these are different epochs.
    let grant = holder.deliver(authorization, 1_001_000).unwrap();
    assert_eq!(grant.remaining_lifetime_us, 9000);
    let delivery = KeyDelivery::from_authenticated_grant(grant, 100, 10).unwrap();
    assert_eq!(delivery.expires_at_us, 8910);
    ap.key_delivered(delivery, 10).unwrap();
    assert!(matches!(
        KeyDelivery::from_authenticated_grant(
            holder.deliver(authorization, 1_001_000).unwrap(),
            9000,
            10
        ),
        Err(Error::KeyExpired)
    ));
    assert!(matches!(
        KeyDelivery::from_authenticated_grant(
            holder.deliver(authorization, 1_001_000).unwrap(),
            0,
            u64::MAX
        ),
        Err(Error::DeadlineOverflow)
    ));
}
