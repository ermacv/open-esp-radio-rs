use hmac::Mac;
use std::{
    boxed::Box,
    future::Future,
    task::{Context, Poll, Waker},
};

use super::*;
use crate::{
    aes::{Wpa2SoftwareAes, Wpa2UnwrappedKeyData, software_aes128_key_wrap},
    frames::{Wpa2Gtk, Wpa2PlainKeyData},
    keys::Wpa2KeyKind,
};

const LOCAL: [u8; 6] = [1; 6];
const AP: [u8; 6] = [2; 6];
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn owned(frame: &Wpa2TxFrame<512>) -> crate::OwnedEapolFrame<512> {
    crate::OwnedEapolFrame::try_copy(Wpa2Interface::Station, AP, frame.as_bytes()).unwrap()
}

fn context() -> PtkContext {
    PtkContext {
        authenticator_address: AP,
        supplicant_address: LOCAL,
        authenticator_nonce: ANONCE,
        supplicant_nonce: SNONCE,
    }
}

fn encrypted_message3(
    ptk: &Ptk,
    key_rsc: [u8; 8],
    plain_key_data: &[u8],
) -> crate::OwnedEapolFrame<512> {
    let wrapped = software_aes128_key_wrap(ptk.kek(), plain_key_data).unwrap();
    let frame = Wpa2TxFrame::<512>::message3(LOCAL, 2, ANONCE, key_rsc, wrapped.as_bytes())
        .unwrap()
        .authenticate(ptk);
    owned(&frame)
}

struct GroupKeyUnwrap {
    plain: [u8; 24],
}

impl AsyncWpa2KeyUnwrap for GroupKeyUnwrap {
    type Error = ();

    async fn unwrap_key_data(
        &mut self,
        _kek: &[u8; 16],
        _encrypted: &[u8],
    ) -> Result<Wpa2UnwrappedKeyData, Self::Error> {
        Ok(Wpa2UnwrappedKeyData::try_copy(&self.plain).unwrap())
    }
}

fn connected(ptk: Ptk) -> Wpa2ConnectedSupplicant {
    let message3 = Wpa2TxFrame::<512>::message3(LOCAL, 2, ANONCE, [0; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let completed_message3 = Wpa2CompletedMessage3::capture(message3.key_frame());
    let (key_confirmation, key_encryption) = ptk.into_connected_keys();
    Wpa2ConnectedSupplicant {
        authenticator: AP,
        key_confirmation,
        key_encryption,
        completed_message3,
        replay_counter: 2,
        completed_group_message1: None,
        pending: None,
        next_ticket: 1,
    }
}

fn resign_eapol_key_variant(
    original: &crate::OwnedEapolFrame<512>,
    ptk: &Ptk,
    resign: bool,
    mutate: impl FnOnce(&mut [u8]),
) -> crate::OwnedEapolFrame<512> {
    let mut bytes = [0; 512];
    let len = original.as_bytes().len();
    bytes[..len].copy_from_slice(original.as_bytes());
    mutate(&mut bytes[..len]);
    if resign {
        bytes[81..97].fill(0);
        let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(ptk.kck()).unwrap();
        mac.update(&bytes[..len]);
        let digest = mac.finalize().into_bytes();
        bytes[81..97].copy_from_slice(&digest[..16]);
    }
    crate::OwnedEapolFrame::try_copy(Wpa2Interface::Station, AP, &bytes[..len]).unwrap()
}

fn completed_connected() -> (Wpa2ConnectedSupplicant, crate::OwnedEapolFrame<512>, Ptk) {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let expected_ptk = pmk.derive_ptk(context());
    let mut supplicant = Wpa2StaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let mut aes = Wpa2SoftwareAes::new();
    let message1 = Wpa2TxFrame::<512>::message1(LOCAL, 1, ANONCE).unwrap();
    block_on(supplicant.on_frame(owned(&message1), &pmk, &mut aes)).unwrap();
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = Wpa2Gtk::new(2, false, [0x5a; 16]).unwrap();
    let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let message3 = encrypted_message3(&expected_ptk, [7, 6, 5, 4, 3, 2, 1, 0], plain.as_bytes());
    let duplicate = message3.clone();
    let Wpa2StaSupplicantAction::InstallKeys(request) =
        block_on(supplicant.on_frame(message3, &pmk, &mut aes)).unwrap()
    else {
        panic!("Message 3 must produce one key transaction")
    };
    let Wpa2StaSupplicantAction::Transmit(_) = supplicant
        .complete_key_install::<512>(request, true)
        .unwrap()
    else {
        panic!("installed keys must produce Message 4")
    };
    (
        supplicant.into_connected().unwrap(),
        duplicate,
        expected_ptk,
    )
}

fn group_kde(key_id: u8, key: u8) -> [u8; 24] {
    let mut kde = [0; 24];
    kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
    kde[8..].fill(key);
    kde
}

#[test]
fn resolves_m1_through_typed_install_and_authenticated_m4() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let expected_ptk = pmk.derive_ptk(context());
    let mut supplicant = Wpa2StaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let mut aes = Wpa2SoftwareAes::new();

    let message1 = Wpa2TxFrame::<512>::message1(LOCAL, 1, ANONCE).unwrap();
    let Wpa2StaSupplicantAction::Transmit(message2) =
        block_on(supplicant.on_frame(owned(&message1), &pmk, &mut aes)).unwrap()
    else {
        panic!("Message 1 must produce Message 2")
    };
    assert_eq!(message2.key_frame().replay_counter(), 1);
    assert!(message2.key_frame().verify_mic(&expected_ptk));

    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = Wpa2Gtk::new(2, false, [0x5a; 16]).unwrap();
    let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let rsc = [7, 6, 5, 4, 3, 2, 1, 0];
    let message3 = encrypted_message3(&expected_ptk, rsc, plain.as_bytes());
    let Wpa2StaSupplicantAction::InstallKeys(request) =
        block_on(supplicant.on_frame(message3, &pmk, &mut aes)).unwrap()
    else {
        panic!("Message 3 must produce one key transaction")
    };
    assert_eq!(request.replay_counter(), 2);
    assert!(request.encrypted_key_data());
    assert_eq!(request.plain_key_data_len(), plain.as_bytes().len());
    assert_eq!(request.pairwise().peer(), &AP);
    assert_eq!(
        request.pairwise().key().as_bytes(),
        expected_ptk.temporal_key()
    );
    assert_eq!(
        request.group().kind(),
        Wpa2KeyKind::Group {
            key_id: 2,
            transmit: false,
        }
    );
    assert_eq!(request.group().receive_sequence(), &rsc);
    assert_eq!(request.group().key().as_bytes(), &[0x5a; 16]);

    let Wpa2StaSupplicantAction::Transmit(message4) = supplicant
        .complete_key_install::<512>(request, true)
        .unwrap()
    else {
        panic!("installed keys must produce Message 4")
    };
    assert_eq!(supplicant.phase(), Wpa2StaPhase::Completed);
    assert_eq!(message4.key_frame().replay_counter(), 2);
    assert!(message4.key_frame().verify_mic(&expected_ptk));
}

#[test]
fn invalid_message3_mic_is_ignored_and_valid_retry_can_install() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let expected_ptk = pmk.derive_ptk(context());
    let mut supplicant = Wpa2StaSupplicant::try_new(LOCAL, AP, SNONCE, &RSN, &RSN, &[]).unwrap();
    let mut aes = Wpa2SoftwareAes::new();
    let message1 = Wpa2TxFrame::<512>::message1(LOCAL, 1, ANONCE).unwrap();
    block_on(supplicant.on_frame(owned(&message1), &pmk, &mut aes)).unwrap();
    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = Wpa2Gtk::new(1, false, [9; 16]).unwrap();
    let plain = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let message3 = encrypted_message3(&expected_ptk, [0; 8], plain.as_bytes());
    let mut bytes = [0; 512];
    let len = message3.as_bytes().len();
    bytes[..len].copy_from_slice(message3.as_bytes());
    bytes[81] ^= 1;
    let changed =
        crate::OwnedEapolFrame::<512>::try_copy(Wpa2Interface::Station, AP, &bytes[..len]).unwrap();
    assert!(matches!(
        block_on(supplicant.on_frame(changed, &pmk, &mut aes)),
        Err(Wpa2StaProcessError::Supplicant(
            Wpa2StaSupplicantError::InvalidMessage3Mic
        ))
    ));
    assert_eq!(supplicant.phase(), Wpa2StaPhase::AwaitingMessage3);

    let valid_retry = encrypted_message3(&expected_ptk, [0; 8], plain.as_bytes());
    let Wpa2StaSupplicantAction::InstallKeys(request) =
        block_on(supplicant.on_frame(valid_retry, &pmk, &mut aes)).unwrap()
    else {
        panic!("valid M3 retry must retain the original join")
    };
    assert_eq!(request.replay_counter(), 2);
    assert_eq!(supplicant.phase(), Wpa2StaPhase::InstallingKeys);
}

#[test]
fn connected_exact_duplicate_message3_retransmits_authenticated_m4() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let message4 = connected.on_duplicate_message3(duplicate).unwrap();

    assert_eq!(
        message4.key_frame().message(),
        crate::EapolKeyMessage::PairwiseMessage4
    );
    assert_eq!(message4.key_frame().replay_counter(), 2);
    assert!(message4.retransmission());
    assert!(message4.key_frame().verify_mic(&ptk));
}

#[test]
fn connected_duplicate_message3_rejects_changed_protocol_fields() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let wrong_interface = crate::OwnedEapolFrame::<512>::try_copy(
        Wpa2Interface::AccessPoint,
        AP,
        duplicate.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        connected.on_duplicate_message3(wrong_interface),
        Err(Wpa2ConnectedSupplicantError::WrongInterface)
    ));
    let wrong_peer = crate::OwnedEapolFrame::<512>::try_copy(
        Wpa2Interface::Station,
        [9; 6],
        duplicate.as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        connected.on_duplicate_message3(wrong_peer),
        Err(Wpa2ConnectedSupplicantError::WrongPeer)
    ));

    let wrong_protocol = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[0] ^= 1;
    });
    assert!(matches!(
        connected.on_duplicate_message3(wrong_protocol),
        Err(Wpa2ConnectedSupplicantError::ProtocolVersionMismatch)
    ));

    let wrong_descriptor = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        let key_info = u16::from_be_bytes([bytes[5], bytes[6]]);
        bytes[5..7].copy_from_slice(&((key_info & !0x0007) | 1).to_be_bytes());
    });
    assert!(matches!(
        connected.on_duplicate_message3(wrong_descriptor),
        Err(Wpa2ConnectedSupplicantError::UnsupportedDescriptorVersion)
    ));

    let wrong_flags = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        let key_info = u16::from_be_bytes([bytes[5], bytes[6]]) ^ (1 << 9);
        bytes[5..7].copy_from_slice(&key_info.to_be_bytes());
    });
    assert!(matches!(
        connected.on_duplicate_message3(wrong_flags),
        Err(Wpa2ConnectedSupplicantError::UnsupportedMessage)
    ));

    let wrong_replay = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[9..17].copy_from_slice(&3_u64.to_be_bytes());
    });
    assert!(matches!(
        connected.on_duplicate_message3(wrong_replay),
        Err(Wpa2ConnectedSupplicantError::ReplayCounterMismatch)
    ));

    let wrong_anonce = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[17] ^= 1;
    });
    assert!(matches!(
        connected.on_duplicate_message3(wrong_anonce),
        Err(Wpa2ConnectedSupplicantError::AuthenticatorNonceMismatch)
    ));
}

#[test]
fn connected_duplicate_message3_rejects_bad_mic_and_changed_commitment() {
    let (mut connected, duplicate, ptk) = completed_connected();

    let bad_mic = resign_eapol_key_variant(&duplicate, &ptk, false, |bytes| {
        bytes[81] ^= 1;
    });
    assert!(matches!(
        connected.on_duplicate_message3(bad_mic),
        Err(Wpa2ConnectedSupplicantError::InvalidMic)
    ));

    let changed_rsc = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[65] ^= 1;
    });
    assert!(matches!(
        connected.on_duplicate_message3(changed_rsc),
        Err(Wpa2ConnectedSupplicantError::RetainedMessage3Mismatch)
    ));

    let changed_key_data = resign_eapol_key_variant(&duplicate, &ptk, true, |bytes| {
        bytes[99] ^= 1;
    });
    assert!(matches!(
        connected.on_duplicate_message3(changed_key_data),
        Err(Wpa2ConnectedSupplicantError::RetainedMessage3Mismatch)
    ));
}

#[test]
fn connected_group_rekey_installs_once_and_retransmits_idempotently() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(context());
    let frame = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(context()));
    let mut unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let Wpa2ConnectedAction::InstallGroupKey(request) =
        block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    assert_eq!(request.replay_counter(), 3);
    assert_eq!(
        request.group().kind(),
        Wpa2KeyKind::Group {
            key_id: 1,
            transmit: false,
        }
    );
    assert_eq!(request.group().key().as_bytes(), &[0x6a; 16]);
    let response = connected.complete_group_key_install(request, true).unwrap();
    assert_eq!(
        response.key_frame().message(),
        crate::EapolKeyMessage::GroupMessage2
    );
    assert!(response.key_frame().verify_mic(&ptk));
    assert_eq!(connected.replay_counter(), 3);

    let Wpa2ConnectedAction::Retransmit(repeated) =
        block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
    else {
        panic!("repeated Group Message 1 must not reinstall GTK")
    };
    assert!(repeated.key_frame().verify_mic(&ptk));
}

#[test]
fn connected_group_rekey_authenticates_duplicate_before_cached_response() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(context());
    let frame = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(context()));
    let mut unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let Wpa2ConnectedAction::InstallGroupKey(request) =
        block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    connected.complete_group_key_install(request, true).unwrap();

    let mut bytes = [0; 512];
    let length = frame.as_bytes().len();
    bytes[..length].copy_from_slice(frame.as_bytes());
    bytes[81] ^= 1;
    let forged_duplicate =
        crate::OwnedEapolFrame::<512>::try_copy(Wpa2Interface::Station, AP, &bytes[..length])
            .unwrap();
    assert!(matches!(
        block_on(connected.on_group_message1(forged_duplicate, &mut unwrap)),
        Err(Wpa2ConnectedProcessError::Supplicant(
            Wpa2ConnectedSupplicantError::InvalidMic
        ))
    ));
}

#[test]
fn connected_group_rekey_rejects_authenticated_changed_same_replay() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(context());
    let frame = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let original = owned(&frame);
    let mut connected = connected(pmk.derive_ptk(context()));
    let mut unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let Wpa2ConnectedAction::InstallGroupKey(request) =
        block_on(connected.on_group_message1(original.clone(), &mut unwrap)).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    connected.complete_group_key_install(request, true).unwrap();

    let changed_rsc = resign_eapol_key_variant(&original, &ptk, true, |bytes| {
        bytes[65] ^= 1;
    });
    assert!(matches!(
        block_on(connected.on_group_message1(changed_rsc, &mut unwrap)),
        Err(Wpa2ConnectedProcessError::Supplicant(
            Wpa2ConnectedSupplicantError::RetainedGroupMessage1Mismatch
        ))
    ));

    let changed_key_data = resign_eapol_key_variant(&original, &ptk, true, |bytes| {
        bytes[99] ^= 1;
    });
    assert!(matches!(
        block_on(connected.on_group_message1(changed_key_data, &mut unwrap)),
        Err(Wpa2ConnectedProcessError::Supplicant(
            Wpa2ConnectedSupplicantError::RetainedGroupMessage1Mismatch
        ))
    ));
}

#[test]
fn failed_group_rekey_does_not_retain_message1_commitment() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(context());
    let frame = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [9; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut connected = connected(pmk.derive_ptk(context()));
    let mut unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    let Wpa2ConnectedAction::InstallGroupKey(request) =
        block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
    else {
        panic!("new Group Message 1 must request one GTK replacement")
    };
    assert_eq!(
        connected.complete_group_key_install(request, false),
        Err(Wpa2ConnectedSupplicantError::InstallFailed)
    );

    let Wpa2ConnectedAction::InstallGroupKey(retry) =
        block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap()
    else {
        panic!("failed publication must leave the same frame eligible for retry")
    };
    connected.complete_group_key_install(retry, true).unwrap();
    assert!(matches!(
        block_on(connected.on_group_message1(owned(&frame), &mut unwrap)).unwrap(),
        Wpa2ConnectedAction::Retransmit(_)
    ));
}

#[test]
fn connected_group_rekey_rejects_bad_mic_and_stale_replay() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk = pmk.derive_ptk(context());
    let valid = Wpa2TxFrame::<512>::group_message1(LOCAL, 3, [0; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    let mut bytes = [0; 512];
    bytes[..valid.as_bytes().len()].copy_from_slice(valid.as_bytes());
    bytes[81] ^= 1;
    let invalid = crate::OwnedEapolFrame::<512>::try_copy(
        Wpa2Interface::Station,
        AP,
        &bytes[..valid.as_bytes().len()],
    )
    .unwrap();
    let mut connected = connected(pmk.derive_ptk(context()));
    let mut unwrap = GroupKeyUnwrap {
        plain: group_kde(1, 0x6a),
    };
    assert!(matches!(
        block_on(connected.on_group_message1(invalid, &mut unwrap)),
        Err(Wpa2ConnectedProcessError::Supplicant(
            Wpa2ConnectedSupplicantError::InvalidMic
        ))
    ));

    let stale = Wpa2TxFrame::<512>::group_message1(LOCAL, 1, [0; 8], &[0x55; 24])
        .unwrap()
        .authenticate(&ptk);
    assert!(matches!(
        block_on(connected.on_group_message1(owned(&stale), &mut unwrap)),
        Err(Wpa2ConnectedProcessError::Supplicant(
            Wpa2ConnectedSupplicantError::StaleReplayCounter
        ))
    ));
}

#[test]
fn response_deadlines_preserve_total_wait_without_spontaneous_m2_retry() {
    let mut message1 = Wpa2StaResponseDeadline::new(Wpa2StaResponseWait::Message1);
    for elapsed in 1..WPA2_STA_MESSAGE1_TIMEOUT_MS {
        assert_eq!(message1.finish_millisecond(), Wpa2StaDeadlineEvent::Pending);
        assert_eq!(message1.elapsed_ms(), elapsed);
    }
    assert_eq!(
        message1.finish_millisecond(),
        Wpa2StaDeadlineEvent::Expired {
            wait: Wpa2StaResponseWait::Message1,
            elapsed_ms: WPA2_STA_MESSAGE1_TIMEOUT_MS,
        }
    );

    let mut message3 = Wpa2StaResponseDeadline::new(Wpa2StaResponseWait::Message3);
    for _ in 1..WPA2_STA_MESSAGE3_TIMEOUT_MS {
        assert_eq!(message3.finish_millisecond(), Wpa2StaDeadlineEvent::Pending);
    }
    assert_eq!(
        message3.finish_millisecond(),
        Wpa2StaDeadlineEvent::Expired {
            wait: Wpa2StaResponseWait::Message3,
            elapsed_ms: WPA2_STA_MESSAGE3_TIMEOUT_MS,
        }
    );
}
