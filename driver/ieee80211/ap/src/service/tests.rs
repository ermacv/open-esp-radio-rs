const TEST_HT_CAPABILITIES: open_esp_radio_ieee80211::ht::HtLocalCapabilities =
    open_esp_radio_ieee80211::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

use super::*;
use open_esp_radio_ieee80211::{
    beacon::WPA2_PERSONAL_CCMP_PSK_RSN_IE,
    channel::WifiChannel,
    ht::{ht_capability_ie, ht_peer_capabilities},
};
use open_esp_radio_wpa2::{
    EapolKeyMessage, OwnedEapolFrame, PtkContext, Wpa2Interface,
    aes::software_aes128_key_unwrap,
    frames::{OwnedAssociationSecurityIes, OwnedRsnIe, Wpa2Gtk, Wpa2TxFrame, parse_gtk_key_data},
    state::{Wpa2ApAction, Wpa2Ticket},
};

const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const OTHER: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
const WPA2_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const SUPPLICANT_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0x0c, 0,
];

fn association_security<'a>(rsn_ie: &'a [u8]) -> ApAssociationSecurityObservation<'a> {
    association_security_with_rsnxe(rsn_ie, None)
}

fn association_security_with_rsnxe<'a>(
    rsn_ie: &'a [u8],
    rsnxe: Option<&'a [u8]>,
) -> ApAssociationSecurityObservation<'a> {
    ApAssociationSecurityObservation {
        privacy: true,
        rsn_ie: Some(rsn_ie),
        rsn_ie_count: 1,
        rsnxe,
        rsnxe_count: u8::from(rsnxe.is_some()),
        legacy_wpa_present: false,
        malformed_elements: false,
    }
}

fn signed_message2(
    rsn_ie: &[u8],
    rsnxe: &[u8],
    authenticator_nonce: [u8; 32],
    supplicant_nonce: [u8; 32],
) -> OwnedEapolFrame<512> {
    let ptk = Pmk::derive(b"password", b"test-ap")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce,
            supplicant_nonce,
        });
    let rsn_ie = OwnedRsnIe::<64>::try_copy(rsn_ie).unwrap();
    let security_ies = OwnedAssociationSecurityIes::<128>::try_copy(&rsn_ie, rsnxe).unwrap();
    let message2 =
        Wpa2TxFrame::<512>::message2_with_security_ies(AP, 9, supplicant_nonce, &security_ies)
            .unwrap()
            .authenticate(&ptk);
    OwnedEapolFrame::try_copy(Wpa2Interface::AccessPoint, PEER, message2.as_bytes()).unwrap()
}

fn corrupt_mic(frame: OwnedEapolFrame<512>) -> OwnedEapolFrame<512> {
    let mut bytes = [0_u8; 512];
    let length = frame.as_bytes().len();
    bytes[..length].copy_from_slice(frame.as_bytes());
    bytes[81] ^= 1;
    OwnedEapolFrame::try_copy(Wpa2Interface::AccessPoint, PEER, &bytes[..length]).unwrap()
}

fn ht_capabilities() -> ApAssociationCapabilities {
    ApAssociationCapabilities {
        maximum_legacy_rate_500kbps: 108,
        ht: ht_peer_capabilities(&ht_capability_ie(
            TEST_HT_CAPABILITIES,
            WifiChannel::mhz20(6).unwrap(),
        )),
        qos_supported: true,
    }
}

const LEGACY_CAPABILITIES: ApAssociationCapabilities = ApAssociationCapabilities {
    maximum_legacy_rate_500kbps: 108,
    ht: None,
    qos_supported: false,
};

fn service(storage: &mut AccessPointPeerStorage) -> AccessPointService<'_> {
    AccessPointService::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        storage,
    )
}

#[test]
fn runtime_limit_is_enforced_before_hardware_moves() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    assert_eq!(
        service.authenticate_open(PEER, 0),
        ApMlmeAction::AuthenticationResponse {
            peer: PEER,
            status: AP_STATUS_SUCCESS,
        }
    );
    assert_eq!(
        service.authenticate_open(OTHER, 0),
        ApMlmeAction::AuthenticationResponse {
            peer: OTHER,
            status: AP_STATUS_SUCCESS,
        }
    );
    let third = [0x02, 0, 0, 0, 0, 4];
    assert_eq!(
        service.authenticate_open(third, 0),
        ApMlmeAction::AuthenticationResponse {
            peer: third,
            status: AP_STATUS_TOO_MANY_STATIONS,
        }
    );
    assert_eq!(service.associated_count(), 0);
    assert_eq!(service.peers().count(), 2);
    assert_eq!(service.peer_status(PEER).unwrap().association_id, 1);
    assert_eq!(service.peer_status(OTHER).unwrap().association_id, 2);
}

#[test]
fn qos_sequence_spaces_are_independent_for_each_peer_and_tid() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    service.authenticate_open(OTHER, 0);

    assert_eq!(service.next_qos_sequence(PEER, 0), Some(0));
    assert_eq!(service.next_qos_sequence(PEER, 0), Some(1));
    assert_eq!(service.current_qos_sequence(PEER, 0), Some(2));
    assert_eq!(service.current_qos_sequence(OTHER, 0), Some(0));
    assert_eq!(service.next_qos_sequence(OTHER, 0), Some(0));
    assert_eq!(service.current_qos_sequence(PEER, 1), Some(0));
    assert_eq!(service.next_qos_sequence([0xff; 6], 0), None);
}

#[test]
fn peer_binding_rejects_a_reused_slot_generation() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    let first = service.bind_peer(PEER).unwrap();
    assert_eq!(service.bound_peer_status(first).unwrap().address, PEER);

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(OTHER, 1);
    let second = service.bind_peer(OTHER).unwrap();
    assert_eq!(service.bound_peer_status(first), None);
    assert_eq!(service.bound_peer_status(second).unwrap().address, OTHER);
    assert_ne!(first, second);
}

#[test]
fn buffered_downlink_identity_cannot_cross_same_address_reassociation() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new_open(
        AP,
        AccessPointClientLimit::new(1).unwrap(),
        AccessPointInactiveTimeout::default(),
        &mut storage,
    );
    let open_security = ApAssociationSecurityObservation {
        privacy: false,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };

    service.authenticate_open(PEER, 0);
    service
        .associate_open(PEER, open_security, LEGACY_CAPABILITIES, 1)
        .unwrap();
    service
        .observe_power_save(ApPowerSaveObservation::Sleeping { peer: PEER }, 2)
        .unwrap();
    let first = service.admit_downlink(PEER).unwrap();
    assert_eq!(first.disposition(), ApDownlinkDisposition::Buffer);
    service.commit_buffered_unicast(first.identity()).unwrap();
    let release = service
        .begin_buffered_unicast_release(first.identity())
        .unwrap()
        .unwrap();

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(PEER, 3);
    service
        .associate_open(PEER, open_security, LEGACY_CAPABILITIES, 4)
        .unwrap();
    let second = service.admit_downlink(PEER).unwrap();
    assert_eq!(
        first.identity().association_id(),
        second.identity().association_id()
    );
    assert_ne!(
        first.identity().association_epoch(),
        second.identity().association_epoch()
    );
    assert_eq!(
        service.bound_authorized_peer_status(first.identity()),
        None,
        "an old queue owner must not bind to the reused AID and MAC"
    );
    assert_eq!(
        service.complete_buffered_unicast_release(release, false),
        Err(ApServiceError::AssociationIdMismatch),
        "an affine release from the old epoch must fail closed"
    );
    assert_eq!(
        service.commit_buffered_unicast(first.identity()),
        Err(ApServiceError::UnknownPeer),
    );
}

#[test]
fn bound_power_state_matches_general_semantics_and_rejects_slot_reuse() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new_open(
        AP,
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        &mut storage,
    );
    service.authenticate_open(PEER, 0);
    service
        .associate_open(
            PEER,
            ApAssociationSecurityObservation {
                privacy: false,
                rsn_ie: None,
                rsn_ie_count: 0,
                rsnxe: None,
                rsnxe_count: 0,
                legacy_wpa_present: false,
                malformed_elements: false,
            },
            LEGACY_CAPABILITIES,
            1_000,
        )
        .unwrap();
    let binding = service.bind_peer(PEER).unwrap();
    let initial_revision = service.status_revision();

    assert_eq!(
        service
            .observe_bound_power_state(binding, ApPeerPowerState::Active, 2_000)
            .unwrap(),
        ApPowerSaveAction::None,
    );
    assert_eq!(service.status_revision(), initial_revision);
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline_micros,
        10_002_000
    );

    assert_eq!(
        service
            .observe_bound_power_state(binding, ApPeerPowerState::Sleeping, 3_000)
            .unwrap(),
        ApPowerSaveAction::StateChanged {
            peer: PEER,
            state: ApPeerPowerState::Sleeping,
            buffered_frames: 0,
        },
    );
    assert_eq!(service.status_revision(), initial_revision.wrapping_add(1));
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline_micros,
        10_003_000
    );

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(OTHER, 4_000);
    assert_eq!(
        service.observe_bound_power_state(binding, ApPeerPowerState::Active, 5_000),
        Err(ApServiceError::UnknownPeer),
        "a recycled table slot cannot inherit the old peer's PM update"
    );
}

#[test]
fn admitted_data_activity_is_coalesced_but_pm_edges_are_immediate() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new_open(
        AP,
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        &mut storage,
    );
    service.authenticate_open(PEER, 0);
    service
        .associate_open(
            PEER,
            ApAssociationSecurityObservation {
                privacy: false,
                rsn_ie: None,
                rsn_ie_count: 0,
                rsnxe: None,
                rsnxe_count: 0,
                legacy_wpa_present: false,
                malformed_elements: false,
            },
            LEGACY_CAPABILITIES,
            1_000,
        )
        .unwrap();
    let binding = service.bind_peer(PEER).unwrap();
    let associated = service.peer_status(PEER).unwrap();

    assert_eq!(
        service
            .observe_bound_data_power_state(binding, ApPeerPowerState::Active, 2_000)
            .unwrap(),
        ApPowerSaveAction::None,
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline_micros,
        associated.deadline_micros,
        "an unchanged data PM state must not rewrite the peer on every MPDU"
    );

    service
        .observe_bound_data_activity(binding, 5_001_000)
        .unwrap();
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline_micros,
        15_001_000,
        "the half-timeout guard refreshes before expiry"
    );

    let revision = service.status_revision();
    assert_eq!(
        service
            .observe_bound_data_power_state(binding, ApPeerPowerState::Sleeping, 5_002_000)
            .unwrap(),
        ApPowerSaveAction::StateChanged {
            peer: PEER,
            state: ApPeerPowerState::Sleeping,
            buffered_frames: 0,
        },
    );
    assert_eq!(service.status_revision(), revision.wrapping_add(1));
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline_micros,
        15_002_000,
        "a PM transition is never delayed by activity coalescing"
    );

    service.remove_peer(PEER).unwrap();
    service.authenticate_open(OTHER, 6_000_000);
    assert_eq!(
        service.observe_bound_data_activity(binding, 6_001_000),
        Err(ApServiceError::UnknownPeer),
        "coalescing must not weaken the slot-generation fence"
    );
}

#[test]
fn inactivity_timeout_is_bounded_and_defaults_to_vendor_policy() {
    assert_eq!(AccessPointInactiveTimeout::default().seconds(), 300);
    assert_eq!(AccessPointInactiveTimeout::new(9).unwrap_err().seconds(), 9);
    assert_eq!(AccessPointInactiveTimeout::new(10).unwrap().seconds(), 10);
    assert_eq!(
        AccessPointInactiveTimeout::new(3_600).unwrap().seconds(),
        3_600
    );
    assert_eq!(
        AccessPointInactiveTimeout::new(3_601)
            .unwrap_err()
            .seconds(),
        3_601
    );
}

#[test]
fn authenticated_peer_expires_at_the_recovered_fifteen_second_frontier() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 1_000);
    assert_eq!(service.next_peer_deadline(), Some(15_001_000));
    assert_eq!(service.begin_due_peer_close(15_000_999), None);
    assert_eq!(
        service.begin_due_peer_close(15_001_000),
        Some(ApPeerClose {
            peer: PEER,
            kind: ApPeerCloseKind::AuthenticationTimeout,
            was_associated: false,
            maximum_legacy_rate_500kbps: 2,
        })
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Closing
    );
    assert_eq!(service.associated_count(), 0);
}

#[test]
fn associated_activity_refreshes_the_configured_inactivity_frontier() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        &mut storage,
    );
    service.authenticate_open(PEER, 0);
    service
        .associate_wpa2(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            2_000,
        )
        .unwrap();
    assert_eq!(service.next_peer_deadline(), Some(10_002_000));
    let binding = service.bind_peer(PEER).expect("associated peer binding");
    service.observe_bound_activity(binding, 5_000_000).unwrap();
    assert_eq!(service.next_peer_deadline(), Some(15_000_000));
    assert_eq!(service.begin_due_peer_close(14_999_999), None);
    assert_eq!(
        service.begin_due_peer_close(15_000_000),
        Some(ApPeerClose {
            peer: PEER,
            kind: ApPeerCloseKind::InactivityTimeout,
            was_associated: true,
            maximum_legacy_rate_500kbps: 108,
        })
    );
    assert_eq!(service.associated_count(), 0);
}

#[test]
fn association_owns_a_bounded_wpa2_state() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    assert_eq!(
        service
            .associate_wpa2(
                PEER,
                association_security(&WPA2_RSN),
                ht_capabilities(),
                [7; 32],
                9,
                1,
            )
            .unwrap(),
        ApMlmeAction::AssociationResponse {
            peer: PEER,
            status: AP_STATUS_SUCCESS,
            association_id: Some(1),
        }
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Securing
    );
    assert_eq!(
        service
            .peer_status(PEER)
            .unwrap()
            .maximum_legacy_rate_500kbps,
        108
    );
    assert_eq!(service.associated_count(), 1);
    assert_eq!(
        service.begin_wpa2(PEER).unwrap(),
        ApMlmeAction::BeginWpa2 { peer: PEER }
    );
    assert!(matches!(
        service.wpa2_mut(PEER).unwrap().message1(false).unwrap(),
        Wpa2ApAction::Transmit(_)
    ));
    let _ticket_type_is_owned: Option<Wpa2Ticket> = None;
}

#[test]
fn association_rejects_a_peer_without_a_common_legacy_rate() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    assert_eq!(
        service
            .associate_wpa2(
                PEER,
                association_security(&WPA2_RSN),
                ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps: 0,
                    ht: None,
                    qos_supported: false,
                },
                [7; 32],
                9,
                1,
            )
            .unwrap(),
        ApMlmeAction::AssociationResponse {
            peer: PEER,
            status: AP_STATUS_UNSUPPORTED_RATES,
            association_id: None,
        }
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Authenticated
    );
}

#[test]
fn complete_four_way_handshake_retains_ptk_until_hardware_authorization() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];

    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    // Supplicants may add their own RSN capabilities. Message 3 must not
    // reflect those bytes back: it authenticates the AP's beacon RSN IE.
    service
        .associate_wpa2(
            PEER,
            association_security(&SUPPLICANT_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            1,
        )
        .unwrap();
    let message1 = service.begin_wpa2_frame::<512>(PEER).unwrap();
    assert_eq!(
        message1.key_frame().message(),
        EapolKeyMessage::PairwiseMessage1
    );
    assert!(!message1.retransmission());
    service
        .observe_wpa2_transmit(PEER, false, true, 10)
        .unwrap();
    assert_eq!(service.next_wpa2_retry_deadline(), Some(1_000_010));
    let ApWpa2RetryProgress::Transmit {
        peer: retried_peer,
        frame: retried_message1,
    } = service.take_due_wpa2_retry::<512>(1_000_010).unwrap()
    else {
        panic!("Message 1 response timeout must retransmit")
    };
    assert_eq!(retried_peer, PEER);
    assert!(retried_message1.retransmission());
    assert_eq!(retried_message1.as_bytes(), message1.as_bytes());

    let ptk = Pmk::derive(b"password", b"test-ap")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        });
    let rsn = OwnedRsnIe::<64>::try_copy(&SUPPLICANT_RSN).unwrap();
    let message2 = Wpa2TxFrame::<512>::message2(AP, 9, SNONCE, &rsn)
        .unwrap()
        .authenticate(&ptk);
    let message2 =
        OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, PEER, message2.as_bytes())
            .unwrap();
    let ApWpa2Progress::Transmit(message3) = service.on_eapol(PEER, message2).unwrap() else {
        panic!("message 2 must produce message 3");
    };
    assert_eq!(
        message3.key_frame().message(),
        EapolKeyMessage::PairwiseMessage3
    );
    assert!(message3.key_frame().verify_mic(&ptk));
    let plaintext = software_aes128_key_unwrap(ptk.kek(), message3.key_frame().key_data())
        .expect("AP wrapped its Message 3 key data");
    assert!(parse_gtk_key_data(plaintext.as_bytes(), &WPA2_PERSONAL_CCMP_PSK_RSN_IE, &[],).is_ok());
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    service
        .observe_wpa2_transmit(PEER, false, true, 2_000_000)
        .unwrap();
    assert_eq!(service.next_wpa2_retry_deadline(), Some(2_100_000));
    let ApWpa2RetryProgress::Transmit {
        peer: retried_peer,
        frame: retried_message3,
    } = service.take_due_wpa2_retry::<512>(2_100_000).unwrap()
    else {
        panic!("Message 3 response timeout must retransmit")
    };
    assert_eq!(retried_peer, PEER);
    assert!(retried_message3.retransmission());
    assert_eq!(retried_message3.as_bytes(), message3.as_bytes());
    assert!(matches!(
        parse_gtk_key_data(plaintext.as_bytes(), &SUPPLICANT_RSN, &[]),
        Err(Wpa2FrameError::RsnIeMismatch)
    ));

    let message4 = Wpa2TxFrame::<512>::message4(AP, 10)
        .unwrap()
        .authenticate(&ptk);
    let message4 =
        OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, PEER, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, message4).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
    assert_eq!(service.next_wpa2_retry_deadline(), None);
    assert!(service.pending_ptk(PEER).is_ok());
    service.authorize(PEER, 2).unwrap();
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Authorized
    );
    assert_eq!(
        service.pending_ptk(PEER).err(),
        Some(ApServiceError::WrongPeerPhase)
    );
}

#[test]
fn message2_must_echo_the_exact_association_rsn() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];

    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    service
        .associate_wpa2(
            PEER,
            association_security(&SUPPLICANT_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            1,
        )
        .unwrap();

    assert!(matches!(
        service
            .on_eapol(PEER, signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE))
            .unwrap(),
        ApWpa2Progress::DeauthenticatePeer
    ));
    assert_eq!(service.wpa2_mut(PEER).unwrap().phase(), Wpa2ApPhase::Failed);
    assert!(service.pending_ptk(PEER).is_err());
}

#[test]
fn unauthenticated_eapol_cannot_poison_or_refresh_a_securing_peer() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];

    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    service
        .associate_wpa2(
            PEER,
            association_security(&SUPPLICANT_RSN),
            ht_capabilities(),
            ANONCE,
            9,
            1,
        )
        .unwrap();
    let original_deadline = service.peer_status(PEER).unwrap().deadline_micros;

    let replay_mismatch = Wpa2TxFrame::<512>::message4(AP, 77).unwrap();
    let replay_mismatch = OwnedEapolFrame::<512>::try_copy(
        Wpa2Interface::AccessPoint,
        PEER,
        replay_mismatch.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, replay_mismatch).unwrap(),
        ApWpa2Progress::None
    ));

    let unsupported = Wpa2TxFrame::<512>::message1(AP, 9, ANONCE).unwrap();
    let unsupported =
        OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, PEER, unsupported.as_bytes())
            .unwrap();
    assert!(matches!(
        service.on_eapol(PEER, unsupported).unwrap(),
        ApWpa2Progress::None
    ));

    // The attacker also supplies mismatched association Key Data. The
    // mismatch is not actionable because this candidate's MIC is bad.
    let forged_m2 = corrupt_mic(signed_message2(&WPA2_RSN, &[], ANONCE, SNONCE));
    assert!(matches!(
        service.on_eapol(PEER, forged_m2).unwrap(),
        ApWpa2Progress::None
    ));
    assert_eq!(
        service.wpa2_mut(PEER).unwrap().phase(),
        Wpa2ApPhase::AwaitingMessage2
    );
    assert!(service.pending_ptk(PEER).is_err());
    assert_eq!(
        service.peer_status(PEER).unwrap().deadline_micros,
        original_deadline,
        "ignored EAPOL must not extend peer liveness"
    );

    let valid_m2 = signed_message2(&SUPPLICANT_RSN, &[], ANONCE, SNONCE);
    assert!(matches!(
        service.on_eapol(PEER, valid_m2).unwrap(),
        ApWpa2Progress::Transmit(_)
    ));
    assert_eq!(
        service.wpa2_mut(PEER).unwrap().phase(),
        Wpa2ApPhase::AwaitingMessage4
    );

    // Neither forged nor even MIC-valid duplicate M2 directly elicits a
    // fresh M3. The finite authenticator retry timer owns retransmission.
    let duplicate_m2 = signed_message2(&SUPPLICANT_RSN, &[], ANONCE, SNONCE);
    assert!(matches!(
        service.on_eapol(PEER, duplicate_m2).unwrap(),
        ApWpa2Progress::None
    ));

    let ptk = Pmk::derive(b"password", b"test-ap")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: AP,
            supplicant_address: PEER,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        });
    let valid_m4 = Wpa2TxFrame::<512>::message4(AP, 10)
        .unwrap()
        .authenticate(&ptk);
    let valid_m4 =
        OwnedEapolFrame::try_copy(Wpa2Interface::AccessPoint, PEER, valid_m4.as_bytes()).unwrap();
    let forged_m4 = corrupt_mic(valid_m4.clone());
    assert!(matches!(
        service.on_eapol(PEER, forged_m4).unwrap(),
        ApWpa2Progress::None
    ));
    assert_eq!(
        service.wpa2_mut(PEER).unwrap().phase(),
        Wpa2ApPhase::AwaitingMessage4
    );
    assert!(matches!(
        service.on_eapol(PEER, valid_m4).unwrap(),
        ApWpa2Progress::AuthorizePeer
    ));
}

#[test]
fn message2_must_echo_the_exact_association_rsnxe() {
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];
    const RSNXE: [u8; 3] = [0xf4, 1, 0x20];

    let mut rejected_storage = AccessPointPeerStorage::new();
    let mut rejected = service(&mut rejected_storage);
    rejected.authenticate_open(PEER, 0);
    rejected
        .associate_wpa2(
            PEER,
            association_security_with_rsnxe(&SUPPLICANT_RSN, Some(&RSNXE)),
            ht_capabilities(),
            ANONCE,
            9,
            1,
        )
        .unwrap();
    assert!(matches!(
        rejected
            .on_eapol(PEER, signed_message2(&SUPPLICANT_RSN, &[], ANONCE, SNONCE),)
            .unwrap(),
        ApWpa2Progress::DeauthenticatePeer
    ));

    let mut accepted_storage = AccessPointPeerStorage::new();
    let mut accepted = service(&mut accepted_storage);
    accepted.authenticate_open(PEER, 0);
    accepted
        .associate_wpa2(
            PEER,
            association_security_with_rsnxe(&SUPPLICANT_RSN, Some(&RSNXE)),
            ht_capabilities(),
            ANONCE,
            9,
            1,
        )
        .unwrap();
    assert!(matches!(
        accepted
            .on_eapol(
                PEER,
                signed_message2(&SUPPLICANT_RSN, &RSNXE, ANONCE, SNONCE),
            )
            .unwrap(),
        ApWpa2Progress::Transmit(_)
    ));
}

#[test]
fn exhausted_pairwise_update_count_closes_the_peer() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    service
        .associate_wpa2(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            1,
        )
        .unwrap();
    service.begin_wpa2_frame::<512>(PEER).unwrap();
    service.observe_wpa2_transmit(PEER, false, true, 0).unwrap();

    for deadline in [1_000_000, 2_000_000, 3_000_000] {
        assert!(matches!(
            service.take_due_wpa2_retry::<512>(deadline).unwrap(),
            ApWpa2RetryProgress::Transmit { peer: PEER, .. }
        ));
    }
    assert!(matches!(
        service.take_due_wpa2_retry::<512>(4_000_000).unwrap(),
        ApWpa2RetryProgress::Close(ApPeerClose {
            peer: PEER,
            kind: ApPeerCloseKind::Wpa2HandshakeTimeout,
            was_associated: true,
            ..
        })
    ));
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Closing
    );
}

#[test]
fn invalid_rsn_does_not_open_the_controlled_port() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 0);
    assert_eq!(
        service.associate_wpa2(
            PEER,
            association_security(&[0x30, 0]),
            LEGACY_CAPABILITIES,
            [7; 32],
            9,
            1,
        ),
        Ok(ApMlmeAction::AssociationResponse {
            peer: PEER,
            status: AP_STATUS_INVALID_RSN,
            association_id: None,
        })
    );
    assert_eq!(
        service.peer_status(PEER).unwrap().phase,
        ApPeerPhase::Authenticated
    );
}

#[test]
fn management_sequence_wraps_at_twelve_bits() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    for expected in 0..=0x0fff {
        assert_eq!(service.next_management_sequence(), expected);
    }
    assert_eq!(service.next_management_sequence(), 0);
}

#[test]
fn all_fifteen_aids_are_stable_and_reused_after_removal() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new(
        AP,
        Pmk::derive(b"password", b"test-ap").unwrap(),
        Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(15).unwrap(),
        AccessPointInactiveTimeout::default(),
        &mut storage,
    );
    for suffix in 1..=15_u8 {
        let peer = [0x02, 0, 0, 0, 1, suffix];
        assert_eq!(
            service.authenticate_open(peer, 0),
            ApMlmeAction::AuthenticationResponse {
                peer,
                status: AP_STATUS_SUCCESS,
            }
        );
        assert_eq!(
            service.peer_status(peer).unwrap().association_id,
            u16::from(suffix)
        );
        assert!(matches!(
            service
                .associate_wpa2(
                    peer,
                    association_security(&WPA2_RSN),
                    ht_capabilities(),
                    [suffix; 32],
                    u64::from(suffix),
                    1,
                )
                .unwrap(),
            ApMlmeAction::AssociationResponse {
                status: AP_STATUS_SUCCESS,
                association_id: Some(_),
                ..
            }
        ));
    }
    assert_eq!(service.associated_count(), 15);
    let overflow = [0x02, 0, 0, 0, 2, 1];
    assert_eq!(
        service.authenticate_open(overflow, 0),
        ApMlmeAction::AuthenticationResponse {
            peer: overflow,
            status: AP_STATUS_TOO_MANY_STATIONS,
        }
    );
    let released = [0x02, 0, 0, 0, 1, 7];
    service.remove_peer(released).unwrap();
    assert_eq!(
        service.authenticate_open(overflow, 0),
        ApMlmeAction::AuthenticationResponse {
            peer: overflow,
            status: AP_STATUS_SUCCESS,
        }
    );
    assert_eq!(service.peer_status(overflow).unwrap().association_id, 7);
}

#[test]
fn bounded_peer_table_has_an_explicit_memory_ceiling() {
    // The service itself travels through the typed lifecycle, while all
    // fifteen WPA2 state machines remain in caller-owned static storage.
    assert!(core::mem::size_of::<AccessPointService<'_>>() <= 256);
    // Fifteen independently negotiated TX BlockAck sessions and the
    // per-peer HMAC-SHA1-128 Association-security commitments, exact
    // non-reusable RX association epochs, and eight independent QoS
    // sequence spaces remain explicit. The 16-byte sequence array is
    // required per receiver: sharing it across clients creates artificial
    // BlockAck holes. The bounded table still uses no dynamic allocation.
    assert!(
        core::mem::size_of::<AccessPointPeerStorage>() <= 4_928,
        "peer storage size {}",
        core::mem::size_of::<AccessPointPeerStorage>()
    );
}

#[test]
fn tx_block_ack_is_owned_by_the_exact_authorized_ht_peer() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = service(&mut storage);
    service.authenticate_open(PEER, 1);
    service
        .associate_wpa2(
            PEER,
            association_security(&WPA2_RSN),
            ht_capabilities(),
            [7; 32],
            9,
            1,
        )
        .unwrap();
    service.checked_peer_mut(PEER).unwrap().phase = ApPeerPhase::Authorized;

    service.authenticate_open(OTHER, 1);
    service
        .associate_wpa2(
            OTHER,
            association_security(&WPA2_RSN),
            LEGACY_CAPABILITIES,
            [8; 32],
            10,
            1,
        )
        .unwrap();
    service.checked_peer_mut(OTHER).unwrap().phase = ApPeerPhase::Authorized;

    let request = service.begin_tx_block_ack(PEER, 100).unwrap().unwrap();
    assert_eq!(
        u16::from_le_bytes([request.body[3], request.body[4]]) & 1,
        1,
        "AP requests only the source-owned baseline A-MSDU class"
    );
    assert_eq!(service.smallest_operational_tx_block_ack_window(), None);
    assert!(service.begin_tx_block_ack(PEER, 101).unwrap().is_none());
    assert!(service.begin_tx_block_ack(OTHER, 101).unwrap().is_none());
    let response = BlockAckAction::AddbaResponse {
        dialog_token: request.dialog_token,
        status: 0,
        tid: AP_TX_BLOCK_ACK_TID,
        immediate: true,
        amsdu: false,
        window: AP_TX_BLOCK_ACK_WINDOW,
        timeout_tu: 0,
    };
    assert!(matches!(
        service.on_tx_block_ack_action(PEER, response),
        Ok(Some(TxBlockAckResponse::Operational(
            OperationalTxBlockAck {
                tid: AP_TX_BLOCK_ACK_TID,
                window: AP_TX_BLOCK_ACK_WINDOW,
                ..
            }
        )))
    ));
    assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_some());
    assert!(
        !service
            .peer_status(PEER)
            .unwrap()
            .tx_block_ack
            .unwrap()
            .amsdu
    );
    assert!(service.peer_status(OTHER).unwrap().tx_block_ack.is_none());
    assert_eq!(
        service.smallest_operational_tx_block_ack_window(),
        Some(AP_TX_BLOCK_ACK_WINDOW)
    );

    service
        .on_tx_block_ack_action(
            PEER,
            BlockAckAction::Delba {
                tid: AP_TX_BLOCK_ACK_TID,
                initiator: true,
                reason: 37,
            },
        )
        .unwrap();
    assert!(
        service.peer_status(PEER).unwrap().tx_block_ack.is_some(),
        "peer-originated RX DELBA cannot revoke the AP-originated TX agreement"
    );
    assert_eq!(
        service.smallest_operational_tx_block_ack_window(),
        Some(AP_TX_BLOCK_ACK_WINDOW)
    );

    service
        .on_tx_block_ack_action(
            PEER,
            BlockAckAction::Delba {
                tid: AP_TX_BLOCK_ACK_TID,
                initiator: false,
                reason: 37,
            },
        )
        .unwrap();
    assert!(service.peer_status(PEER).unwrap().tx_block_ack.is_none());
    assert_eq!(service.smallest_operational_tx_block_ack_window(), None);

    let request = service.begin_tx_block_ack(PEER, 200).unwrap().unwrap();
    let response = BlockAckAction::AddbaResponse {
        dialog_token: request.dialog_token,
        status: 0,
        tid: AP_TX_BLOCK_ACK_TID,
        immediate: true,
        amsdu: true,
        window: AP_TX_BLOCK_ACK_WINDOW,
        timeout_tu: 0,
    };
    service.on_tx_block_ack_action(PEER, response).unwrap();
    assert!(
        service
            .peer_status(PEER)
            .unwrap()
            .tx_block_ack
            .unwrap()
            .amsdu
    );
}
