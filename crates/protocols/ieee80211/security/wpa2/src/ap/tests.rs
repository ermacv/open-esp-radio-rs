use super::*;

fn rsn(pairwise: u8, akm: u8, capabilities: u16) -> [u8; 22] {
    let mut ie = [0_u8; 22];
    ie[0] = 0x30;
    ie[1] = 20;
    ie[2..4].copy_from_slice(&1_u16.to_le_bytes());
    ie[4..8].copy_from_slice(&[0x00, 0x0f, 0xac, 4]);
    ie[8..10].copy_from_slice(&1_u16.to_le_bytes());
    ie[10..14].copy_from_slice(&[0x00, 0x0f, 0xac, pairwise]);
    ie[14..16].copy_from_slice(&1_u16.to_le_bytes());
    ie[16..20].copy_from_slice(&[0x00, 0x0f, 0xac, akm]);
    ie[20..22].copy_from_slice(&capabilities.to_le_bytes());
    ie
}

#[test]
fn accepts_wpa2_psk_ccmp_and_optional_mfpc() {
    let ie = rsn(4, 2, 1 << 7);
    let validated = validate_wpa2_ap_rsn(&ie).unwrap();
    assert_eq!(validated.owned().as_bytes(), &ie);
    assert_eq!(validated.capabilities(), 1 << 7);
}

#[test]
fn rejects_non_ccmp_non_psk_and_required_pmf() {
    assert_eq!(
        validate_wpa2_ap_rsn(&rsn(2, 2, 0)),
        Err(Wpa2ApRsnError::UnsupportedPairwiseCipher)
    );
    assert_eq!(
        validate_wpa2_ap_rsn(&rsn(4, 8, 0)),
        Err(Wpa2ApRsnError::UnsupportedAkm)
    );
    assert_eq!(
        validate_wpa2_ap_rsn(&rsn(4, 2, 1 << 6)),
        Err(Wpa2ApRsnError::ManagementFrameProtectionUnsupported)
    );
}

#[test]
fn accepts_zero_pmkid_count_and_rejects_nonzero_lists() {
    let mut ie = [0_u8; 24];
    ie[..22].copy_from_slice(&rsn(4, 2, 0));
    ie[1] = 22;
    assert!(validate_wpa2_ap_rsn(&ie).is_ok());
    ie[22..24].copy_from_slice(&1_u16.to_le_bytes());
    assert_eq!(
        validate_wpa2_ap_rsn(&ie),
        Err(Wpa2ApRsnError::PmkidCachingUnsupported)
    );
}
