use super::*;
use open_esp_radio_esp32s31_hal::types::MacKeyInstallOutcome;
use open_esp_radio_ieee80211::ccmp::{CcmpHeader, CcmpPacketNumber, CcmpReplayError};
use open_esp_radio_wpa2::{
    OwnedEapolFrame, Pmk, PtkContext, Wpa2Interface,
    frames::{OwnedRsnIe, Wpa2Gtk, Wpa2TxFrame},
};

#[derive(Default)]
struct Hardware {
    policy: Option<[u8; 6]>,
    installed: std::vec::Vec<u8>,
    cleared: std::vec::Vec<u8>,
    tsf_started: bool,
    tsf_stopped: bool,
}

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, address: [u8; 6]) {
        self.policy = Some(address);
    }

    fn disable_ap_link_policy(&mut self) {
        self.policy = None;
    }
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        _identity: open_esp_radio_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.installed.push(index);
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.cleared.push(index);
    }
}

impl ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(&mut self) {
        self.tsf_started = true;
    }

    fn stop_access_point_tsf(&mut self) {
        self.tsf_stopped = true;
    }
}

fn service(
    ap: [u8; 6],
    storage: &mut open_esp_radio_wifi_ap::AccessPointPeerStorage,
) -> AccessPointService<'_> {
    AccessPointService::new(
        ap,
        Pmk::derive(b"password", b"ap").unwrap(),
        Wpa2Gtk::new(1, true, [0x55; 16]).unwrap(),
        open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
        open_esp_radio_wifi_ap::AccessPointInactiveTimeout::default(),
        storage,
    )
}

#[test]
fn active_epoch_owns_policy_group_key_management_and_stop_frontier() {
    let ap = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut hardware = Hardware::default();
    let mut engine = Esp32s31ApEngine::start(
        &mut hardware,
        service(ap, &mut peers),
        &mut beacon,
        &mut pairwise,
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("AP start"));

    let mut request = [0; 30];
    request[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    request[4..10].copy_from_slice(&ap);
    request[10..16].copy_from_slice(&peer);
    request[16..22].copy_from_slice(&ap);
    request[26..28].copy_from_slice(&1_u16.to_le_bytes());
    let mut response = [0; 160];
    assert_eq!(
        engine
            .handle_management(&mut hardware, &request, [1; 32], 7, 1, &mut response)
            .unwrap(),
        Esp32s31ApManagementOutcome::Response {
            len: 30,
            begin_wpa2: false
        }
    );
    assert!(engine.prepare_beacon(102_400).is_some());

    let observation = engine.observation();
    let _stopped = engine.stop(&mut hardware);
    assert_eq!(hardware.policy, None);
    assert_eq!(hardware.installed, [2]);
    assert_eq!(hardware.cleared, [2]);
    assert!(hardware.tsf_started);
    assert!(hardware.tsf_stopped);
    assert_eq!(
        observation,
        Esp32s31ApEngineObservation {
            beacons_prepared: 1,
            authentication_responses_prepared: 1,
            ..Esp32s31ApEngineObservation::default()
        }
    );
}

#[test]
fn associated_peer_stop_emits_vendor_ordered_disconnects_before_removal() {
    const RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];
    let ap = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut hardware = Hardware::default();
    let mut engine = Esp32s31ApEngine::start(
        &mut hardware,
        service(ap, &mut peers),
        &mut beacon,
        &mut pairwise,
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("AP start"));

    let mut authentication = [0; 30];
    authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    authentication[4..10].copy_from_slice(&ap);
    authentication[10..16].copy_from_slice(&peer);
    authentication[16..22].copy_from_slice(&ap);
    authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
    let mut output = [0; 160];
    engine
        .handle_management(&mut hardware, &authentication, [7; 32], 9, 1, &mut output)
        .unwrap();
    assert_eq!(
        engine.tx_protection_policy().ht(),
        HtProtectionMode::None,
        "an authenticated but not associated station is not an HT BSS member"
    );

    let mut association = [0; 56];
    association[24..26].copy_from_slice(&0x0010_u16.to_le_bytes());
    association[4..10].copy_from_slice(&ap);
    association[10..16].copy_from_slice(&peer);
    association[16..22].copy_from_slice(&ap);
    association[28..34].copy_from_slice(&[1, 4, 12, 24, 48, 108]);
    association[34..].copy_from_slice(&RSN);
    engine
        .handle_management(&mut hardware, &association, [7; 32], 9, 2, &mut output)
        .unwrap();
    assert_eq!(
        engine.tx_protection_policy().ht(),
        HtProtectionMode::NonHtMixed,
        "an associated station without HT Capabilities requires mixed-mode protection"
    );
    assert!(matches!(
        engine
            .handle_management(&mut hardware, &association, [8; 32], 10, 3, &mut output)
            .unwrap(),
        Esp32s31ApManagementOutcome::Response {
            begin_wpa2: false,
            ..
        }
    ));
    assert_eq!(
        engine.service.peer_status(peer).unwrap().phase,
        ApPeerPhase::Securing,
        "a duplicate association must not replace the in-flight WPA2 owner"
    );
    let mut retried_authentication = authentication;
    retried_authentication[1] |= 0x08;
    assert!(matches!(
        engine
            .handle_management(
                &mut hardware,
                &retried_authentication,
                [9; 32],
                11,
                4,
                &mut output,
            )
            .unwrap(),
        Esp32s31ApManagementOutcome::Response {
            begin_wpa2: false,
            ..
        }
    ));
    assert_eq!(
        engine.service.peer_status(peer).unwrap().phase,
        ApPeerPhase::Securing,
        "an authentication retry must not erase the in-flight WPA2 owner"
    );

    let close = engine.begin_stop_peer().expect("associated peer to close");
    assert!(close.was_associated);

    let mut peer_deauthentication = [0; 26];
    peer_deauthentication[..2].copy_from_slice(&0x00c0_u16.to_le_bytes());
    peer_deauthentication[4..10].copy_from_slice(&ap);
    peer_deauthentication[10..16].copy_from_slice(&peer);
    peer_deauthentication[16..22].copy_from_slice(&ap);
    peer_deauthentication[24..26].copy_from_slice(&3_u16.to_le_bytes());
    assert_eq!(
        engine
            .handle_management(
                &mut hardware,
                &peer_deauthentication,
                [9; 32],
                10,
                3,
                &mut output,
            )
            .unwrap(),
        Esp32s31ApManagementOutcome::Ignored
    );
    assert_eq!(
        engine.service.peer_status(peer).unwrap().phase,
        ApPeerPhase::Closing
    );

    let disassociation = engine
        .encode_peer_disconnect(close, ApPeerDisconnectKind::Disassociation, 2, &mut output)
        .unwrap();
    assert_eq!(
        disassociation,
        open_esp_radio_ieee80211::ap::AP_PEER_DISCONNECT_LEN
    );
    assert_eq!(&output[..2], &0x00a0_u16.to_le_bytes());
    assert_eq!(&output[24..26], &2_u16.to_le_bytes());

    let deauthentication = engine
        .encode_peer_disconnect(
            close,
            ApPeerDisconnectKind::Deauthentication,
            2,
            &mut output,
        )
        .unwrap();
    assert_eq!(
        deauthentication,
        open_esp_radio_ieee80211::ap::AP_PEER_DISCONNECT_LEN
    );
    assert_eq!(&output[..2], &0x00c0_u16.to_le_bytes());
    assert_eq!(&output[24..26], &2_u16.to_le_bytes());

    engine.complete_peer_close(&mut hardware, close).unwrap();
    assert!(engine.service_status().peers.iter().all(Option::is_none));
    assert_eq!(engine.observation().peer_removals, 1);
    assert_eq!(engine.observation().disassociations_prepared, 1);
    assert_eq!(engine.observation().deauthentications_prepared, 1);
    let _ = engine.stop(&mut hardware);
}

#[test]
fn message_four_installs_pairwise_key_before_authorization_is_reported() {
    const RSN: [u8; 22] = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];
    const ANONCE: [u8; 32] = [7; 32];
    const SNONCE: [u8; 32] = [8; 32];
    let ap = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut hardware = Hardware::default();
    let mut engine = Esp32s31ApEngine::start(
        &mut hardware,
        service(ap, &mut peers),
        &mut beacon,
        &mut pairwise,
        &ssid,
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("AP start"));

    let mut authentication = [0; 30];
    authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    authentication[4..10].copy_from_slice(&ap);
    authentication[10..16].copy_from_slice(&peer);
    authentication[16..22].copy_from_slice(&ap);
    authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
    let mut response = [0; 160];
    engine
        .handle_management(&mut hardware, &authentication, ANONCE, 9, 1, &mut response)
        .unwrap();

    let mut association = [0; 84];
    association[24..26].copy_from_slice(&0x0010_u16.to_le_bytes());
    association[4..10].copy_from_slice(&ap);
    association[10..16].copy_from_slice(&peer);
    association[16..22].copy_from_slice(&ap);
    association[28..34].copy_from_slice(&[1, 4, 12, 24, 48, 108]);
    association[34..56].copy_from_slice(&RSN);
    association[56..].copy_from_slice(&open_esp_radio_ieee80211::ht::ht_capability_ie(
        crate::profile::HT_CAPABILITIES,
        WifiChannel::mhz20(6).unwrap(),
    ));
    assert!(matches!(
        engine
            .handle_management(&mut hardware, &association, ANONCE, 9, 2, &mut response)
            .unwrap(),
        Esp32s31ApManagementOutcome::Response {
            begin_wpa2: true,
            ..
        }
    ));
    engine.begin_wpa2::<512>(peer).unwrap();

    let ptk = Pmk::derive(b"password", b"ap")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: ap,
            supplicant_address: peer,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        });
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let message2 = Wpa2TxFrame::<512>::message2(ap, 9, SNONCE, &rsn)
        .unwrap()
        .authenticate(&ptk);
    let message2 =
        OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, peer, message2.as_bytes())
            .unwrap();
    let Esp32s31ApWpa2Outcome::Transmit(message3) = engine
        .handle_eapol(&mut hardware, peer, message2, 3)
        .unwrap()
    else {
        panic!("message two must produce message three");
    };
    let mut message3_mpdu = [0; 768];
    let message3_len = engine
        .encode_eapol(peer, &message3, &mut message3_mpdu)
        .unwrap();
    assert!(message3_len > message3.as_bytes().len());
    assert_eq!(&message3_mpdu[4..10], &peer);
    assert_eq!(&message3_mpdu[10..16], &ap);
    assert_eq!(&message3_mpdu[22..24], &[0, 0]);
    assert_eq!(&message3_mpdu[30..32], &[0x88, 0x8e]);
    let message4 = Wpa2TxFrame::<512>::message4(ap, 10)
        .unwrap()
        .authenticate(&ptk);
    let message4 =
        OwnedEapolFrame::<512>::try_copy(Wpa2Interface::AccessPoint, peer, message4.as_bytes())
            .unwrap();
    assert!(matches!(
        engine
            .handle_eapol(&mut hardware, peer, message4, 4)
            .unwrap(),
        Esp32s31ApWpa2Outcome::PeerAuthorized { peer: authorized } if authorized == peer
    ));
    assert_eq!(engine.observation().authorized_peers, 1);

    let rx_pn3 = CcmpPacketNumber::new(3).unwrap();
    let rx_request = Esp32s31ApRxAdmissionRequest::new(
        peer,
        CcmpReplayLane::NonQos,
        Some(CcmpHeader::new(rx_pn3, CcmpKeyId::PAIRWISE)),
    );
    let duplicate_owner = Esp32s31ApRxDuplicateOwner::new(
        engine.service.peer_status(peer).unwrap().association_id,
        engine.service.peer_status(peer).unwrap().association_epoch,
    )
    .unwrap()
    .with_key_generation(1);
    assert_eq!(
        engine.admit_rx_data(rx_request),
        Esp32s31ApRxAdmission::authorized(duplicate_owner)
    );
    assert_eq!(
        engine.admit_rx_data(rx_request),
        Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(CcmpReplayError::Replayed {
            packet_number: rx_pn3,
            highest: rx_pn3,
        })),
    );
    let rx_pn4 = CcmpPacketNumber::new(4).unwrap();
    let ordinary_rx_request = Esp32s31ApOrdinaryPairwiseRxRequest::new(
        peer,
        CcmpReplayLane::NonQos,
        CcmpHeader::new(rx_pn4, CcmpKeyId::PAIRWISE),
    );
    let deadline = engine.service.peer_status(peer).unwrap().deadline_micros;
    let (admission, activity) = engine.admit_ordinary_pairwise_rx_with_activity(
        ordinary_rx_request,
        ApPeerPowerState::Active,
        5,
    );
    assert_eq!(
        admission,
        Esp32s31ApRxAdmission::authorized(duplicate_owner)
    );
    assert_eq!(activity.unwrap(), Some(ApPowerSaveAction::None));
    assert_eq!(
        engine.service.peer_status(peer).unwrap().deadline_micros,
        deadline,
        "ordinary admission and coalesced activity share one peer binding"
    );

    let repeated_message4 = Wpa2TxFrame::<512>::message4(ap, 10)
        .unwrap()
        .authenticate(&ptk);
    let repeated_message4 = OwnedEapolFrame::<512>::try_copy(
        Wpa2Interface::AccessPoint,
        peer,
        repeated_message4.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        engine
            .handle_eapol(&mut hardware, peer, repeated_message4, 5)
            .unwrap(),
        Esp32s31ApWpa2Outcome::None
    ));
    assert_eq!(engine.observation().authorized_peers, 1);

    let mut ethernet = [0_u8; 18];
    ethernet[..6].copy_from_slice(&peer);
    ethernet[6..12].copy_from_slice(&ap);
    ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    ethernet[14..].copy_from_slice(&[1, 2, 3, 4]);
    let next_data_sequence = engine.service.current_data_sequence();
    let mut too_small = [0xa5; 32];
    assert!(matches!(
        engine.encode_protected_ethernet_with_more_data(peer, &ethernet, &mut too_small, false,),
        Err(Esp32s31ApEngineError::DataFrame(
            ApDataFrameError::OutputTooSmall { .. }
        ))
    ));
    assert_eq!(engine.service.current_data_sequence(), next_data_sequence);
    let mut protected = [0_u8; 96];
    let prepared = engine
        .encode_protected_ethernet_with_more_data(peer, &ethernet, &mut protected, false)
        .unwrap();
    assert_eq!(engine.service.current_data_sequence(), next_data_sequence);
    assert_eq!(&protected[26..34], &[0; 8]);
    let encoded = engine
        .commit_prepared_data(prepared, &mut protected)
        .unwrap();
    assert_eq!(encoded.hardware_key_selector, Some(8));
    assert_eq!(&protected[..2], &0x4288_u16.to_le_bytes());
    assert_eq!(&protected[22..24], &0x0000_u16.to_le_bytes());
    assert_eq!(&protected[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(&protected[42..46], &[1, 2, 3, 4]);
    assert_eq!(engine.service.current_data_sequence(), 1);
    assert_eq!(engine.service.current_qos_sequence(peer, 0), Some(1));

    let request = engine
        .service
        .begin_tx_block_ack(peer, 100)
        .unwrap()
        .unwrap();
    engine
        .service
        .on_tx_block_ack_action(
            peer,
            open_esp_radio_ieee80211::block_ack::BlockAckAction::AddbaResponse {
                dialog_token: request.dialog_token,
                status: 0,
                tid: open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_TID,
                immediate: true,
                amsdu: true,
                window: open_esp_radio_wifi_ap::AP_TX_BLOCK_ACK_WINDOW,
                timeout_tu: 0,
            },
        )
        .unwrap();
    let mut second = ethernet;
    second[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 7]);
    second[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
    second[14..].copy_from_slice(&[5, 6, 7, 8]);
    let qos_sequence = engine.service.current_qos_sequence(peer, 0);
    let mut short_amsdu = [0xa5; 64];
    assert_eq!(
        engine
            .encode_amsdu_ethernet_pair(&ethernet, &second, &mut short_amsdu)
            .unwrap(),
        None
    );
    assert_eq!(short_amsdu, [0xa5; 64]);
    assert_eq!(engine.service.current_qos_sequence(peer, 0), qos_sequence);
    let mut amsdu = [0_u8; 128];
    let prepared_amsdu = engine
        .encode_amsdu_ethernet_pair(&ethernet, &second, &mut amsdu)
        .unwrap()
        .expect("negotiated pair fits the bounded AP scratch");
    assert_eq!(engine.service.current_qos_sequence(peer, 0), qos_sequence);
    assert_eq!(&amsdu[26..34], &[0; 8]);
    let encoded_amsdu = engine
        .commit_prepared_amsdu(prepared_amsdu, &mut amsdu)
        .unwrap();
    assert_eq!(encoded_amsdu.hardware_key_selector, Some(8));
    assert_eq!(&amsdu[..2], &0x4288_u16.to_le_bytes());
    assert_eq!(amsdu[24], 0x80);
    assert_eq!(&amsdu[26..34], &[6, 0, 0, 0x20, 0, 0, 0, 0]);
    let mut subframes = open_esp_radio_ieee80211::data::amsdu_subframes(
        open_esp_radio_ieee80211::data::DataInterfaceRole::Station,
        &amsdu[..encoded_amsdu.length],
        open_esp_radio_ieee80211::data::IEEE80211_QOS_DATA_HEADER_LEN + 8,
        encoded_amsdu.length - open_esp_radio_ieee80211::data::IEEE80211_QOS_DATA_HEADER_LEN - 8,
    )
    .unwrap();
    assert_eq!(subframes.next().unwrap().unwrap().payload, &[1, 2, 3, 4]);
    assert_eq!(subframes.next().unwrap().unwrap().payload, &[5, 6, 7, 8]);
    assert!(subframes.next().is_none());

    ethernet[..6].fill(0xff);
    let group_sequence = engine.service.current_data_sequence();
    let prepared = engine
        .encode_protected_ethernet_with_more_data([0xff; 6], &ethernet, &mut protected, false)
        .unwrap();
    assert_eq!(engine.service.current_data_sequence(), group_sequence);
    assert_eq!(&protected[24..32], &[0; 8]);
    let encoded = engine
        .commit_prepared_data(prepared, &mut protected)
        .unwrap();
    assert_eq!(encoded.hardware_key_selector, Some(2));
    assert_eq!(&protected[24..32], &[3, 0, 0, 0x60, 0, 0, 0, 0]);
    assert_eq!(engine.service.current_data_sequence(), 2);

    // Supplicants may restart authentication without a preceding
    // deauthentication. The old PTK must leave hardware before the same
    // AID begins a new handshake.
    engine
        .handle_management(&mut hardware, &authentication, ANONCE, 11, 5, &mut response)
        .unwrap();
    assert_eq!(hardware.cleared, [8]);
    assert!(!engine.is_authorized_peer(peer));
    assert_eq!(
        engine.admit_rx_data(rx_request),
        Esp32s31ApRxAdmission::unauthorized(),
        "reauthentication closes the controlled port before old-PN admission"
    );
    assert_eq!(engine.service.peer_status(peer).unwrap().association_id, 1);

    let _stopped = engine.stop(&mut hardware);
    assert_eq!(hardware.installed, [2, 8]);
    assert_eq!(hardware.cleared, [8, 2]);
}

#[test]
fn open_ht_peer_uses_bounded_qos_amsdu_without_key_or_block_ack_owner() {
    let ap = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = open_esp_radio_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let mut service = AccessPointService::new_open(
        ap,
        open_esp_radio_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
        open_esp_radio_wifi_ap::AccessPointInactiveTimeout::default(),
        &mut peers,
    );
    service.authenticate_open(peer, 1);
    let ht_ie = open_esp_radio_ieee80211::ht::ht_capability_ie(
        crate::profile::HT_CAPABILITIES,
        WifiChannel::mhz20(6).unwrap(),
    );
    service
        .associate_open(
            peer,
            open_esp_radio_ieee80211::ap::ApAssociationSecurityObservation {
                privacy: false,
                rsn_ie: None,
                rsn_ie_count: 0,
                rsnxe: None,
                rsnxe_count: 0,
                legacy_wpa_present: false,
                malformed_elements: false,
            },
            open_esp_radio_wifi_ap::ApAssociationCapabilities {
                maximum_legacy_rate_500kbps: 108,
                ht: open_esp_radio_ieee80211::ht::ht_peer_capabilities(&ht_ie),
                qos_supported: true,
            },
            2,
        )
        .unwrap();
    let mut hardware = Hardware::default();
    let mut engine = Esp32s31ApEngine::start(
        &mut hardware,
        service,
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("Open AP starts"));

    let mut first = [0_u8; 18];
    first[..6].copy_from_slice(&peer);
    first[6..12].copy_from_slice(&ap);
    first[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    first[14..].copy_from_slice(&[1, 2, 3, 4]);
    let mut second = first;
    second[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 7]);
    second[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
    second[14..].copy_from_slice(&[5, 6, 7, 8]);
    let mut output = [0; 128];
    let prepared = engine
        .encode_amsdu_ethernet_pair(&first, &second, &mut output)
        .unwrap()
        .expect("Open HT/QoS pair is admitted");
    assert_eq!(engine.service.current_qos_sequence(peer, 0), Some(0));
    let encoded = engine.commit_prepared_amsdu(prepared, &mut output).unwrap();
    assert_eq!(encoded.hardware_key_selector, None);
    assert_eq!(&output[..2], &0x0288_u16.to_le_bytes());
    assert_eq!(output[24], 0x80);
    assert_eq!(engine.service.current_qos_sequence(peer, 0), Some(1));
    assert_eq!(engine.service.current_data_sequence(), 0);
    assert!(
        engine
            .service
            .peer_status(peer)
            .unwrap()
            .tx_block_ack
            .is_none()
    );
    let _ = engine.stop(&mut hardware);
}
