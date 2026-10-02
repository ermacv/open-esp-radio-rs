use super::*;
use crate::management::elements::{ElementError, Elements};

#[test]
fn authentication_and_ds_rejections_preserve_status_and_complete_body() {
    // A failure response is allowed to have no FT IEs.
    let auth = [2, 0, 2, 0, 53, 0];
    let parsed = Authentication::parse(&auth).unwrap();
    assert_eq!(parsed.status, 53);
    let mut output = [0xa5; 32];
    assert_eq!(parsed.encode(&mut output).unwrap(), auth.len());
    assert_eq!(&output[..auth.len()], auth);
    let action = [6, 2, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 3, 28, 0];
    let parsed = Action::parse(&action).unwrap();
    assert_eq!(parsed.status, Some(28));
    assert_eq!(parsed.target, [2, 0, 0, 0, 0, 3]);
    assert_eq!(parsed.encode(&mut output).unwrap(), action.len());
    assert_eq!(&output[..action.len()], action);
    assert!(Action::parse(&action[..15]).is_err());
    assert!(Authentication::parse(&[0, 0, 2, 0, 0, 0]).is_err());
    assert!(Authentication::parse(&[2, 0, 2, 0, 0, 0, 54]).is_err());
}

#[test]
fn ft_subelements_preserve_unknowns_and_reject_duplicates_before_writing() {
    let subelements = Elements::parse(&[3, 2, 0xff, 0, 1, 6, 1, 2, 3, 4, 5, 6, 200, 1, 9]).unwrap();
    let fields = FastTransitionFields {
        element_count: 0,
        rsnxe_used: false,
        mic: [0; 16],
        anonce: [7; 32],
        snonce: [8; 32],
        subelements,
    };
    let mut bytes = [0; 256];
    let len = fields.encode(&mut bytes).unwrap();
    let ft = FastTransitionElement::parse(&bytes[..len]).unwrap();
    assert_eq!(ft.r0kh().unwrap().unwrap().as_bytes(), &[0xff, 0]);
    assert_eq!(ft.r1kh().unwrap(), Some(R1khId([1, 2, 3, 4, 5, 6])));
    assert_eq!(ft.subelements().as_bytes(), subelements.as_bytes());
    let mut tiny = [0xa5; 10];
    assert!(fields.encode(&mut tiny).is_err());
    assert_eq!(tiny, [0xa5; 10]);
    let duplicate = FastTransitionFields {
        subelements: Elements::parse(&[3, 1, 1, 3, 1, 2]).unwrap(),
        ..fields
    };
    let untouched = bytes;
    assert_eq!(
        duplicate.encode(&mut bytes),
        Err(WireError::Elements(ElementError::Duplicate(3)))
    );
    assert_eq!(bytes, untouched);
}

#[test]
fn pmkid_replacement_preserves_advertised_suites_and_rejects_short_output_atomically() {
    let advertised = [
        48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 0, 0,
    ];
    let rsn = crate::security::rsn::RsnElement::parse(&advertised).unwrap();
    let mut output = [0xa5; 64];
    let len = rsn.encode_with_pmkids(&[[0x77; 16]], &mut output).unwrap();
    let changed = crate::security::rsn::RsnElement::parse(&output[..len]).unwrap();
    assert_eq!(changed.akm_suites(), rsn.akm_suites());
    assert_eq!(changed.pmkids().next(), Some([0x77; 16]));
    let mut short = [0xa5; 22];
    assert!(rsn.encode_with_pmkids(&[[0; 16]], &mut short).is_err());
    assert_eq!(short, [0xa5; 22]);
}

#[test]
fn complete_ft_elements_reject_every_partial_ie_duplicate_and_wrong_protected_count() {
    let rsn = [
        48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 0, 0,
    ];
    let md = MobilityDomain {
        id: MobilityDomainId([0x12, 0x34]),
        over_ds: true,
        resource_request: true,
    }
    .encode();
    let fields = FastTransitionFields {
        element_count: 3,
        rsnxe_used: false,
        mic: [0; 16],
        anonce: [1; 32],
        snonce: [2; 32],
        subelements: Elements::EMPTY,
    };
    let mut ft = [0; 257];
    let length = fields.encode(&mut ft).unwrap();
    let mut bytes = rsn.to_vec();
    bytes.extend_from_slice(&md);
    bytes.extend_from_slice(&ft[..length]);
    assert!(FtElements::parse(&bytes).is_ok());
    for end in 0..bytes.len() {
        assert!(
            FtElements::parse(&bytes[..end]).is_err(),
            "accepted truncated prefix {end}"
        );
    }
    for extra in [&rsn[..], &md[..], &ft[..length]] {
        let mut duplicate = bytes.clone();
        duplicate.extend_from_slice(extra);
        assert!(FtElements::parse(&duplicate).is_err());
    }
    let mut invalid = bytes.clone();
    let offset = rsn.len() + md.len();
    invalid[offset + 3] = 4;
    assert!(FtElements::parse(&invalid).is_err());
    let mut xe = bytes.clone();
    xe.extend_from_slice(&[244, 1, 0x20]);
    assert!(FtElements::parse(&xe).is_err());
    xe[offset + 3] = 4;
    assert!(FtElements::parse(&xe).is_ok());
    let mut bad_xe = xe.clone();
    *bad_xe.last_mut().unwrap() = 0x21;
    assert!(FtElements::parse(&bad_xe).is_err());
    let mut zero_count = bytes.clone();
    zero_count[offset + 3] = 0;
    assert!(FtElements::parse(&zero_count).is_ok());
}

#[test]
fn ric_descriptors_are_counted_contiguous_and_cannot_substitute_protected_security_elements() {
    let good = [
        RIC_DATA_ELEMENT_ID,
        4,
        7,
        1,
        0,
        0,
        13,
        1,
        0,
        RIC_DATA_ELEMENT_ID,
        4,
        8,
        0,
        0,
        0,
    ];
    RicElements::parse(&good).unwrap();
    assert!(RicElements::parse(&good[..7]).is_err());
    let mut bad = good;
    bad[6] = MOBILITY_DOMAIN_ELEMENT_ID;
    assert!(RicElements::parse(&bad).is_err());
    let mut duplicate = good;
    duplicate[11] = 7;
    assert!(RicElements::parse(&duplicate).is_err());
    assert!(RicElements::parse(&[13, 0]).is_err());
}

#[test]
fn timeouts_preserve_tu_and_seconds_as_distinct_types_and_reject_malformed_values() {
    let deadline = [56, 5, 1, 0xe8, 3, 0, 0];
    let lifetime = [56, 5, 2, 0x10, 0x0e, 0, 0];
    assert_eq!(
        TimeoutInterval::parse(&deadline).unwrap(),
        TimeoutInterval::ReassociationDeadline { tu: 1000 }
    );
    assert_eq!(
        TimeoutInterval::parse(&lifetime).unwrap(),
        TimeoutInterval::KeyLifetime { seconds: 3600 }
    );
    assert_eq!(
        TimeoutInterval::KeyLifetime { seconds: 3600 }.encode(),
        lifetime
    );
    assert!(TimeoutInterval::parse(&[56, 4, 1, 0, 0, 0]).is_err());
    assert!(TimeoutInterval::parse(&[56, 5, 0, 0, 0, 0, 0]).is_err());
}
