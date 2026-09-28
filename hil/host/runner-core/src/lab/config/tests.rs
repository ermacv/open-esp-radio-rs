use super::*;

#[test]
fn linux_fixture_requires_explicit_radio_and_network_settings() {
    use std::io::Write;
    let mut raw: toml::Value =
        toml::from_str(include_str!("../../../../../local.example.toml")).unwrap();
    raw["station_fixture"] = toml::from_str("kind='local-linux'\ninterface='wlan0'\nphys=['ht20','ht40','he20']\ncountry='DE'\nchannel=13\naddress='10.42.0.1/24'\n").unwrap();
    for (key, value, valid) in [
        ("country", toml::Value::String("DE".into()), true),
        ("country", toml::Value::String("D".into()), false),
        ("channel", toml::Value::Integer(14), false),
        ("address", toml::Value::String("10.42.0.1/32".into()), false),
        ("coexistence", toml::Value::String("respect".into()), true),
        (
            "coexistence",
            toml::Value::String("force-ht40".into()),
            true,
        ),
        ("coexistence", toml::Value::String("ignore".into()), false),
    ] {
        let mut candidate = raw.clone();
        candidate["station_fixture"]
            .as_table_mut()
            .unwrap()
            .insert(key.into(), value);
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml::to_string(&candidate).unwrap().as_bytes())
            .unwrap();
        assert_eq!(LabConfig::load(file.path()).is_ok(), valid, "{key}");
    }
}

#[test]
fn ap_scenarios_resolve_both_radio_roles_from_one_channel_geometry() {
    let lab = LabConfig::for_test();
    let mut tested = 0;
    for link in [None, Some(PhyExpectation::Ht20), Some(PhyExpectation::Ht40)] {
        let wifi = WifiLabUse {
            link,
            management_frame_protection: Default::default(),
            access_point_security: Default::default(),
            access_point: true,
        };
        let resolved = lab.resolve(wifi);
        let StationFixtureConfig::OpenWrt(config) = &resolved.station_fixture else {
            panic!("OpenWrt test lab");
        };
        assert_eq!(config.channel, resolved.access_point.channel());
        let expected = if resolved.fixture_phy(wifi) == PhyExpectation::Ht40 {
            40
        } else {
            20
        };
        assert_eq!(resolved.access_point.bandwidth_mhz(), expected);
        if expected == 40 {
            assert_eq!(
                config.ht40_above,
                resolved.access_point.channel_width() == WifiChannelWidth::Mhz40Above
            );
        }
        tested += 1;
    }
    let station = WifiLabUse {
        link: Some(PhyExpectation::He20),
        management_frame_protection: Default::default(),
        access_point_security: Default::default(),
        access_point: false,
    };
    assert_eq!(
        lab.resolve(station).access_point.channel_width(),
        lab.access_point.channel_width()
    );
    assert!(tested > 0);
    assert_eq!(
        lab.access_point.channel_width(),
        WifiChannelWidth::Mhz40Above
    );
}

fn openwrt_config(phys: &[&str]) -> tempfile::NamedTempFile {
    use std::io::Write;

    let mut config: toml::Value =
        toml::from_str(include_str!("../../../../../local.example.toml")).unwrap();
    config["station_fixture"]["phys"] = toml::Value::Array(
        phys.iter()
            .map(|phy| toml::Value::String((*phy).into()))
            .collect(),
    );
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(toml::to_string(&config).unwrap().as_bytes())
        .unwrap();
    file
}

#[test]
fn openwrt_accepts_each_declared_phy_and_multiple_profiles() {
    for phys in [
        &["ht20"][..],
        &["ht40"][..],
        &["he20"][..],
        &["ht20", "ht40", "he20"][..],
    ] {
        let file = openwrt_config(phys);
        let config = LabConfig::load(file.path()).unwrap();
        for phy in [
            PhyExpectation::Ht20,
            PhyExpectation::Ht40,
            PhyExpectation::He20,
        ] {
            assert_eq!(
                config.station_fixture.require_phy(phy).is_ok(),
                phys.contains(&phy.id()),
            );
        }
    }
}

#[test]
fn openwrt_rejects_empty_duplicate_and_unknown_phy_profiles() {
    for phys in [&[][..], &["ht40", "ht40"][..], &["unknown"][..]] {
        let file = openwrt_config(phys);
        assert!(LabConfig::load(file.path()).is_err());
    }
}

#[test]
fn parses_static_ipv4() {
    let parsed = parse_ipv4(RawIpv4Config::Static {
        address: "192.168.1.182/24".into(),
        gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
    })
    .unwrap();
    assert_eq!(
        parsed,
        NetworkIpv4Configuration::Static {
            address: [192, 168, 1, 182],
            prefix_length: 24,
            gateway: Some([192, 168, 1, 1]),
        }
    );
}

#[test]
fn rejects_unsafe_fixture_tokens() {
    assert!(validate_shell_token("iface", "phy0-ap0; reboot").is_err());
}

#[test]
fn physical_identity_is_stable_and_path_safe() {
    assert!(validate_identifier("lab.id", "berlin-s31-01").is_ok());
    assert!(validate_identifier("lab.id", "Berlin/S31").is_err());
    assert!(validate_identifier("device.id", "").is_err());
}

#[test]
fn independent_observer_accepts_only_safe_identifiers_and_managed_ap() {
    use std::io::Write;
    let mut raw: toml::Value =
        toml::from_str(include_str!("../../../../../local.example.toml")).unwrap();
    raw.as_table_mut().unwrap().insert(
        "air_observer".into(),
        toml::from_str("ssh_target='lab-observer'\nphy='phy0'\ninterface='observe0'\n").unwrap(),
    );
    for invalid in [false, true] {
        let mut candidate = raw.clone();
        if invalid {
            candidate["air_observer"]["phy"] = toml::Value::String("phy0; false".into());
        }
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml::to_string(&candidate).unwrap().as_bytes())
            .unwrap();
        assert_eq!(LabConfig::load(file.path()).is_ok(), !invalid);
    }
    raw["station_fixture"] = toml::from_str("kind='external'\nphys=['ht40']\n").unwrap();
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(toml::to_string(&raw).unwrap().as_bytes())
        .unwrap();
    assert!(LabConfig::load(file.path()).is_err());
}

#[test]
fn the_peer_board_is_optional_and_needs_an_identity() {
    use std::io::Write;
    let raw: toml::Value =
        toml::from_str(include_str!("../../../../../local.example.toml")).unwrap();
    let load = |value: &toml::Value| {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml::to_string(value).unwrap().as_bytes())
            .unwrap();
        LabConfig::load(file.path())
    };
    let lab = load(&raw).unwrap();
    assert_eq!(
        lab.peer,
        Some(PeerBoardConfig::new(
            String::from("esp32c5-peer-01"),
            std::path::PathBuf::from("/dev/ttyUSB0"),
        ))
    );
    let mut absent = raw.clone();
    absent.as_table_mut().unwrap().remove("peer");
    assert_eq!(load(&absent).unwrap().peer, None);
    let mut anonymous = raw.clone();
    anonymous["peer"]["id"] = toml::Value::String(String::from(" "));
    assert!(load(&anonymous).is_err());
    // The table's earlier name names the same board.
    let mut earlier = raw.clone();
    let table = earlier.as_table_mut().unwrap();
    let peer = table.remove("peer").unwrap();
    table.insert(String::from("ieee802154_peer"), peer);
    assert_eq!(load(&earlier).unwrap().peer, lab.peer);
}

#[test]
fn devices_are_named_by_serial_port_or_by_registered_board() {
    use std::io::Write;
    let raw: toml::Value =
        toml::from_str(include_str!("../../../../../local.example.toml")).unwrap();
    let resolve = |board: &str, chip: Option<&str>| -> crate::Result<std::path::PathBuf> {
        match (board, chip) {
            ("esp32s31", Some("esp32s31")) => Ok("/dev/ttyACM0".into()),
            ("esp32c5", None) => Ok("/dev/ttyACM1".into()),
            _ => Err(format!("unexpected {board} {chip:?}").into()),
        }
    };
    let load = |value: &toml::Value| {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml::to_string(value).unwrap().as_bytes())
            .unwrap();
        LabConfig::load_resolving(file.path(), &resolve)
    };
    let mut boards = raw.clone();
    let device = boards["device"].as_table_mut().unwrap();
    device.remove("serial");
    device.insert("board".into(), "esp32s31".into());
    let peer = boards["peer"].as_table_mut().unwrap();
    peer.remove("serial");
    peer.remove("id");
    peer.insert("board".into(), "esp32c5".into());
    let lab = load(&boards).unwrap();
    assert_eq!(lab.device.serial, std::path::PathBuf::from("/dev/ttyACM0"));
    assert_eq!(
        lab.peer,
        Some(PeerBoardConfig::new(
            String::from("esp32c5"),
            std::path::PathBuf::from("/dev/ttyACM1"),
        ))
    );
    // A peer board that is not attached fails only the runs that use it.
    let detached = |board: &str, chip: Option<&str>| -> crate::Result<std::path::PathBuf> {
        match (board, chip) {
            ("esp32s31", Some("esp32s31")) => Ok("/dev/ttyACM0".into()),
            _ => Err(format!("board `{board}` is not attached").into()),
        }
    };
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(toml::to_string(&boards).unwrap().as_bytes())
        .unwrap();
    let lab = LabConfig::load_resolving(file.path(), &detached).unwrap();
    assert_eq!(lab.device.serial, std::path::PathBuf::from("/dev/ttyACM0"));
    let error = lab.peer.unwrap().serial().unwrap_err();
    assert!(error.to_string().contains("not attached"), "{error}");
    let mut both = boards.clone();
    both["device"]
        .as_table_mut()
        .unwrap()
        .insert("serial".into(), "/dev/ttyACM0".into());
    assert!(load(&both).is_err());
    let mut neither = boards.clone();
    neither["device"].as_table_mut().unwrap().remove("board");
    assert!(load(&neither).is_err());
    let mut unknown = boards;
    unknown["device"]["board"] = "esp32c5".into();
    assert!(
        load(&unknown).is_err(),
        "the device must resolve as an esp32s31"
    );
}

#[test]
fn a_wifi_link_occupies_its_channel_and_secondary_channel() {
    let lab = LabConfig::for_test();
    let link = |phy| crate::lab::link::WifiLabUse {
        link: Some(phy),
        ..Default::default()
    };
    // Channel 6 (2437 MHz): 20 MHz wide, HT40 with its secondary above.
    assert_eq!(
        lab.wifi_range_khz(link(PhyExpectation::Ht20)),
        (2_426_000, 2_448_000)
    );
    assert_eq!(
        lab.wifi_range_khz(link(PhyExpectation::Ht40)),
        (2_426_000, 2_468_000)
    );
    // The IEEE 802.15.4 channel 15 (2424-2426 MHz) just touches channel 6.
    let peer = oer_hil_arbiter::spectrum::Spectrum::ieee802154(
        15,
        oer_hil_arbiter::spectrum::Need::None,
        oer_hil_arbiter::spectrum::Emits::None,
    );
    assert_eq!(peer.high_khz, 2_426_000);
}

#[test]
fn devices_under_test_are_keyed_by_chip_and_device_is_the_esp32s31() {
    use std::io::Write;
    let raw: toml::Value =
        toml::from_str(include_str!("../../../../../local.example.toml")).unwrap();
    let resolve = |board: &str, chip: Option<&str>| -> crate::Result<std::path::PathBuf> {
        match (board, chip) {
            ("esp32c5", Some("esp32c5")) => Ok("/dev/ttyACM1".into()),
            ("esp32c5", None) => Ok("/dev/ttyACM1".into()),
            ("gone", _) => Err("board `gone` is not attached".into()),
            _ => Err(format!("unexpected {board} {chip:?}").into()),
        }
    };
    let load = |value: &toml::Value| {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml::to_string(value).unwrap().as_bytes())
            .unwrap();
        LabConfig::load_resolving(file.path(), &resolve)
    };
    let target = |board: &str| {
        let mut table = toml::Table::new();
        table.insert("id".into(), "esp32c5-dut".into());
        table.insert("board".into(), board.into());
        toml::Value::Table(table)
    };
    // [device] is the esp32s31 and stays the default; another chip resolves
    // only when a run asks for it.
    let mut both = raw.clone();
    let mut targets = toml::Table::new();
    targets.insert("esp32c5".into(), target("esp32c5"));
    both.as_table_mut()
        .unwrap()
        .insert("targets".into(), toml::Value::Table(targets));
    let lab = load(&both).unwrap();
    assert_eq!(lab.target(), "esp32s31");
    assert_eq!(lab.targets(), ["esp32c5", "esp32s31"]);
    let esp32c5 = lab.for_target_resolving("esp32c5", &resolve).unwrap();
    assert_eq!(esp32c5.target(), "esp32c5");
    assert_eq!(esp32c5.device.id, "esp32c5-dut");
    assert_eq!(
        esp32c5.device.serial,
        std::path::PathBuf::from("/dev/ttyACM1")
    );
    assert!(lab.for_target_resolving("esp32h2", &resolve).is_err());
    // An unattached board of another chip does not fail the load.
    let mut absent = both.clone();
    absent["targets"]["esp32c5"]["board"] = "gone".into();
    let lab = load(&absent).unwrap();
    assert!(lab.for_target_resolving("esp32c5", &resolve).is_err());
    // [device] and [targets.esp32s31] name one device; an unknown chip is refused.
    let mut twice = both.clone();
    let device = twice["device"].clone();
    twice["targets"]
        .as_table_mut()
        .unwrap()
        .insert("esp32s31".into(), device);
    assert!(load(&twice).is_err());
    let mut unknown = both;
    unknown["targets"]
        .as_table_mut()
        .unwrap()
        .insert("esp32x9".into(), target("esp32c5"));
    assert!(load(&unknown).is_err());
}
