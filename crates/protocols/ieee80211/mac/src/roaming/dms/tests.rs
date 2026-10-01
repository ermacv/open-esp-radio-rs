use super::*;
use crate::roaming::{TclasParameters, TclasRule};
fn ethernet() -> std::vec::Vec<u8> {
    std::vec![14, 17, 0, 0, 6, 0, 0, 0, 0, 0, 0, 1, 0, 94, 0, 0, 251, 8, 0]
}
#[test]
fn dms_request_and_response_have_distinct_processing_requirements() {
    let single = ethernet();
    let multiple = [single.clone(), single.clone()].concat();
    let multiple_processing = [multiple.clone(), std::vec![44, 1, 0]].concat();
    let single_processing = [single.clone(), std::vec![44, 1, 0]].concat();
    for (bytes, valid_add, valid_response) in [
        (&single, true, true),
        (&multiple, false, true),
        (&multiple_processing, true, true),
        (&single_processing, false, false),
    ] {
        let attributes = Elements::parse(bytes).unwrap();
        assert_eq!(
            DmsDescriptor {
                dms_id: 0,
                request_type: DmsRequestType::ADD,
                attributes,
            }
            .validate()
            .is_ok(),
            valid_add
        );
        assert_eq!(
            DmsStatus {
                dms_id: 7,
                response_type: DmsResponseType::ACCEPT,
                last_sequence_control: 0xffff,
                attributes,
            }
            .validate()
            .is_ok(),
            valid_response
        );
    }
}
#[test]
fn optional_qos_syntax_belongs_to_dms_attributes() {
    let bytes = [ethernet(), std::vec![13, 1, 42]].concat();
    let attributes = Elements::parse(&bytes).unwrap();
    assert_eq!(super::super::validate_tclas_elements(attributes), Ok(()));
    assert_eq!(
        validate_dms_attributes(attributes),
        Err(WireError::InvalidElementLength(13))
    );
    let bytes = [ethernet(), std::vec![13, 55], std::vec![0; 55]].concat();
    assert_eq!(
        validate_dms_attributes(Elements::parse(&bytes).unwrap()),
        Ok(())
    );
}
#[test]
fn independent_dms_fixture_retains_nested_descriptors_and_network_byte_order() {
    let attributes = ethernet();
    let bytes = [std::vec![10, 23, 7, 99, 22, 0, 20, 0], attributes.clone()].concat();
    let request = DmsRequest::parse(&bytes).unwrap();
    let descriptor = request.descriptors().unwrap().next().unwrap();
    assert_eq!(descriptor.dms_id, 0);
    assert_eq!(descriptor.request_type, DmsRequestType::ADD);
    let rule = TclasRule::parse(descriptor.attributes.iter().next().unwrap().body).unwrap();
    let TclasParameters::Ethernet {
        destination,
        ether_type,
        ..
    } = rule.parameters
    else {
        panic!()
    };
    assert_eq!(destination, [1, 0, 94, 0, 0, 251]);
    assert_eq!(ether_type, 0x0800);
    let mut out = [0; 64];
    let len = request.encode(&mut out).unwrap();
    assert_eq!(&out[..len], bytes);
    let status = DmsStatus {
        dms_id: 3,
        response_type: DmsResponseType::ACCEPT,
        last_sequence_control: 0xffff,
        attributes: descriptor.attributes,
    };
    let len = status.encode(&mut out).unwrap();
    assert_eq!(DmsStatus::parse(out[0], &out[2..len]).unwrap(), status);
}
#[test]
fn ipv4_and_ipv6_ports_dscp_and_flow_label_are_preserved() {
    let ipv4 = [
        0, 1, 127, 4, 10, 0, 0, 1, 224, 0, 0, 251, 0x12, 0x34, 0x14, 0xe9, 0xc3, 17, 0,
    ];
    let rule = TclasRule::parse(&ipv4).unwrap();
    let TclasParameters::Ipv4 {
        source_port,
        destination_port,
        dscp,
        ..
    } = rule.parameters
    else {
        panic!()
    };
    assert_eq!(source_port, 0x1234);
    assert_eq!(destination_port, 5353);
    assert_eq!(dscp, 0xc3);
    let mut out = [0; 64];
    let len = rule.encode(&mut out).unwrap();
    assert_eq!(&out[2..len], ipv4);
    let mut ipv6 = std::vec![0, 4, 255, 6];
    ipv6.extend_from_slice(&[0; 16]);
    let mut destination = [0; 16];
    destination[0] = 255;
    destination[15] = 251;
    ipv6.extend_from_slice(&destination);
    ipv6.extend_from_slice(&[0x12, 0x34, 0x14, 0xe9, 3, 17, 0x81, 0x23, 0x45]);
    let rule = TclasRule::parse(&ipv6).unwrap();
    let len = rule.encode(&mut out).unwrap();
    assert_eq!(&out[2..len], ipv6);
    let TclasParameters::Ipv6 {
        flow_label,
        next_header,
        ..
    } = rule.parameters
    else {
        panic!()
    };
    assert_eq!(flow_label, [0x81, 0x23, 0x45]);
    assert_eq!(next_header, Some(17));
}
#[test]
fn classifier_identity_is_order_independent_but_preserves_multiplicity_and_masks() {
    let first = ethernet();
    let mut second = first.clone();
    second[4] = 2;
    let a = [
        first.clone(),
        second.clone(),
        std::vec![44, 1, 1, 221, 2, 7, 8],
    ]
    .concat();
    let b = [second.clone(), first.clone(), std::vec![44, 1, 1]].concat();
    assert!(
        equivalent_dms_classifiers(Elements::parse(&a).unwrap(), Elements::parse(&b).unwrap())
            .unwrap()
    );
    let c = [first.clone(), first, std::vec![44, 1, 1]].concat();
    assert!(
        !equivalent_dms_classifiers(Elements::parse(&a).unwrap(), Elements::parse(&c).unwrap())
            .unwrap()
    );
}
#[test]
fn termination_sequence_special_values_and_autonomous_token_are_distinct() {
    let bytes = [10, 24, 0, 100, 5, 7, 3, 2, 0xf0, 0xff];
    let response = DmsResponse::parse(&bytes).unwrap();
    let status = response.statuses().unwrap().next().unwrap();
    assert_eq!(
        LastSequenceControl::parse(status.last_sequence_control).unwrap(),
        LastSequenceControl::Sequence(4095)
    );
    assert_eq!(
        LastSequenceControl::parse(0xfffe).unwrap(),
        LastSequenceControl::NotGroupTransmitted
    );
    assert_eq!(
        LastSequenceControl::parse(0xffff).unwrap(),
        LastSequenceControl::Unsupported
    );
    assert!(LastSequenceControl::parse(0x1231).is_err());
    let mut out = [0; 16];
    let len = response.encode(&mut out).unwrap();
    assert_eq!(&out[..len], bytes);
}
#[test]
fn malformed_nested_lengths_and_output_shortage_never_write_partial_frames() {
    assert!(DmsRequest::parse(&[10, 23, 1, 99, 4, 0, 2, 0, 14]).is_err());
    assert!(DmsDescriptor::parse(0, &[1]).is_err());
    assert!(DmsStatus::parse(0, &[0, 255, 255]).is_err());
    let response = DmsResponse::parse(&[10, 24, 0, 100, 5, 7, 3, 2, 255, 255]).unwrap();
    let mut out = [0xaa; 9];
    assert!(response.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 9]);
    let bytes = ethernet();
    let rule = TclasRule::parse(&bytes[2..]).unwrap();
    let mut out = [0xaa; 18];
    assert!(rule.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 18]);
}

#[test]
fn opaque_known_classifier_is_validated_before_writing_and_tfs_types_round_trip() {
    let mut out = [0xaa; 64];
    let malformed = TclasRule {
        user_priority: 0,
        mask: 2,
        parameters: TclasParameters::Other {
            kind: 0,
            body: &[1],
        },
    };
    assert!(malformed.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 64]);
    for bytes in [
        &[0, 2, 1, 0x60, 0x2a][..],
        &[0, 3, 0, 1, 0, 0xa0, 0x12, 0xf0, 0xff],
        &[0, 5, 7, 3, 1, 0, 42],
    ] {
        let rule = TclasRule::parse(bytes).unwrap();
        let n = rule.encode(&mut out).unwrap();
        assert_eq!(&out[2..n], bytes);
    }
    assert!(TclasRule::parse(&[0, 3, 0, 0, 0, 1]).is_err());
}
