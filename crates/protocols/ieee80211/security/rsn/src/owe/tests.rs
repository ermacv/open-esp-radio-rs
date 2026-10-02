use super::*;
use oer_ieee80211_mac::{
    management::elements::Elements, security::rsn::RsnElement, ssid::WifiSsid,
};
mod association;

// Independent wire fixture: OWE/CCMP, MFPC/MFPR and the default BIP cipher.
const OWE_RSN: &[u8] = &[
    48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 18, 192, 0,
];
fn profile(bytes: &[u8]) -> SecurityProfile<'_> {
    SecurityProfile::from_elements(Elements::parse(bytes).unwrap()).unwrap()
}
fn fixture_key(group: Group) -> OwePmk {
    let addresses = Addresses {
        station: [2; 6],
        access_point: [4; 6],
    };
    let (_, client, ap, _, _, _) = vectors()
        .into_iter()
        .find(|value| value.0 == group)
        .unwrap();
    let c = hex(client);
    let a = hex(ap);
    OwePmk::derive(group, addresses, &c, &a, &a).unwrap()
}
fn scope() -> CacheScope {
    CacheScope {
        addresses: Addresses {
            station: [2; 6],
            access_point: [4; 6],
        },
        ssid: WifiSsid::new(b"owe\0test").unwrap(),
        management_protection: true,
    }
}

fn hex(value: &str) -> std::vec::Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = core::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}

fn scalar(group: Group, value: u8) -> std::vec::Vec<u8> {
    let mut bytes = std::vec![0; group.coordinate_len()];
    *bytes.last_mut().unwrap() = value;
    bytes
}

#[test]
fn checked_dh_matches_independent_vectors_for_both_roles_and_every_group() {
    for (group, client, ap, pmk, pmkid, _) in vectors() {
        let station = KeyPair::from_scalar(group, &scalar(group, 1)).unwrap();
        let access_point = KeyPair::from_scalar(group, &scalar(group, 2)).unwrap();
        assert_eq!(station.group(), group);
        assert_eq!(station.parameter().public_key, hex(client));
        assert_eq!(access_point.parameter().public_key, hex(ap));
        let client = hex(client);
        let peer = access_point.parameter();
        let station_key = station
            .finish(crate::RsnInterface::Station, scope().addresses, peer)
            .unwrap();
        let ap_key = access_point
            .finish(
                crate::RsnInterface::AccessPoint,
                scope().addresses,
                oer_ieee80211_mac::owe::DhParameter {
                    group: group.number(),
                    public_key: &client,
                },
            )
            .unwrap();
        assert_eq!(station_key.test_bytes(), hex(pmk));
        assert_eq!(ap_key.test_bytes(), station_key.test_bytes());
        assert_eq!(station_key.pmkid(), hex(pmkid).as_slice());
        assert_eq!(ap_key.pmkid(), station_key.pmkid());
    }
}

#[test]
fn checked_dh_rejects_invalid_scalars_points_and_peer_context() {
    use oer_ieee80211_mac::owe::DhParameter;
    for (group, _, ap, _, _, _) in vectors() {
        assert!(matches!(
            KeyPair::from_scalar(group, &scalar(group, 0)),
            Err(Error::InvalidKey)
        ));
        assert!(matches!(
            KeyPair::from_scalar(group, &std::vec![255; group.coordinate_len()]),
            Err(Error::InvalidKey)
        ));
        let bytes = scalar(group, 1);
        assert!(matches!(
            KeyPair::from_scalar(group, &bytes[1..]),
            Err(Error::InvalidKey)
        ));
        // OpenSSL independently rejects x=1 for P256/P384, x=3 for P521.
        let off_curve = scalar(group, if group == Group::P521 { 3 } else { 1 });
        for public in [&off_curve[..], &std::vec![255; group.coordinate_len()][..]] {
            let key = KeyPair::from_scalar(group, &bytes).unwrap();
            assert!(matches!(
                key.finish(
                    crate::RsnInterface::Station,
                    scope().addresses,
                    DhParameter {
                        group: group.number(),
                        public_key: public
                    }
                ),
                Err(Error::InvalidKey)
            ));
        }
        let ap = hex(ap);
        let key = KeyPair::from_scalar(group, &bytes).unwrap();
        let other = if group == Group::P256 {
            Group::P384
        } else {
            Group::P256
        };
        assert!(
            key.finish(
                crate::RsnInterface::Station,
                scope().addresses,
                DhParameter {
                    group: other.number(),
                    public_key: &scalar(other, 0)
                }
            )
            .is_err()
        );
        let key = KeyPair::from_scalar(group, &bytes).unwrap();
        assert!(
            key.finish(
                crate::RsnInterface::Station,
                scope().addresses,
                DhParameter {
                    group: group.number(),
                    public_key: &ap[1..]
                }
            )
            .is_err()
        );
        let key = KeyPair::from_scalar(group, &bytes).unwrap();
        let mut addresses = scope().addresses;
        addresses.access_point = addresses.station;
        assert!(matches!(
            key.finish(
                crate::RsnInterface::Station,
                addresses,
                DhParameter {
                    group: group.number(),
                    public_key: &ap
                }
            ),
            Err(Error::WrongContext)
        ));
    }
}

// Independently generated using OpenSSL's EC backend (cryptography): private
// scalars 1 and 2, RFC 8110 HKDF, IEEE KDF-HMAC with the fixed context below.
fn vectors() -> [(
    Group,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
); 3] {
    [
        (
            Group::P256,
            "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
            "7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978",
            "fdaf512f1bda44abda8b4d784bc560d1fcbb747f8f15a452639233b698ea74c7",
            "9b4d4b535cf933f43a52210f510121ee",
            "0f5a85f0537b1e1c9a8e008f4e3258d6a0e59555320b63885c62b5db91dc0c9c5a7231eed6528a6d02c9fc1a16c47400",
        ),
        (
            Group::P384,
            "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
            "08d999057ba3d2d969260045c55b97f089025959a6f434d651d207d19fb96e9e4fe0e86ebe0e64f85b96a9c75295df61",
            "4dbfee2ad69d2bafb7cd1cb4dd46bec60a25ccdf343f1be935076baf970d9bbca539b0c92c1c997606aa13633c0d9bac",
            "1012f389475f37740a961ea224959ce7",
            "fe8ad4290ad553913a00af5a5f486d2c00f3a41753a53eee9f53f6addc96a2117ea3bf2a228d2c98965809b3bb104eb270277c4c81f81a16e4751a8d1024577ea0d8fce122acc071",
        ),
        (
            Group::P521,
            "00c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe75928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66",
            "00433c219024277e7e682fcb288148c282747403279b1ccc06352c6e5505d769be97b3b204da6ef55507aa104a3a35c5af41cf2fa364d60fd967f43e3933ba6d783d",
            "0b3fe768f7772e298a1dd134b8e4af9f7d4b42df5d83bc2824c5c8b8fae9dad02c5dc416a1cc63ef674f60b3dbd8d7eac4eb3b1c0cdb8eb6d19948523442f343",
            "7007b09f8bb89974606c10e04cd5662b",
            "2ac729ed0fefe873af6b0734981366a3eac803131febbbf756b90081d32089070b408996ab04ab5245b0a0a9d589d186c2f0c244f8157656da438bb5b6145775273430e53e506b059fb7dccd7e1e2867",
        ),
    ]
}

#[test]
fn pmk_pmkid_and_full_ptk_match_independent_vectors_for_every_group() {
    let addresses = Addresses {
        station: [2; 6],
        access_point: [4; 6],
    };
    for (group, client, ap, pmk, pmkid, ptk) in vectors() {
        let c = hex(client);
        let a = hex(ap);
        let key = OwePmk::derive(group, addresses, &c, &a, &a).unwrap();
        assert_eq!(key.test_bytes(), hex(pmk));
        assert_eq!(key.pmkid(), hex(pmkid).as_slice());
        let key = key
            .derive_ptk(crate::PtkContext {
                authenticator_address: addresses.access_point,
                supplicant_address: addresses.station,
                authenticator_nonce: [1; 32],
                supplicant_nonce: [2; 32],
            })
            .unwrap();
        let expected = hex(ptk);
        assert_eq!(key.test_bytes(), expected);
        let data_key_start = expected.len() - oer_ieee80211_mac::security::CCMP_128_KEY_LEN;
        assert_eq!(key.temporal_key(), &expected[data_key_start..]);
    }
}

#[test]
fn owe_mics_and_wrapped_data_match_independent_hmac_and_openssl_vectors() {
    // Python stdlib HMAC over independently serialized EAPOL Message 4;
    // OpenSSL-backed cryptography AES key wrap over octets 00..0f. The PTK
    // fixtures above supply KCK/KEK with their normative group-specific widths.
    let vectors = [
        (
            Group::P256,
            "d61b4ea976d3c6b1d87e41a5fe0354c4",
            "e34b9ae34801db6d8f0629d5b07b840b65e533c049c39d6d",
        ),
        (
            Group::P384,
            "4c103c8301f4bf603f2717f637b037fd84fe3a2ee06427ec",
            "5427aa4d7cb3732ee1015965da1e50166051554b61b27aec",
        ),
        (
            Group::P521,
            "73f02674e60d219e714809df8e1aa5919782175b56538bcae4c0a8dce09e0537",
            "66f70029f4e614024807ef6d103ed185ba6de8d86b1154e9",
        ),
    ];
    let context = crate::PtkContext {
        authenticator_address: scope().addresses.access_point,
        supplicant_address: scope().addresses.station,
        authenticator_nonce: [1; 32],
        supplicant_nonce: [2; 32],
    };
    let plaintext: [u8; 16] = core::array::from_fn(|index| index as u8);
    for (group, mic, wrapped) in vectors {
        let ptk = fixture_key(group).derive_ptk(context).unwrap();
        let frame =
            crate::frames::RsnTxFrame::<512>::message4(group, scope().addresses.access_point, 11)
                .unwrap()
                .authenticate_owe(&ptk)
                .unwrap();
        assert_eq!(frame.key_frame().mic(), hex(mic));
        assert_eq!(
            ptk.wrap_key_data(&plaintext).unwrap().as_bytes(),
            hex(wrapped)
        );
        let mut tampered = frame.as_bytes().to_vec();
        // Replay counter mutation keeps the packet syntactically valid.
        tampered[16] ^= 1;
        assert_eq!(ptk.verify_eapol(&tampered), Err(Error::InvalidMic));
        let mut invalid = frame.as_bytes().to_vec();
        invalid[6] |= 1; // Descriptor version 1 cannot authenticate as OWE.
        let before = invalid.clone();
        assert_eq!(
            ptk.authenticate_eapol(&mut invalid),
            Err(Error::WrongContext)
        );
        assert_eq!(invalid, before);
    }
}

#[test]
fn a_valid_zero_x_shared_point_is_not_confused_with_the_point_at_infinity() {
    // OpenSSL accepts x=0 on all three NIST curves. Multiplication by scalar
    // 1 gives a non-infinite point whose shared x coordinate is all zero.
    // These HKDF results were computed independently with Python HMAC.
    let expected = [
        "934a61c82a7173bdd07926b383e7f5f4a192b4f4b08cb7c1fc2d3665a5f86d6c",
        "b72edc092ab312a50a707ad780819ac1eb14e057f50701506d9d9fcde3aefcabd28955001784e1cac6411f5d6d0d244a",
        "3b0b67318cf8a3565d1edc84fa5e4ae6b9f430d7d4160ff2ba5beb612e24e4d8ff4bdd210053f76f1e2fc1cf3a47fe9e16602d16031c3fbe24a8fa7be23aa4fb",
    ];
    for ((group, _, _, _, _, _), expected) in vectors().into_iter().zip(expected) {
        let zero = [0; 66];
        let zero = &zero[..group.coordinate_len()];
        let key = KeyPair::from_scalar(group, &scalar(group, 1))
            .unwrap()
            .finish(
                crate::RsnInterface::Station,
                scope().addresses,
                oer_ieee80211_mac::owe::DhParameter {
                    group: group.number(),
                    public_key: zero,
                },
            )
            .unwrap();
        assert_eq!(key.test_bytes(), hex(expected));
    }
}

#[test]
fn every_owe_group_completes_the_shared_four_way_with_correct_geometry() {
    use crate::{HandshakeSuite, OwnedEapolFrame, RsnInterface, frames::RsnTxFrame, state::*};
    let addresses = Addresses {
        station: [2; 6],
        access_point: [4; 6],
    };
    let own = |group: Group, role, peer, frame: &RsnTxFrame<512>| {
        OwnedEapolFrame::<512>::try_copy_with_mic_length(
            role,
            peer,
            frame.as_bytes(),
            group.eapol_mic_length(),
        )
        .unwrap()
    };
    for (group, _, _, _, _, _) in vectors() {
        let mut dh_sta = Station::<512>::new(
            association::context(),
            profile(OWE_RSN),
            association::groups(group),
            None,
            0,
        )
        .unwrap();
        dh_sta
            .provide_key(KeyPair::from_scalar(group, &scalar(group, 1)).unwrap(), 1)
            .unwrap();
        let request_ticket = dh_sta.transmission().unwrap().ticket;
        let request = dh_sta.transmission().unwrap().bytes.to_vec();
        dh_sta.admitted(request_ticket, 2).unwrap();
        let mut dh_ap = AccessPoint::<512>::new(
            association::context(),
            Elements::parse(&request).unwrap(),
            association::groups(group),
            None,
            3,
        )
        .unwrap();
        dh_ap
            .provide_key(KeyPair::from_scalar(group, &scalar(group, 2)).unwrap(), 3)
            .unwrap();
        let response_ticket = dh_ap.transmission().unwrap().ticket;
        let response = dh_ap.transmission().unwrap().bytes.to_vec();
        dh_ap.admitted(response_ticket, 4).unwrap();
        let association = dh_sta
            .response(
                request_ticket,
                oer_ieee80211_mac::management::STATUS_SUCCESS,
                Elements::parse(&response).unwrap(),
                5,
            )
            .unwrap()
            .unwrap();
        let ap_association = dh_ap
            .tx_completed(response_ticket, true, 6)
            .unwrap()
            .unwrap();
        let mut sta =
            RsnStaState::new(group, addresses.station, addresses.access_point, [2; 32]).unwrap();
        let mut ap = RsnApState::new(
            group,
            addresses.access_point,
            addresses.station,
            [1; 32],
            10,
        )
        .unwrap();
        let m1 = RsnTxFrame::<512>::message1(group, addresses.station, 10, [1; 32]).unwrap();
        let (ticket, context) = match sta
            .on_frame(own(
                group,
                RsnInterface::Station,
                addresses.access_point,
                &m1,
            ))
            .unwrap()
        {
            RsnStaAction::DerivePtk { ticket, context } => (ticket, context),
            other => panic!("{other:?}"),
        };
        let sta_ptk = association
            .derive_ptk(crate::PtkContext {
                authenticator_address: context.authenticator_address,
                supplicant_address: context.supplicant_address,
                authenticator_nonce: context.authenticator_nonce,
                supplicant_nonce: context.supplicant_nonce,
            })
            .unwrap();
        sta.complete_ptk::<512>(ticket, true).unwrap();
        let m2 = association.message2::<512>(10, &sta_ptk).unwrap();
        let (ticket, context, m2) = match ap
            .on_frame(own(
                group,
                RsnInterface::AccessPoint,
                addresses.station,
                &m2,
            ))
            .unwrap()
        {
            RsnApAction::DerivePtk {
                ticket,
                context,
                message2,
            } => (ticket, context, message2),
            other => panic!("{other:?}"),
        };
        let ap_ptk = ap_association
            .derive_ptk(crate::PtkContext {
                authenticator_address: context.authenticator_address,
                supplicant_address: context.supplicant_address,
                authenticator_nonce: context.authenticator_nonce,
                supplicant_nonce: context.supplicant_nonce,
            })
            .unwrap();
        let (ticket, m2) = match ap.complete_ptk(ticket, m2, true).unwrap() {
            RsnApAction::VerifyMessage2Mic { ticket, message2 } => (ticket, message2),
            other => panic!("{other:?}"),
        };
        ap_ptk.verify_eapol(m2.as_bytes()).unwrap();
        ap_association
            .validate_message2_key_data(m2.key_frame().key_data())
            .unwrap();
        let ticket = match ap.complete_message2_mic(ticket, m2, true).unwrap() {
            RsnApAction::PrepareMessage3 { ticket } => ticket,
            other => panic!("{other:?}"),
        };
        let groups = crate::frames::RsnGroupKeys {
            gtk: crate::frames::RsnGtk::new(1, false, [0x55; 16]).unwrap(),
            igtk: Some(crate::frames::RsnIgtk::new(4, [7; 6], [0xaa; 16]).unwrap()),
        };
        let plaintext = ap_association.message3_key_data::<512>(&groups).unwrap();
        let wrapped = ap_ptk.wrap_key_data(plaintext.as_bytes()).unwrap();
        ap.complete_message3_preparation::<512>(ticket, true)
            .unwrap();
        let m3 = RsnTxFrame::<512>::message3(
            group,
            addresses.station,
            11,
            [1; 32],
            [0; 8],
            wrapped.as_bytes(),
        )
        .unwrap()
        .authenticate_owe(&ap_ptk)
        .unwrap();
        let retained = own(group, RsnInterface::Station, addresses.access_point, &m3);
        let (ticket, m3) = match sta.on_frame(retained.clone()).unwrap() {
            RsnStaAction::VerifyMessage3Mic { ticket, frame } => (ticket, frame),
            other => panic!("{other:?}"),
        };
        sta_ptk.verify_eapol(m3.as_bytes()).unwrap();
        let (ticket, m3) = match sta.complete_message3_mic(ticket, m3, true).unwrap() {
            RsnStaAction::DecryptMessage3KeyData { ticket, frame } => (ticket, frame),
            other => panic!("{other:?}"),
        };
        let plaintext = sta_ptk.unwrap_key_data(m3.key_frame().key_data()).unwrap();
        let installed = association
            .parse_message3_key_data(plaintext.as_bytes())
            .unwrap();
        assert_eq!(installed.gtk.key(), groups.gtk.key());
        assert_eq!(
            installed.igtk.unwrap().key(),
            groups.igtk.as_ref().unwrap().key()
        );
        let ticket = match sta.complete_key_data(ticket, m3, true).unwrap() {
            RsnStaAction::InstallKeys { ticket, .. } => ticket,
            other => panic!("{other:?}"),
        };
        sta.complete_key_install::<512>(ticket, true).unwrap();
        let m4 = RsnTxFrame::<512>::message4(group, addresses.access_point, 11)
            .unwrap()
            .authenticate_owe(&sta_ptk)
            .unwrap();
        let (ticket, m4) = match ap
            .on_frame(own(
                group,
                RsnInterface::AccessPoint,
                addresses.station,
                &m4,
            ))
            .unwrap()
        {
            RsnApAction::VerifyMessage4Mic { ticket, message4 } => (ticket, message4),
            other => panic!("{other:?}"),
        };
        ap_ptk.verify_eapol(m4.as_bytes()).unwrap();
        assert_eq!(
            ap.complete_message4_mic(ticket, m4, true).unwrap(),
            RsnApAction::AuthorizePeer
        );
        assert!(matches!(
            sta.on_frame(retained).unwrap(),
            RsnStaAction::Transmit(RsnTransmit {
                retransmission: true,
                ..
            })
        ));
        assert_eq!(sta.phase(), RsnStaPhase::Completed);
        assert_eq!(ap.phase(), RsnApPhase::Authorized);
        let cached = association.cache_station(&sta, 20, 100).unwrap();
        assert_eq!(cached.scope(), scope());
        let ap_cached = ap_association.cache_access_point(&ap, 20, 100).unwrap();
        assert_eq!(ap_cached.pmkid(), cached.pmkid());
        let legacy = crate::Pmk::from_bytes([1; 32]).derive_ptk(
            crate::Akm::Sae,
            crate::PtkContext {
                authenticator_address: addresses.access_point,
                supplicant_address: addresses.station,
                authenticator_nonce: [1; 32],
                supplicant_nonce: [2; 32],
            },
        );
        assert_eq!(
            RsnTxFrame::<512>::message4(group, addresses.access_point, 11)
                .unwrap()
                .authenticate(&legacy)
                .unwrap_err(),
            crate::frames::RsnFrameError::WrongCryptoSuite
        );
    }
}

#[test]
fn all_groups_keep_full_pmk_and_ptk_geometry_and_reject_cross_peer_use() {
    let addresses = Addresses {
        station: [2; 6],
        access_point: [4; 6],
    };
    let public = [1; 66];
    let secret = [2; 66];
    for group in [Group::P256, Group::P384, Group::P521] {
        let coordinate = group.coordinate_len();
        let key = OwePmk::derive(
            group,
            addresses,
            &public[..coordinate],
            &public[..coordinate],
            &secret[..coordinate],
        )
        .unwrap();
        assert_eq!(
            key.test_bytes().len(),
            crate::akm::owe_key_geometry(group).pmk_len()
        );
        let context = crate::PtkContext {
            authenticator_address: addresses.access_point,
            supplicant_address: addresses.station,
            authenticator_nonce: [1; 32],
            supplicant_nonce: [2; 32],
        };
        let ptk = key.derive_ptk(context).unwrap();
        let wrapped = ptk.wrap_key_data(&[0x55; 24]).unwrap();
        assert_eq!(
            ptk.unwrap_key_data(wrapped.as_bytes()).unwrap().as_bytes(),
            &[0x55; 24]
        );
        let changed = crate::PtkContext {
            supplicant_address: [6; 6],
            ..context
        };
        assert!(key.derive_ptk(changed).is_err());
        let changed = crate::PtkContext {
            supplicant_nonce: [0; 32],
            ..context
        };
        assert!(key.derive_ptk(changed).is_err());
    }
}

#[test]
fn security_admission_rejects_pmf_downgrades_and_duplicate_security_elements() {
    let protected = profile(OWE_RSN);
    assert!(protected.negotiate(protected).unwrap());
    let mut open_pmf = OWE_RSN.to_vec();
    *open_pmf.last_mut().unwrap() = 0;
    open_pmf[OWE_RSN.len() - 2] = 0;
    assert_eq!(
        protected.negotiate(profile(&open_pmf)),
        Err(Error::SecurityMismatch)
    );
    open_pmf[OWE_RSN.len() - 2] = 64;
    assert_eq!(
        SecurityProfile::from_elements(Elements::parse(&open_pmf).unwrap()),
        Err(Error::UnsupportedSecurity)
    );
    let mut duplicate = OWE_RSN.to_vec();
    duplicate.extend_from_slice(OWE_RSN);
    assert!(matches!(
        SecurityProfile::from_elements(Elements::parse(&duplicate).unwrap()),
        Err(Error::Elements(_))
    ));
    let mut rsnxe = OWE_RSN.to_vec();
    rsnxe.extend_from_slice(&[244, 2, 0, 0]);
    assert_eq!(
        SecurityProfile::from_elements(Elements::parse(&rsnxe).unwrap()),
        Err(Error::UnsupportedSecurity)
    );
    let mut destination = [0x55; 4];
    assert_eq!(
        protected.encode(&mut destination),
        Err(Error::CapacityExceeded)
    );
    assert_eq!(destination, [0x55; 4]);
}

#[test]
fn cache_preserves_live_entries_and_enforces_scope_expiry_and_monotonic_time() {
    let mut cache = PmkCache::<1>::new(10);
    let cached = CachedPmk::new(scope(), fixture_key(Group::P256), 10, 20).unwrap();
    let pmkid = cached.pmkid();
    cache.insert(cached, 10).unwrap();
    assert_eq!(
        cache
            .lookup(scope(), Group::P256, 11)
            .unwrap()
            .unwrap()
            .pmkid(),
        pmkid
    );
    let other = CachedPmk::new(scope(), fixture_key(Group::P384), 11, 30).unwrap();
    assert_eq!(cache.insert(other, 11), Err(Error::CapacityExceeded));
    assert!(cache.lookup(scope(), Group::P256, 11).unwrap().is_some());
    let other_scope = CacheScope {
        ssid: WifiSsid::new(b"other").unwrap(),
        ..scope()
    };
    assert!(
        cache
            .lookup(other_scope, Group::P256, 11)
            .unwrap()
            .is_none()
    );
    let other_scope = CacheScope {
        management_protection: false,
        ..scope()
    };
    assert!(
        cache
            .lookup(other_scope, Group::P256, 11)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        cache.lookup(scope(), Group::P256, 9),
        Err(Error::TimeWentBackwards)
    ));
    assert!(cache.lookup(scope(), Group::P256, 20).unwrap().is_none());
    cache
        .insert(
            CachedPmk::new(scope(), fixture_key(Group::P384), 20, 30).unwrap(),
            20,
        )
        .unwrap();
    let entry = cache.lookup(scope(), Group::P384, 20).unwrap().unwrap();
    assert!(matches!(entry.duplicate(19), Err(Error::TimeWentBackwards)));
    assert!(matches!(entry.duplicate(30), Err(Error::KeyExpired)));
    cache.remove(scope());
    assert!(cache.lookup(scope(), Group::P384, 20).unwrap().is_none());
}

#[test]
fn cached_resume_requires_an_offered_echoed_live_pmkid_and_ignores_dh_body() {
    let cached = CachedPmk::new(scope(), fixture_key(Group::P256), 10, 20).unwrap();
    let mut selected = [0; 128];
    let length = RsnElement::parse(OWE_RSN)
        .unwrap()
        .encode_with_pmkids(&[cached.pmkid()], &mut selected)
        .unwrap();
    let selected = &selected[..length];
    let mut response = selected.to_vec();
    // A matched cached PMKID makes the DH body irrelevant (RFC 8110 4.5).
    response.extend_from_slice(&[255, 1, 32]);
    let association = InitialAssociation::resume(
        cached.duplicate(11).unwrap(),
        scope(),
        profile(OWE_RSN),
        profile(selected),
        Elements::parse(&response).unwrap(),
        11,
    )
    .unwrap();
    assert_eq!(association.pmkid(), cached.pmkid());
    assert!(matches!(
        InitialAssociation::resume(
            cached.duplicate(11).unwrap(),
            scope(),
            profile(OWE_RSN),
            profile(OWE_RSN),
            Elements::parse(&response).unwrap(),
            11
        ),
        Err(Error::WrongContext)
    ));
    let other_scope = CacheScope {
        ssid: WifiSsid::new(b"wrong").unwrap(),
        ..scope()
    };
    assert!(matches!(
        InitialAssociation::resume(
            cached.duplicate(11).unwrap(),
            other_scope,
            profile(OWE_RSN),
            profile(selected),
            Elements::parse(&response).unwrap(),
            11
        ),
        Err(Error::WrongContext)
    ));
    assert!(matches!(
        InitialAssociation::resume(
            cached.duplicate(11).unwrap(),
            scope(),
            profile(OWE_RSN),
            profile(selected),
            Elements::parse(&response).unwrap(),
            20
        ),
        Err(Error::KeyExpired)
    ));
    let mut wrong = response.clone();
    wrong[selected.len() - 1] ^= 1;
    assert!(matches!(
        InitialAssociation::resume(
            cached,
            scope(),
            profile(OWE_RSN),
            profile(selected),
            Elements::parse(&wrong).unwrap(),
            11
        ),
        Err(Error::WrongContext)
    ));
}

#[test]
fn association_binding_checks_security_keys_and_completion_before_caching() {
    let key = fixture_key(Group::P256);
    let association =
        InitialAssociation::new(key, scope().ssid, profile(OWE_RSN), profile(OWE_RSN)).unwrap();
    assert_eq!(
        association.validate_message2_key_data(&[]),
        Err(Error::SecurityMismatch)
    );
    association.validate_message2_key_data(OWE_RSN).unwrap();
    let keys = crate::frames::RsnGroupKeys {
        gtk: crate::frames::RsnGtk::new(1, false, [1; 16]).unwrap(),
        igtk: None,
    };
    assert!(matches!(
        association.message3_key_data::<512>(&keys),
        Err(Error::SecurityMismatch)
    ));
    let context = crate::PtkContext {
        authenticator_address: scope().addresses.access_point,
        supplicant_address: scope().addresses.station,
        authenticator_nonce: [1; 32],
        supplicant_nonce: [2; 32],
    };
    let public = [1; 32];
    let different = OwePmk::derive(Group::P256, scope().addresses, &public, &public, &[2; 32])
        .unwrap()
        .derive_ptk(context)
        .unwrap();
    assert!(matches!(
        association.message2::<512>(10, &different),
        Err(Error::WrongContext)
    ));
    let state = crate::state::RsnStaState::new(
        Group::P256,
        scope().addresses.station,
        scope().addresses.access_point,
        [2; 32],
    )
    .unwrap();
    assert!(matches!(
        association.cache_station(&state, 10, 20),
        Err(Error::WrongPhase)
    ));
}

#[test]
fn transition_discovery_requires_a_reciprocal_owe_identity_and_handles_hidden_ssid() {
    let open = BssIdentity::new([6; 6], WifiSsid::new(b"guest\0open").unwrap()).unwrap();
    let owe = BssIdentity::new([4; 6], scope().ssid).unwrap();
    let pair = TransitionPair::new(open, owe).unwrap();
    let mut open_ie = [0; 64];
    let length = pair
        .open_advertisement(Some((81, 6)))
        .encode(&mut open_ie)
        .unwrap();
    let open_elements = Elements::parse(&open_ie[..length]).unwrap();
    let target = TransitionTarget::discover(open, false, open_elements)
        .unwrap()
        .unwrap();
    assert_eq!(target.identity(), owe);
    assert_eq!(target.channel_hint(), Some((81, 6)));
    assert_eq!(
        TransitionTarget::discover(open, true, open_elements),
        Err(Error::SecurityMismatch)
    );
    let mut owe_ie = [0; 64];
    let length = pair.owe_advertisement(None).encode(&mut owe_ie).unwrap();
    let mut observed = OWE_RSN.to_vec();
    observed.extend_from_slice(&owe_ie[..length]);
    let observed = Elements::parse(&observed).unwrap();
    assert_eq!(
        target.confirm(owe.bssid(), None, true, observed).unwrap(),
        profile(OWE_RSN)
    );
    target
        .confirm(owe.bssid(), Some(owe.ssid()), true, observed)
        .unwrap();
    assert_eq!(
        target.confirm([8; 6], None, true, observed),
        Err(Error::WrongContext)
    );
    assert_eq!(
        target.confirm(owe.bssid(), Some(open.ssid()), true, observed),
        Err(Error::WrongContext)
    );
    assert_eq!(
        target.confirm(owe.bssid(), None, false, observed),
        Err(Error::SecurityMismatch)
    );
    assert_eq!(
        target.confirm(owe.bssid(), None, true, Elements::parse(OWE_RSN).unwrap()),
        Err(Error::SecurityMismatch)
    );
    assert_eq!(TransitionPair::new(open, open), Err(Error::WrongContext));
    assert_eq!(
        TransitionTarget::discover(open, false, Elements::EMPTY),
        Ok(None)
    );
}
