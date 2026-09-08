use super::*;

#[test]
fn discovery_owns_management_tx_without_creating_a_peer_or_association_completion() {
    let ap = [2, 0, 0, 0, 0, 1];
    let mut hardware = Hardware::default();
    let mut beacon = [0; WPA2_BEACON_CAPACITY];
    let mut peers = oer_wifi_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::ApPairwiseKeyStorage::new();
    let engine = ApEngine::start(
        &mut hardware,
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            Wpa2Gtk::new(1, true, [7; 16]).unwrap(),
            oer_wifi_ap::AccessPointClientLimit::new(2).unwrap(),
            oer_wifi_ap::AccessPointInactiveTimeout::default(),
            &mut peers,
        ),
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        oer_ieee80211::channel::WifiChannel::mhz20(6).unwrap(),
        100,
        2,
    )
    .unwrap_or_else(|_| panic!("AP engine starts"));
    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = ApMac::new(
        engine,
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: Timer,
        },
        ApTxConfig {
            publication_timeout_micros: 1_000,
        },
    );

    let peer = [2, 0, 0, 0, 0, 2];
    let mut request = [0; 28];
    request[0] = 0x40;
    request[4..10].fill(0xff);
    request[10..16].copy_from_slice(&peer);
    request[16..22].fill(0xff);
    request[24..].copy_from_slice(&[0, 2, b'a', b'p']);
    let mut output = [0; 256];
    let outcome = mac
        .publish_management(&mut hardware, &request, [0; 32], 0, 100, &mut output)
        .unwrap();
    assert!(matches!(
        outcome,
        ApManagementOutcome::Response {
            begin_wpa2: false,
            ..
        }
    ));
    assert_eq!(
        mac.pending_publication_kind(),
        Some(ApPendingPublicationKind::ProbeResponse)
    );
    assert!(mac.engine().peer_status(peer).is_none());
    assert_eq!(hardware.publications, 1);
    assert!(
        mac.publish_management(&mut hardware, &request, [0; 32], 0, 101, &mut output)
            .is_err()
    );
    assert_eq!(
        hardware.publications, 1,
        "hardware owner must not be overwritten"
    );
    hardware.completion = Some(MacTxCompletionObservation::new_model(0, 0));
    let (_, action) = mac
        .service_tx(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: oer_esp32s31_wifi_mac::irq::EVENT_TX_COMPLETE,
            },
            102,
        )
        .unwrap();
    assert_eq!(action, ApTxCompletionAction::None);
    assert_eq!(mac.observation().association_responses_transmitted, 0);
    assert_eq!(mac.observation().authentication_responses_transmitted, 0);
    assert!(mac.engine().peer_status(peer).is_none());
    request[27] = b'x';
    assert_eq!(
        mac.publish_management(&mut hardware, &request, [0; 32], 0, 103, &mut output)
            .unwrap(),
        ApManagementOutcome::Ignored
    );
    assert_eq!(hardware.publications, 1);
    assert!(mac.try_into_parts().is_ok());
}
