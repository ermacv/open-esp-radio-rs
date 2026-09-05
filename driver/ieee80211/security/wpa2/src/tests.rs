use super::*;

fn key_packet(key_info: u16, replay_counter: u64) -> [u8; EAPOL_KEY_PACKET_LEN] {
    let mut packet = [0; EAPOL_KEY_PACKET_LEN];
    packet[0] = 2;
    packet[1] = EAPOL_PACKET_TYPE_KEY;
    packet[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
    packet[4] = RSN_KEY_DESCRIPTOR_TYPE;
    packet[5..7].copy_from_slice(&key_info.to_be_bytes());
    packet[9..17].copy_from_slice(&replay_counter.to_be_bytes());
    packet
}

#[test]
fn parses_and_classifies_pairwise_messages() {
    let message1 = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 7);
    let parsed = EapolKeyFrame::parse(&message1).unwrap();
    assert_eq!(parsed.message(), EapolKeyMessage::PairwiseMessage1);
    assert_eq!(parsed.key_info().descriptor_version(), 2);
    assert_eq!(parsed.replay_counter(), 7);

    let message3 = key_packet(
        KEY_INFO_PAIRWISE | KEY_INFO_ACK | KEY_INFO_MIC | KEY_INFO_INSTALL | KEY_INFO_SECURE | 2,
        8,
    );
    assert_eq!(
        EapolKeyFrame::parse(&message3).unwrap().message(),
        EapolKeyMessage::PairwiseMessage3
    );
}

#[test]
fn rejects_ambiguous_lengths_and_non_rsn_descriptors() {
    let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 1);
    packet[2..4].copy_from_slice(&0_u16.to_be_bytes());
    assert_eq!(
        EapolKeyFrame::parse(&packet),
        Err(EapolParseError::LengthMismatch)
    );

    let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 1);
    packet[4] = 254;
    assert_eq!(
        EapolKeyFrame::parse(&packet),
        Err(EapolParseError::NotRsnKeyDescriptor)
    );
}

#[test]
fn derives_known_ieee_pmk() {
    let pmk = Pmk::derive(b"password", b"IEEE").unwrap();
    assert_eq!(
        pmk.0,
        [
            0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a,
            0x5f, 0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e,
            0x97, 0x10, 0xa1, 0x2e,
        ]
    );
}

#[test]
fn association_security_binding_commits_to_every_byte_and_the_pmk() {
    let pmk = Pmk::from_bytes([0x11; 32]);
    let other_pmk = Pmk::from_bytes([0x22; 32]);
    let association_security_ies = [0x30, 2, 1, 0, 0xf4, 1, 0x20];
    let binding = pmk.bind_association_security_ies(&association_security_ies);

    assert!(binding.matches(&pmk, &association_security_ies));
    let mut changed = association_security_ies;
    changed[6] ^= 1;
    assert!(!binding.matches(&pmk, &changed));
    assert!(!binding.matches(&other_pmk, &association_security_ies));
}

#[test]
fn derives_known_ptk_and_builds_message_2() {
    let pmk = Pmk(core::array::from_fn(|index| index as u8));
    let context = PtkContext {
        authenticator_address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        supplicant_address: [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb],
        authenticator_nonce: core::array::from_fn(|index| index as u8 + 32),
        supplicant_nonce: core::array::from_fn(|index| index as u8 + 64),
    };
    let ptk = pmk.derive_ptk(context);
    assert_eq!(
        ptk.0,
        [
            0x35, 0x07, 0x82, 0xa5, 0x49, 0x8c, 0x27, 0x32, 0x15, 0xbf, 0x37, 0x70, 0x79, 0xe3,
            0x65, 0x0f, 0x63, 0x13, 0xd9, 0x26, 0xdb, 0xe9, 0xed, 0x87, 0x53, 0xa6, 0x0f, 0x1b,
            0x6e, 0x62, 0x25, 0xea, 0x5c, 0xbe, 0xca, 0x83, 0xd7, 0xbb, 0xa7, 0x6c, 0x9e, 0x6d,
            0x02, 0xa8, 0x48, 0xd1, 0xe5, 0x5f,
        ]
    );
    let rsn = [
        0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ];
    let security_ies = frames::OwnedAssociationSecurityIes::<128>::try_copy_bytes(&rsn).unwrap();
    let message = frames::Wpa2TxFrame::<512>::message2_with_security_ies(
        context.authenticator_address,
        7,
        context.supplicant_nonce,
        &security_ies,
    )
    .unwrap()
    .authenticate(&ptk);
    let parsed = EapolKeyFrame::parse(message.as_bytes()).unwrap();
    assert_eq!(parsed.message(), EapolKeyMessage::PairwiseMessage2);
    assert_eq!(parsed.replay_counter(), 7);
    assert_eq!(parsed.nonce(), &context.supplicant_nonce);
    assert_ne!(parsed.mic(), &[0; 16]);
    assert_eq!(parsed.key_data(), &rsn);

    let message4 = frames::Wpa2TxFrame::<512>::message4(context.authenticator_address, 8)
        .unwrap()
        .authenticate(&ptk);
    let parsed4 = EapolKeyFrame::parse(message4.as_bytes()).unwrap();
    assert_eq!(parsed4.protocol_version(), 1);
    assert_eq!(parsed4.message(), EapolKeyMessage::PairwiseMessage4);
    assert_eq!(parsed4.replay_counter(), 8);
    assert!(parsed4.verify_mic(&ptk));

    let mut changed = [0; EAPOL_KEY_PACKET_LEN];
    changed.copy_from_slice(message4.as_bytes());
    changed[17] ^= 1;
    assert!(!EapolKeyFrame::parse(&changed).unwrap().verify_mic(&ptk));
}
