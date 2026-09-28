use super::*;
use oer_ieee80211_mac::security::SaePwe;

use oer_ieee80211_rsn::sae::SaePasswordElement;
use std::vec::Vec;

const LOCAL: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const BSSID: [u8; 6] = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
const PASSWORD: &[u8] = b"correct horse";

fn commit(local: [u8; 6], peer: [u8; 6], seed: u8) -> SaeCommit {
    SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(PASSWORD, local, peer).unwrap(),
        [seed; 32],
        [seed + 1; 32],
    )
    .unwrap()
}

/// One Authentication frame from the access point.
fn from_access_point(transaction: u16, status: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 30];
    frame[0] = 0xb0;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&3_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&transaction.to_le_bytes());
    frame[28..30].copy_from_slice(&status.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

fn access_point_commit(access_point: &SaeCommit) -> Vec<u8> {
    let mut body = [0; SAE_COMMIT_LEN];
    access_point
        .values()
        .encode(None, false, &mut body)
        .unwrap();
    from_access_point(1, 0, &body)
}

#[test]
fn a_full_exchange_yields_the_access_point_pmk() {
    let access_point = commit(BSSID, LOCAL, 0x40);
    let mut station = StaSaeAuthentication::new(
        LOCAL,
        BSSID,
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
    );
    let sent = station.commit();
    assert_eq!((sent.transaction, sent.status_code), (1, 0));
    let station_values = SaeCommitValues::parse(sent.body(), false).unwrap();

    let StaSaeEvent::Transmit(confirm) =
        station.observe_management_frame(&access_point_commit(&access_point))
    else {
        panic!("the station confirms after the peer commit");
    };
    assert_eq!((confirm.transaction, confirm.status_code), (2, 0));
    let access_point_keys = access_point.process(station_values).unwrap();
    assert_eq!(access_point_keys.verify_peer_confirm(confirm.body()), Ok(1));

    let reply = from_access_point(2, 0, &access_point_keys.own_confirm(1));
    assert_eq!(
        station.observe_management_frame(&reply),
        StaSaeEvent::Authenticated(StaSaePmk {
            pmk: access_point_keys.pmk,
            pmkid: access_point_keys.pmkid,
        })
    );
}

#[test]
fn an_anti_clogging_refusal_repeats_the_commit_with_the_token() {
    let mut station = StaSaeAuthentication::new(
        LOCAL,
        BSSID,
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
    );
    let first = station.commit();
    let StaSaeEvent::Transmit(again) =
        station.observe_management_frame(&from_access_point(1, 76, &[19, 0, 7, 8, 9]))
    else {
        panic!("the station repeats its commit");
    };
    assert_eq!(&again.body()[2..5], &[7, 8, 9]);
    assert_eq!(&again.body()[5..], &first.body()[2..]);
}

#[test]
fn the_timer_is_four_seconds_then_two_after_the_confirm() {
    let mut station = StaSaeAuthentication::new(
        LOCAL,
        BSSID,
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
    );
    for _ in 1..STA_SAE_COMMIT_TIMEOUT_MS {
        assert_eq!(station.finish_millisecond(), StaSaeEvent::Irrelevant);
    }
    // The peer commit resets the timer to the confirm exchange.
    let access_point = commit(BSSID, LOCAL, 0x40);
    assert!(matches!(
        station.observe_management_frame(&access_point_commit(&access_point)),
        StaSaeEvent::Transmit(_)
    ));
    for _ in 1..STA_SAE_CONFIRM_TIMEOUT_MS {
        assert_eq!(station.finish_millisecond(), StaSaeEvent::Irrelevant);
    }
    assert_eq!(
        station.finish_millisecond(),
        StaSaeEvent::Failed(StaSaeFailure::Timeout)
    );
}

#[test]
fn a_wrong_password_or_a_refusal_fails() {
    let mut station = StaSaeAuthentication::new(
        LOCAL,
        BSSID,
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
    );
    let impostor = SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(b"wrong", BSSID, LOCAL).unwrap(),
        [0x40; 32],
        [0x41; 32],
    )
    .unwrap();
    let sent = SaeCommitValues::parse(station.commit().body(), false).unwrap();
    assert!(matches!(
        station.observe_management_frame(&access_point_commit(&impostor)),
        StaSaeEvent::Transmit(_)
    ));
    let impostor_keys = impostor.process(sent).unwrap();
    assert_eq!(
        station.observe_management_frame(&from_access_point(2, 0, &impostor_keys.own_confirm(1))),
        StaSaeEvent::Failed(StaSaeFailure::Protocol(SaeError::ConfirmMismatch))
    );

    let mut refused = StaSaeAuthentication::new(
        LOCAL,
        BSSID,
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
    );
    assert_eq!(
        refused.observe_management_frame(&from_access_point(1, 77, &[20, 0])),
        StaSaeEvent::Failed(StaSaeFailure::Rejected { status_code: 77 })
    );
    // An H2E station refuses a hunting-and-pecking commit.
    let mut h2e = StaSaeAuthentication::new(
        LOCAL,
        BSSID,
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HashToElement,
    );
    assert_eq!(h2e.commit().status_code, 126);
    assert_eq!(
        h2e.observe_management_frame(&access_point_commit(&commit(BSSID, LOCAL, 0x40))),
        StaSaeEvent::Failed(StaSaeFailure::Protocol(SaeError::Malformed))
    );
}
