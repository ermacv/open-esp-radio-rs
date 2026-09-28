use super::*;
use oer_hil_protocol::bluetooth_peripheral_acl_payload;

const HANDLE: u16 = 0x0001;
const PLAIN: Profile = Profile {
    echoes: ECHOES,
    update: true,
    keys: Keys::None,
    refreshes: 0,
    local: None,
    reasons: &[SUPERVISION_TIMEOUT],
    phy_tracking: false,
};

fn connected() -> (Link, Cycle) {
    let mut link = Link::default();
    let mut cycle = Cycle::default();
    let mut complete = vec![
        LE_META,
        19,
        LE_CONNECTION_COMPLETE,
        0,
        1,
        0,
        PERIPHERAL_ROLE,
        0,
    ];
    complete.extend_from_slice(&[0x11; 6]);
    complete.extend_from_slice(&[80, 0, 0, 0, 200, 0, 0]);
    link.event(&complete, PLAIN, &mut cycle).unwrap();
    (link, cycle)
}

fn fragments(payload: &[u8]) -> Vec<Vec<u8>> {
    payload
        .chunks(usize::from(HOST_ACL_BYTES))
        .enumerate()
        .map(|(index, chunk)| {
            let flag = if index == 0 {
                CONTROLLER_START
            } else {
                CONTINUATION
            };
            let mut packet = (HANDLE | flag << 12).to_le_bytes().to_vec();
            packet.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
            packet.extend_from_slice(chunk);
            packet
        })
        .collect()
}

fn key_request(rand: [u8; 8], ediv: u16) -> Vec<u8> {
    let mut event = vec![LE_META, 13, LE_LONG_TERM_KEY_REQUEST, 1, 0];
    event.extend_from_slice(&rand);
    event.extend_from_slice(&ediv.to_le_bytes());
    event
}

#[test]
fn a_frame_reassembles_from_its_exact_fragments_and_holds_the_first_credit() {
    let (mut link, mut cycle) = connected();
    let payload = bluetooth_peripheral_acl_payload();
    let packets = fragments(&payload);
    assert_eq!(packets.len() as u32, FRAGMENTS_PER_FRAME);
    let mut completed = None;
    for (index, packet) in packets.iter().enumerate() {
        let (handle, frame) = link.acl(packet, &mut cycle).unwrap();
        assert_eq!(handle, HANDLE);
        if index == 0 {
            assert!(link.held_until.is_some());
        }
        completed = frame;
    }
    assert_eq!(completed.as_deref(), Some(&payload[..]));
    assert_eq!(cycle.credits_held, 1);
    assert_eq!(cycle.fragments, FRAGMENTS_PER_FRAME);
}

#[test]
fn out_of_order_boundaries_and_foreign_handles_fail() {
    let (mut link, mut cycle) = connected();
    let packets = fragments(&bluetooth_peripheral_acl_payload());
    assert!(link.acl(&packets[1], &mut cycle).is_err());
    let (mut link, mut cycle) = connected();
    let mut foreign = packets[0].clone();
    foreign[0] = 2;
    assert!(link.acl(&foreign, &mut cycle).is_err());
}

#[test]
fn completed_packets_free_the_controller_buffer_only_for_the_connection() {
    let (mut link, mut cycle) = connected();
    link.controller_busy = true;
    link.event(
        &[NUMBER_OF_COMPLETED_PACKETS, 5, 1, 2, 0, 1, 0],
        PLAIN,
        &mut cycle,
    )
    .unwrap();
    assert!(link.controller_busy);
    link.event(
        &[NUMBER_OF_COMPLETED_PACKETS, 5, 1, 1, 0, 1, 0],
        PLAIN,
        &mut cycle,
    )
    .unwrap();
    assert!(!link.controller_busy);
}

#[test]
fn each_termination_requires_its_own_disconnection_reason() {
    let finished = |termination, reason: Option<u8>| {
        let (mut link, mut cycle) = connected();
        link.echoes = ECHOES;
        cycle.updated_interval = Some(UPDATED_INTERVAL);
        if let Some(reason) = reason {
            link.event(
                &[DISCONNECTION_COMPLETE, 4, 0, 1, 0, reason],
                PLAIN,
                &mut cycle,
            )
            .unwrap();
        }
        link.finish(
            Profile::connect_reset(termination, Security::Plaintext),
            &cycle,
        )
    };
    assert!(finished(Termination::PeerReset, Some(SUPERVISION_TIMEOUT)).is_ok());
    assert!(finished(Termination::PeerRfkill, Some(REMOTE_USER_TERMINATED)).is_ok());
    assert!(finished(Termination::TargetDisconnect, Some(LOCAL_HOST_TERMINATED)).is_ok());
    assert!(finished(Termination::TargetReset, None).is_ok());
    assert!(finished(Termination::PeerReset, Some(REMOTE_USER_TERMINATED)).is_err());
    assert!(finished(Termination::TargetDisconnect, Some(SUPERVISION_TIMEOUT)).is_err());
    assert!(finished(Termination::TargetReset, Some(SUPERVISION_TIMEOUT)).is_err());
}

#[test]
fn a_missing_echo_or_update_fails_the_connection() {
    let (mut link, mut cycle) = connected();
    link.echoes = 1;
    cycle.updated_interval = Some(UPDATED_INTERVAL);
    link.reason = Some(SUPERVISION_TIMEOUT);
    assert!(link.finish(PLAIN, &cycle).is_err());
    link.echoes = ECHOES;
    cycle.updated_interval = Some(80);
    assert!(link.finish(PLAIN, &cycle).is_err());
}

#[test]
fn a_failed_connection_or_update_is_an_error() {
    let mut link = Link::default();
    let mut cycle = Cycle::default();
    let mut failed = vec![
        LE_META,
        19,
        LE_CONNECTION_COMPLETE,
        0x3e,
        1,
        0,
        PERIPHERAL_ROLE,
        0,
    ];
    failed.extend_from_slice(&[0; 13]);
    assert!(link.event(&failed, PLAIN, &mut cycle).is_err());
    let (mut link, mut cycle) = connected();
    let update = [
        LE_META,
        10,
        LE_CONNECTION_UPDATE_COMPLETE,
        0x3b,
        1,
        0,
        96,
        0,
        0,
        0,
        200,
        0,
    ];
    assert!(link.event(&update, PLAIN, &mut cycle).is_err());
}

#[test]
fn each_key_policy_answers_only_its_expected_requests() {
    let test = (BLUETOOTH_TEST_RAND, BLUETOOTH_TEST_EDIV);
    let refresh = (BLUETOOTH_REFRESH_RAND, BLUETOOTH_REFRESH_EDIV);
    let reply = |keys, encrypted, (rand, ediv)| key_reply(keys, encrypted, rand, ediv);
    assert_eq!(
        reply(Keys::Valid, false, test).unwrap(),
        KeyReply::Key(BLUETOOTH_TEST_LTK)
    );
    assert_eq!(
        reply(Keys::Valid, true, refresh).unwrap(),
        KeyReply::Key(BLUETOOTH_REFRESH_LTK)
    );
    assert_eq!(
        reply(Keys::Missing, false, test).unwrap(),
        KeyReply::Negative
    );
    assert_eq!(
        reply(Keys::Wrong, false, test).unwrap(),
        KeyReply::Key(WRONG_LTK)
    );
    assert_eq!(
        reply(Keys::MissingRefresh, false, test).unwrap(),
        KeyReply::Key(BLUETOOTH_TEST_LTK)
    );
    assert_eq!(
        reply(Keys::MissingRefresh, true, refresh).unwrap(),
        KeyReply::Negative
    );
    for (keys, encrypted, request) in [
        (Keys::None, false, test),
        (Keys::Valid, false, refresh),
        (Keys::Valid, true, test),
        (Keys::Missing, true, test),
        (Keys::Wrong, false, refresh),
    ] {
        assert!(
            reply(keys, encrypted, request).is_err(),
            "{keys:?} {encrypted}"
        );
    }
}

#[test]
fn a_key_refresh_connection_needs_both_keys_and_one_refresh() {
    let profile = Profile::connect_reset(Termination::PeerReset, Security::KeyRefresh);
    let (mut link, mut cycle) = connected();
    let first = link
        .event(
            &key_request(BLUETOOTH_TEST_RAND, BLUETOOTH_TEST_EDIV),
            profile,
            &mut cycle,
        )
        .unwrap();
    assert_eq!(first, Some(KeyReply::Key(BLUETOOTH_TEST_LTK)));
    link.event(&[ENCRYPTION_CHANGE, 4, 0, 1, 0, 1], profile, &mut cycle)
        .unwrap();
    let second = link
        .event(
            &key_request(BLUETOOTH_REFRESH_RAND, BLUETOOTH_REFRESH_EDIV),
            profile,
            &mut cycle,
        )
        .unwrap();
    assert_eq!(second, Some(KeyReply::Key(BLUETOOTH_REFRESH_LTK)));
    link.echoes = ECHOES;
    cycle.updated_interval = Some(UPDATED_INTERVAL);
    link.reason = Some(SUPERVISION_TIMEOUT);
    assert!(
        link.finish(profile, &cycle).is_err(),
        "no refresh completed"
    );
    link.event(
        &[ENCRYPTION_KEY_REFRESH_COMPLETE, 3, 0, 1, 0],
        profile,
        &mut cycle,
    )
    .unwrap();
    assert!(link.finish(profile, &cycle).is_ok());
}

#[test]
fn key_failures_end_with_their_own_reason_and_no_encryption() {
    for (failure, first_reply, reason) in [
        (
            BluetoothSecurityFailure::MissingKey,
            KeyReply::Negative,
            REMOTE_USER_TERMINATED,
        ),
        (
            BluetoothSecurityFailure::WrongKey,
            KeyReply::Key(WRONG_LTK),
            MIC_FAILURE,
        ),
    ] {
        let profile = Profile::security_failure(failure).unwrap();
        let (mut link, mut cycle) = connected();
        let reply = link
            .event(
                &key_request(BLUETOOTH_TEST_RAND, BLUETOOTH_TEST_EDIV),
                profile,
                &mut cycle,
            )
            .unwrap();
        assert_eq!(reply, Some(first_reply));
        link.reason = Some(reason);
        assert!(link.finish(profile, &cycle).is_ok(), "{failure:?}");
        link.encrypted = true;
        assert!(link.finish(profile, &cycle).is_err(), "{failure:?}");
    }
    assert!(Profile::security_failure(BluetoothSecurityFailure::ActiveDataMic).is_err());
}

#[test]
fn a_security_failure_scenario_recovers_on_an_encrypted_connection() {
    let plan = plan(Config::SecurityFailure {
        failure: BluetoothSecurityFailure::MissingRefreshKey,
        read_version_before_disconnect: false,
    })
    .unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].1.keys, Keys::MissingRefresh);
    assert_eq!(plan[0].1.reasons, &[KEY_MISSING]);
    assert_eq!(
        plan[1].1,
        Profile::connect_reset(Termination::PeerReset, Security::Encrypted)
    );
}

#[test]
fn a_failed_encryption_change_is_never_accepted() {
    let profile = Profile::connect_reset(Termination::PeerReset, Security::Encrypted);
    for event in [
        [ENCRYPTION_CHANGE, 4, KEY_MISSING, 1, 0, 0],
        [ENCRYPTION_CHANGE, 4, 0, 1, 0, 0],
    ] {
        let (mut link, mut cycle) = connected();
        assert!(
            link.event(&event, profile, &mut cycle).is_err(),
            "{event:02x?}"
        );
    }
}

#[test]
fn phy_tracking_must_complete_a_pass_without_a_skip_or_suspension() {
    let reading = |running, tracked, skipped| PhyTrackingEvidence {
        running,
        tracked,
        not_due: 0,
        skipped,
    };
    assert!(tracked_during(Some(reading(true, 3, 1)), Some(reading(true, 4, 1))).is_ok());
    for (before, after) in [
        (Some(reading(true, 3, 1)), Some(reading(true, 3, 1))),
        (Some(reading(true, 3, 1)), Some(reading(true, 5, 2))),
        (Some(reading(false, 3, 1)), Some(reading(true, 5, 1))),
        (Some(reading(true, 3, 1)), Some(reading(false, 5, 1))),
        (None, Some(reading(true, 5, 1))),
        (Some(reading(true, 3, 1)), None),
    ] {
        assert!(
            tracked_during(before, after).is_err(),
            "{before:?} {after:?}"
        );
    }
}

#[test]
fn an_epoch_ends_only_with_a_closed_old_host_and_the_requested_restart() {
    let evidence = |old_host_closed, restarted| BluetoothHciLifecycleEvidence {
        old_host_closed,
        restarted,
    };
    assert!(ended(BluetoothHciLifecycle::Restart, evidence(true, true)).is_ok());
    assert!(ended(BluetoothHciLifecycle::Retire, evidence(true, false)).is_ok());
    assert!(ended(BluetoothHciLifecycle::Restart, evidence(false, false)).is_err());
    assert!(ended(BluetoothHciLifecycle::Restart, evidence(true, false)).is_err());
    assert!(ended(BluetoothHciLifecycle::Retire, evidence(false, false)).is_err());
    assert!(ended(BluetoothHciLifecycle::Retire, evidence(true, true)).is_err());
}
