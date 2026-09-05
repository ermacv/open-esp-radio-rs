use super::*;
use crate::EapolKeyMessage;
use crate::state::Wpa2ApAction;

fn rsn_ie() -> OwnedRsnIe<22> {
    let mut bytes = [0; 22];
    bytes[0] = RSN_ELEMENT_ID;
    bytes[1] = 20;
    OwnedRsnIe::try_copy(&bytes).unwrap()
}

fn association_ies() -> OwnedAssociationSecurityIes<22> {
    OwnedAssociationSecurityIes::try_copy(&rsn_ie(), &[]).unwrap()
}

#[test]
fn contiguous_association_security_ies_retain_exact_rsn_and_rsnxe() {
    let rsn = rsn_ie();
    let rsnxe = [RSNXE_ELEMENT_ID, 2, 0x20, 0x00];
    let mut bytes = [0_u8; 26];
    bytes[..22].copy_from_slice(rsn.as_bytes());
    bytes[22..].copy_from_slice(&rsnxe);
    let owned = OwnedAssociationSecurityIes::<128>::try_copy_bytes(&bytes).unwrap();
    assert_eq!(owned.as_bytes(), &bytes);
    assert_eq!(owned.rsn_ie(), rsn.as_bytes());

    bytes[23] = 3;
    assert_eq!(
        OwnedAssociationSecurityIes::<128>::try_copy_bytes(&bytes),
        Err(Wpa2FrameError::InvalidRsnxe)
    );
}

#[test]
fn gtk_key_data_round_trips_with_key_wrap_padding() {
    let rsn = rsn_ie();
    let gtk = Wpa2Gtk::new(2, false, [0x5a; 16]).unwrap();
    let data = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    assert_eq!(data.as_bytes().len() % 8, 0);
    let parsed = parse_gtk_key_data(data.as_bytes(), rsn.as_bytes(), &[]).unwrap();
    assert_eq!(parsed.key_id(), 2);
    assert!(!parsed.transmit());
    assert_eq!(parsed.key(), &[0x5a; 16]);
}

#[test]
fn builders_produce_classified_m1_to_m4() {
    let rsn = rsn_ie();
    let m1 = Wpa2TxFrame::<128>::message1([2; 6], 7, [3; 32]).unwrap();
    let m2 = Wpa2TxFrame::<128>::message2([1; 6], 7, [4; 32], &rsn).unwrap();
    let m3 = Wpa2TxFrame::<128>::message3([2; 6], 8, [3; 32], [5; 8], &[6; 24]).unwrap();
    let m4 = Wpa2TxFrame::<128>::message4([1; 6], 8).unwrap();
    assert_eq!(m1.key_frame().message(), EapolKeyMessage::PairwiseMessage1);
    assert_eq!(m2.key_frame().message(), EapolKeyMessage::PairwiseMessage2);
    assert_eq!(m3.key_frame().message(), EapolKeyMessage::PairwiseMessage3);
    assert!(m3.key_frame().key_info().encrypted_key_data());
    assert_eq!(m4.key_frame().message(), EapolKeyMessage::PairwiseMessage4);
    assert_eq!(m1.key_frame().key_length(), WPA2_GTK_LEN as u16);
    assert_eq!(m2.key_frame().key_length(), 0);
    assert_eq!(m3.key_frame().key_length(), WPA2_GTK_LEN as u16);
    assert_eq!(m4.key_frame().key_length(), 0);
    assert_eq!(m1.key_frame().protocol_version(), 2);
    assert_eq!(m2.key_frame().protocol_version(), 1);
    assert_eq!(m3.key_frame().protocol_version(), 2);
    assert_eq!(m4.key_frame().protocol_version(), 1);
}

#[test]
fn state_actions_are_bound_to_role_peer_and_nonce_context() {
    let security_ies = association_ies();
    let sta = Wpa2StaState::new([1; 6], [2; 6], [3; 32]).unwrap();
    let m2_action = Wpa2Transmit {
        message: Wpa2TxMessage::PairwiseMessage2,
        replay_counter: 7,
        retransmission: false,
    };
    let m2 = build_sta_action_frame::<128, _>(&sta, m2_action, &security_ies).unwrap();
    assert_eq!(m2.peer(), &[2; 6]);
    assert_eq!(m2.key_frame().nonce(), &[3; 32]);
    assert_eq!(
        build_ap_action_frame::<128>(
            &Wpa2ApState::new([2; 6], [1; 6], [4; 32], 7).unwrap(),
            m2_action,
            [0; 8],
            &[]
        )
        .err(),
        Some(Wpa2FrameError::UnexpectedTransmitAction)
    );

    let ap = Wpa2ApState::new([2; 6], [1; 6], [4; 32], 7).unwrap();
    let Wpa2ApAction::Transmit(m1_action) = ap.message1(false).unwrap() else {
        panic!("message1 must produce a transmit action")
    };
    let m1 = build_ap_action_frame::<128>(&ap, m1_action, [0; 8], &[]).unwrap();
    assert_eq!(m1.peer(), &[1; 6]);
    assert_eq!(m1.key_frame().nonce(), &[4; 32]);
}

#[test]
fn parser_rejects_changed_rsn_ie_and_duplicate_gtk() {
    let rsn = rsn_ie();
    let gtk = Wpa2Gtk::new(1, false, [7; 16]).unwrap();
    let data = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let mut other = [0; 22];
    other[0] = RSN_ELEMENT_ID;
    other[1] = 20;
    other[2] = 1;
    assert_eq!(
        parse_gtk_key_data(data.as_bytes(), &other, &[]).err(),
        Some(Wpa2FrameError::RsnIeMismatch)
    );

    let mut duplicate = [0; 70];
    let source = data.as_bytes();
    duplicate[..46].copy_from_slice(&source[..46]);
    duplicate[46..70].copy_from_slice(&source[22..46]);
    assert_eq!(
        parse_gtk_key_data(&duplicate, rsn.as_bytes(), &[]).err(),
        Some(Wpa2FrameError::DuplicateGtk)
    );
}

#[test]
fn parser_validates_authenticator_rsnxe_without_ignoring_unknown_elements() {
    let rsn = rsn_ie();
    let gtk = Wpa2Gtk::new(1, false, [7; 16]).unwrap();
    let data = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
    let rsnxe = [RSNXE_ELEMENT_ID, 2, 0x20, 0x00];
    let mut with_rsnxe = [0; 64];
    let source = data.as_bytes();
    with_rsnxe[..22].copy_from_slice(&source[..22]);
    with_rsnxe[22..26].copy_from_slice(&rsnxe);
    with_rsnxe[26..50].copy_from_slice(&source[22..46]);
    with_rsnxe[50] = VENDOR_ELEMENT_ID;

    let parsed = parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &rsnxe).unwrap();
    assert_eq!(parsed.key_id(), 1);
    assert_eq!(parsed.key(), &[7; 16]);

    assert_eq!(
        parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &[]).err(),
        Some(Wpa2FrameError::UnexpectedRsnxe)
    );
    assert_eq!(
        parse_gtk_key_data(source, rsn.as_bytes(), &rsnxe).err(),
        Some(Wpa2FrameError::MissingRsnxe)
    );

    let changed = [RSNXE_ELEMENT_ID, 2, 0x21, 0x00];
    assert_eq!(
        parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &changed).err(),
        Some(Wpa2FrameError::RsnxeMismatch)
    );
}
