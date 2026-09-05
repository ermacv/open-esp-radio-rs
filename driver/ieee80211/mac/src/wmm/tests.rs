use super::{
    Dscp, WMM_OUI_AND_TYPE, WMM_PARAMETER_BODY_LEN, WmmAccessCategory, WmmClassificationSource,
    WmmUserPriority, classify_ethernet_wmm, parse_wmm_parameter_element,
};

const STANDARD_PARAMETER_ELEMENT: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

#[test]
fn parses_all_four_access_categories_and_complete_txop() {
    let parameters = parse_wmm_parameter_element(&STANDARD_PARAMETER_ELEMENT).unwrap();
    assert_eq!(parameters.parameter_set_count, 5);
    assert!(parameters.uapsd);

    let best_effort = parameters.access_category(WmmAccessCategory::BestEffort);
    assert_eq!(best_effort.aifsn, 3);
    assert_eq!(best_effort.ecw_min, 4);
    assert_eq!(best_effort.ecw_max, 10);
    assert_eq!(best_effort.txop_limit_units_32_us, 0);

    let video = parameters.access_category(WmmAccessCategory::Video);
    assert_eq!(video.aifsn, 2);
    assert_eq!(video.ecw_min, 3);
    assert_eq!(video.ecw_max, 4);
    assert_eq!(video.txop_limit_units_32_us, 94);

    let voice = parameters.access_category(WmmAccessCategory::Voice);
    assert!(voice.admission_control_mandatory);
    assert_eq!(voice.txop_limit_units_32_us, 47);
}

#[test]
fn retains_the_standard_txop_high_byte_that_the_blob_drops() {
    let mut element = STANDARD_PARAMETER_ELEMENT;
    element[21] = 1;
    assert_eq!(
        parse_wmm_parameter_element(&element)
            .unwrap()
            .access_category(WmmAccessCategory::Video)
            .txop_limit_units_32_us,
        350
    );
}

#[test]
fn rejects_wrong_identity_short_elements_and_duplicate_aci() {
    let mut element = STANDARD_PARAMETER_ELEMENT;
    element[2..6].copy_from_slice(&WMM_OUI_AND_TYPE);
    element[1] = (WMM_PARAMETER_BODY_LEN - 1) as u8;
    assert!(parse_wmm_parameter_element(&element).is_none());

    element = STANDARD_PARAMETER_ELEMENT;
    element[22] = 0x42;
    assert!(parse_wmm_parameter_element(&element).is_none());
}

#[test]
fn typed_user_priorities_map_to_the_four_standard_access_categories() {
    let expected = [
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Background,
        WmmAccessCategory::Background,
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Video,
        WmmAccessCategory::Video,
        WmmAccessCategory::Voice,
        WmmAccessCategory::Voice,
    ];
    for (value, category) in expected.into_iter().enumerate() {
        let priority = WmmUserPriority::new(value as u8).unwrap();
        assert_eq!(priority.value(), value as u8);
        assert_eq!(priority.access_category(), category);
    }
    assert_eq!(WmmUserPriority::new(8), None);
}

#[test]
fn station_dscp_mapping_bleaches_unassigned_and_network_control_values() {
    let cases = [
        (0, 0),
        (1, 1),
        (8, 1),
        (10, 0),
        (18, 3),
        (24, 4),
        (34, 4),
        (40, 5),
        (44, 6),
        (46, 6),
        (48, 0),
        (56, 0),
        (63, 0),
    ];
    for (dscp, priority) in cases {
        assert_eq!(
            Dscp::new(dscp).unwrap().station_user_priority().value(),
            priority
        );
    }
    assert_eq!(Dscp::new(64), None);
    assert_eq!(Dscp::new(46).unwrap().default_user_priority().value(), 5);
}

#[test]
fn ethernet_classifier_uses_vlan_pcp_before_ipv4_dscp() {
    let mut frame = [0_u8; 22];
    frame[12..14].copy_from_slice(&0x8100_u16.to_be_bytes());
    frame[14..16].copy_from_slice(&(5_u16 << 13).to_be_bytes());
    frame[16..18].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[18] = 0x45;
    frame[19] = 46 << 2;

    let class = classify_ethernet_wmm(&frame);
    assert_eq!(class.user_priority, WmmUserPriority::UP5);
    assert_eq!(class.access_category, WmmAccessCategory::Video);
    assert_eq!(
        class.source,
        WmmClassificationSource::Ieee8021d(WmmUserPriority::UP5)
    );
}

#[test]
fn ethernet_classifier_extracts_ipv4_and_ipv6_dscp_fail_closed() {
    let mut ipv4 = [0_u8; 16];
    ipv4[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    ipv4[14] = 0x45;
    ipv4[15] = 46 << 2;
    let class = classify_ethernet_wmm(&ipv4);
    assert_eq!(class.user_priority, WmmUserPriority::UP6);
    assert_eq!(class.access_category, WmmAccessCategory::Voice);

    let mut ipv6 = [0_u8; 16];
    ipv6[12..14].copy_from_slice(&0x86dd_u16.to_be_bytes());
    let traffic_class = 40_u8 << 2;
    ipv6[14] = 0x60 | (traffic_class >> 4);
    ipv6[15] = traffic_class << 4;
    let class = classify_ethernet_wmm(&ipv6);
    assert_eq!(class.user_priority, WmmUserPriority::UP5);

    ipv6[14] = 0x40;
    assert_eq!(
        classify_ethernet_wmm(&ipv6).user_priority,
        WmmUserPriority::UP0
    );
    assert_eq!(
        classify_ethernet_wmm(&[0; 13]).user_priority,
        WmmUserPriority::UP0
    );
}
