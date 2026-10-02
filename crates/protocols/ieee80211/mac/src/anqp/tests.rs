use super::*;

#[test]
fn query_ids_are_little_endian_sorted_and_lengths_are_complete() {
    let bytes = [0, 1, 4, 0, 1, 1, 12, 1];
    let list = Elements::parse(&bytes).unwrap();
    list.validate().unwrap();
    let Value::QueryList(query) = list.iter().next().unwrap().value().unwrap() else {
        panic!()
    };
    assert_eq!(
        query.iter().collect::<std::vec::Vec<_>>(),
        [InfoId::CAPABILITY_LIST, InfoId::DOMAIN_NAME]
    );
    assert_eq!(
        QueryList::parse(&[12, 1, 1, 1]),
        Err(WireError::UnorderedIds)
    );
    assert!(Elements::parse(&bytes[..bytes.len() - 1]).is_err());
    assert!(Elements::parse(&[0, 1, 0]).is_err());
}
#[test]
fn names_validate_language_utf8_and_atomic_output() {
    let names = [
        Name {
            language: *b"eng",
            text: "Cafe",
        },
        Name {
            language: *b"rus",
            text: "Точка",
        },
    ];
    let mut bytes = [0u8; 64];
    let length = Names::encode(&names, &mut bytes).unwrap();
    assert_eq!(
        Names::parse(&bytes[..length])
            .unwrap()
            .iter()
            .collect::<std::vec::Vec<_>>(),
        names
    );
    assert_eq!(
        Names::parse(&[4, b'e', b'n', b'g', 255]),
        Err(WireError::InvalidText)
    );
    let mut short = [0x55; 1];
    assert!(Names::encode(&names, &mut short).is_err());
    assert_eq!(short, [0x55]);
}
#[test]
fn nested_realm_method_and_parameter_counts_are_validated() {
    // One ASCII realm "a", EAP-TLS (13), credential parameter 5 = certificate.
    let bytes = [1, 0, 10, 0, 0, 1, b'a', 1, 5, 13, 1, 5, 1, 6];
    let realms = realm::Realms::parse(&bytes).unwrap();
    let record = realms.iter().next().unwrap();
    assert_eq!(record.realms, b"a");
    assert_eq!(record.methods.iter().next().unwrap().method, 13);
    for length in 0..bytes.len() {
        assert!(realm::Realms::parse(&bytes[..length]).is_err());
    }
    let mut wrong_count = bytes;
    wrong_count[10] = 2;
    assert!(realm::Realms::parse(&wrong_count).is_err());
    let mut output = [0u8; 32];
    let records = realms.iter().collect::<std::vec::Vec<_>>();
    let length = realm::Realms::encode(&records, &mut output).unwrap();
    assert_eq!(&output[..length], bytes);
}
#[test]
fn hotspot_namespace_unknown_extensions_and_wan_fields() {
    let metric = hs20::WanMetrics {
        info: 5,
        downlink_kbps: 0x12345678,
        uplink_kbps: 1000,
        downlink_load: 20,
        uplink_load: 0,
        measurement_duration: 100,
    };
    let mut metric_bytes = [0u8; hs20::WAN_METRICS_LEN];
    metric.encode(&mut metric_bytes).unwrap();
    assert_eq!(&metric_bytes[1..5], &[0x78, 0x56, 0x34, 0x12]);
    let mut bytes = [0u8; 64];
    let length = hs20::Element {
        subtype: hs20::Subtype::WAN_METRICS,
        reserved: 0,
        body: &metric_bytes,
    }
    .encode(&mut bytes)
    .unwrap();
    let element = Elements::parse(&bytes[..length])
        .unwrap()
        .iter()
        .next()
        .unwrap();
    assert_eq!(element.id, InfoId::VENDOR);
    let Value::Hotspot(value) = element.value().unwrap() else {
        panic!()
    };
    assert_eq!(value.value(), Ok(hs20::Value::WanMetrics(metric)));
    let unknown = Element {
        id: InfoId(500),
        body: &[1, 2],
    };
    assert_eq!(unknown.value(), Ok(Value::Unknown(unknown)));
    assert!(hs20::Connections::parse(&[1, 2, 3]).is_err());
    let connection = hs20::Connection {
        protocol: 6,
        port: 443,
        status: hs20::connection_status::OPEN,
    };
    let length = hs20::Connections::encode(&[connection], &mut bytes).unwrap();
    assert_eq!(&bytes[..length], &[6, 0xbb, 1, 1]);
    assert_eq!(
        hs20::Connections::parse(&bytes[..length])
            .unwrap()
            .iter()
            .next(),
        Some(connection)
    );
}

#[test]
fn osu_provider_nested_lengths_ssid_and_icon_metadata_are_checked() {
    let mut names = [0u8; 32];
    let names_length = Names::encode(
        &[Name {
            language: *b"eng",
            text: "Provider",
        }],
        &mut names,
    )
    .unwrap();
    let names = Names::parse(&names[..names_length]).unwrap();
    let mut icons = [0u8; 64];
    let icon_length = hs20::Icons::encode(
        &[hs20::IconMetadata {
            width: 64,
            height: 32,
            language: *b"eng",
            mime: "image/png",
            filename: "logo.png",
        }],
        &mut icons,
    )
    .unwrap();
    let icons = hs20::Icons::parse(&icons[..icon_length]).unwrap();
    let provider = hs20::OsuProvider {
        names,
        server_uri: b"https://osu.example",
        methods: &[0, 1],
        icons,
        nai: "osu@example",
        service_description: names,
    };
    let mut bytes = [0u8; 256];
    let length = hs20::OsuProviders::encode(&[0, 255, 0], &[provider], &mut bytes).unwrap();
    let list = hs20::OsuProviders::parse(&bytes[..length]).unwrap();
    assert_eq!(list.ssid, &[0, 255, 0]);
    assert_eq!(list.count(), 1);
    assert_eq!(list.iter().next(), Some(provider));
    for size in 0..length {
        assert!(hs20::OsuProviders::parse(&bytes[..size]).is_err());
    }
    let mut excess = bytes[..length].to_vec();
    excess.push(0);
    assert!(hs20::OsuProviders::parse(&excess).is_err());
    assert!(hs20::OsuProviders::encode(&[0; 33], &[], &mut bytes).is_err());
    let mut short = [0x55; 4];
    assert!(hs20::OsuProviders::encode(b"osu", &[provider], &mut short).is_err());
    assert_eq!(short, [0x55; 4]);
}
#[test]
fn icon_file_failure_and_binary_body_lengths_are_strict() {
    let file = hs20::IconBinaryFile {
        status: hs20::IconStatus::SUCCESS,
        mime: "image/png",
        data: &[0, 255, 0, 1],
    };
    let mut bytes = [0u8; 64];
    let length = file.encode(&mut bytes).unwrap();
    assert_eq!(hs20::IconBinaryFile::parse(&bytes[..length]), Ok(file));
    let mut truncated = bytes[..length].to_vec();
    truncated.pop();
    assert!(hs20::IconBinaryFile::parse(&truncated).is_err());
    let wrong = hs20::IconBinaryFile {
        status: hs20::IconStatus::FILE_NOT_FOUND,
        ..file
    };
    let mut output = [0x55; 64];
    assert_eq!(wrong.encode(&mut output), Err(WireError::InvalidValue));
    assert_eq!(output, [0x55; 64]);
    assert_eq!(
        hs20::IconBinaryFile::parse(&[1, 0, 0, 0]),
        Ok(hs20::IconBinaryFile {
            status: hs20::IconStatus::FILE_NOT_FOUND,
            mime: "",
            data: &[]
        })
    );
}

#[test]
fn incremental_realm_failure_keeps_previous_count_and_records() {
    let record = realm::RealmData {
        encoding: 0,
        realms: b"a",
        methods: realm::EapMethods::EMPTY,
    };
    let mut bytes = [0u8; 10];
    let mut encoder = realm::Encoder::new(&mut bytes).unwrap();
    encoder.push(record).unwrap();
    assert!(encoder.push(record).is_err());
    assert_eq!(
        encoder.push(realm::RealmData {
            realms: &[255],
            ..record
        }),
        Err(WireError::InvalidText)
    );
    let length = encoder.finish();
    let result = realm::Realms::parse(&bytes[..length]).unwrap();
    assert_eq!(result.count(), 1);
    assert_eq!(result.iter().next(), Some(record));
}

#[test]
fn base_singletons_reject_duplicates_and_unknown_cardinality_is_preserved() {
    let mut bytes = [0u8; 32];
    let known = Element {
        id: InfoId::IP_ADDRESS_AVAILABILITY,
        body: &[0],
    };
    let first = known.encode(&mut bytes).unwrap();
    let second = known.encode(&mut bytes[first..]).unwrap();
    assert_eq!(
        Elements::parse(&bytes[..first + second])
            .unwrap()
            .validate(),
        Err(WireError::Duplicate(InfoId::IP_ADDRESS_AVAILABILITY))
    );
    let unknown = Element {
        id: InfoId(500),
        body: b"data",
    };
    let first = unknown.encode(&mut bytes).unwrap();
    let second = unknown.encode(&mut bytes[first..]).unwrap();
    let list = Elements::parse(&bytes[..first + second]).unwrap();
    list.validate().unwrap();
    assert_eq!(list.iter().count(), 2);
}

#[test]
fn provider_record_builders_keep_framing_in_mac_and_reject_partial_output() {
    let mut nested = [0u8; 32];
    let nested_length = hs20::Element {
        subtype: hs20::Subtype::CAPABILITY_LIST,
        reserved: 0,
        body: &[1, 2, 4],
    }
    .encode(&mut nested)
    .unwrap();
    let vendor = Elements::parse(&nested[..nested_length]).unwrap();
    let mut bytes = [0u8; 64];
    let length = CapabilityList::encode(
        &[InfoId::CAPABILITY_LIST, InfoId::NAI_REALM],
        vendor,
        &mut bytes,
    )
    .unwrap();
    let Value::CapabilityList(list) = Elements::parse(&bytes[..length])
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .value()
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        list.ids().collect::<std::vec::Vec<_>>(),
        [InfoId::CAPABILITY_LIST, InfoId::NAI_REALM]
    );
    assert_eq!(list.vendor, vendor);
    let records = [NetworkAuthentication {
        kind: network_authentication_kind::HTTP_REDIRECT,
        redirect_uri: b"https://example",
    }];
    let length = NetworkAuthentications::encode(&records, &mut bytes).unwrap();
    assert_eq!(
        NetworkAuthentications::parse(&bytes[..length])
            .unwrap()
            .iter()
            .next(),
        Some(records[0])
    );
    let mut short = [0x55; 1];
    assert!(NetworkAuthentications::encode(&records, &mut short).is_err());
    assert_eq!(short, [0x55]);
    let invalid = NetworkAuthentication {
        kind: network_authentication_kind::DNS_REDIRECT,
        ..records[0]
    };
    assert_eq!(
        NetworkAuthentications::encode(&[invalid], &mut bytes),
        Err(WireError::InvalidValue)
    );
    assert_eq!(
        NetworkAuthentications::parse(&[
            network_authentication_kind::ONLINE_ENROLLMENT,
            1,
            0,
            b'a'
        ]),
        Err(WireError::InvalidValue)
    );
    let length = Strings::encode(&[b"example", b"org"], &mut bytes).unwrap();
    let names = Strings::parse(&bytes[..length]).unwrap();
    assert_eq!(
        names.iter().collect::<std::vec::Vec<_>>(),
        [&b"example"[..], &b"org"[..]]
    );
    let venue = Venue {
        group: 1,
        kind: 1,
        names: Names::parse(&[]).unwrap(),
    };
    let length = venue.encode(&mut bytes).unwrap();
    assert_eq!(
        Element {
            id: InfoId::VENUE_NAME,
            body: &bytes[..length]
        }
        .value(),
        Ok(Value::Venue(venue))
    );
}
