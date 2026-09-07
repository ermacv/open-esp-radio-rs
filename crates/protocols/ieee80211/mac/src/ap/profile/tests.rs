use super::*;

pub(crate) const TEST_ADVERTISEMENT: crate::ap::profile::Advertisement = {
    use crate::{
        ap::profile::{Advertisement, LegacyRates, WmmParameters},
        extensions::wmm::WmmAcParameters,
    };
    Advertisement::new(
        LegacyRates::new(
            [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
            [0x6c, 0x12, 0x24, 0x48],
        ),
        crate::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
        WmmParameters::new(
            4,
            false,
            [
                WmmAcParameters {
                    admission_control_mandatory: false,
                    aifsn: 3,
                    ecw_min: 4,
                    ecw_max: 10,
                    txop_limit_units_32_us: 0,
                },
                WmmAcParameters {
                    admission_control_mandatory: false,
                    aifsn: 7,
                    ecw_min: 4,
                    ecw_max: 10,
                    txop_limit_units_32_us: 0,
                },
                WmmAcParameters {
                    admission_control_mandatory: false,
                    aifsn: 2,
                    ecw_min: 3,
                    ecw_max: 4,
                    txop_limit_units_32_us: 94,
                },
                WmmAcParameters {
                    admission_control_mandatory: false,
                    aifsn: 2,
                    ecw_min: 2,
                    ecw_max: 3,
                    txop_limit_units_32_us: 47,
                },
            ],
        ),
        // ESS, Short Preamble and Short Slot Time. Privacy follows AP security.
        0x0421,
        // No non-ERP peer/protection/Barker-preamble requirement at creation.
        0,
    )
};

#[test]
fn wmm_parameters_round_trip_each_access_category_and_admission_bit() {
    use crate::{extensions::wmm::parse_wmm_parameter_element, qos::WmmAccessCategory};
    let categories = [
        WmmAcParameters {
            admission_control_mandatory: true,
            aifsn: 9,
            ecw_min: 2,
            ecw_max: 7,
            txop_limit_units_32_us: 513,
        },
        WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: 5,
            ecw_min: 3,
            ecw_max: 8,
            txop_limit_units_32_us: 0,
        },
        WmmAcParameters {
            admission_control_mandatory: true,
            aifsn: 2,
            ecw_min: 4,
            ecw_max: 9,
            txop_limit_units_32_us: 23,
        },
        WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: 1,
            ecw_min: 5,
            ecw_max: 10,
            txop_limit_units_32_us: 41,
        },
    ];
    let element = WmmParameters::new(11, true, categories).element();
    let decoded = parse_wmm_parameter_element(&element).unwrap();
    assert_eq!(decoded.parameter_set_count, 11);
    assert!(decoded.uapsd);
    for (category, expected) in [
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Background,
        WmmAccessCategory::Video,
        WmmAccessCategory::Voice,
    ]
    .into_iter()
    .zip(categories)
    {
        assert_eq!(decoded.access_category(category), expected);
    }
}

#[test]
fn beacon_association_and_peer_rate_admission_use_the_supplied_profile() {
    use crate::{
        ap::{
            ApManagementRequest, parse_ap_management_request,
            write_ht_association_response_frame_for_security,
        },
        beacon::write_ht_beacon,
        channel::WifiChannel,
        ssid::WifiSsid,
    };
    let profile = Advertisement::new(
        LegacyRates::new([0x82; 8], [0x84; 4]),
        TEST_ADVERTISEMENT.ht,
        TEST_ADVERTISEMENT.wmm,
        1,
        5,
    );
    let address = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let channel = WifiChannel::mhz20(6).unwrap();
    let mut beacon = [0; 256];
    write_ht_beacon(
        &profile,
        &mut beacon,
        address,
        &WifiSsid::new(b"profile").unwrap(),
        channel,
        100,
        2,
        0,
        WifiSecurityMode::Open,
    )
    .unwrap();
    assert_eq!(u16::from_le_bytes(beacon[34..36].try_into().unwrap()), 1);
    let mut response = [0; 256];
    write_ht_association_response_frame_for_security(
        &profile,
        &mut response,
        address,
        peer,
        0,
        1,
        0,
        channel,
        None,
        WifiSecurityMode::Wpa2Personal,
    )
    .unwrap();
    assert_eq!(
        u16::from_le_bytes(response[24..26].try_into().unwrap()),
        0x0011
    );
    assert_eq!(response[48], 5);
    assert_eq!(&response[32..40], &[0x82; 8]);
    assert_eq!(&response[42..46], &[0x84; 4]);

    let mut request = [0; 33];
    request[4..10].copy_from_slice(&address);
    request[10..16].copy_from_slice(&peer);
    request[16..22].copy_from_slice(&address);
    request[28..].copy_from_slice(&[1, 3, 108, 0x82, 0x84]);
    assert!(matches!(
        parse_ap_management_request(&profile, &request, address),
        Some(ApManagementRequest::Association {
            maximum_legacy_rate_500kbps: 4,
            ..
        })
    ));
}
