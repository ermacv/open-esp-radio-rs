use super::*;

const EXAMPLE: &str = include_str!("../../../../stand/stand.example.toml");

/// A board's port: `/dev/tty-<id>`, as if every board were attached.
fn attached(board: &oer_hil_stand_schema::Board) -> crate::Result<std::path::PathBuf> {
    Ok(format!("/dev/tty-{}", board.id).into())
}

fn load(path: &Path, chip: &str) -> crate::Result<LabConfig> {
    LabConfig::load_resolving(path, chip, &BoardChoice::default(), &attached)
}

/// The example with `edit` applied, written to a private file.
fn stand_file(edit: impl FnOnce(&mut toml::Value)) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut raw: toml::Value = toml::from_str(EXAMPLE).unwrap();
    edit(&mut raw);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(toml::to_string(&raw).unwrap().as_bytes())
        .unwrap();
    file
}

/// A second esp32s31 board on the bottom hub.
fn second_s31(raw: &mut toml::Value) {
    let board: toml::Value = toml::from_str(
        "id = \"s31-b\"\nusb-serial = \"30:ED:A0:00:00:02\"\nchip = \"esp32s31\"\n\
         radios = [\"wifi-2g4\"]\nroles = [\"dut\", \"peer\"]\n\
         port = { hub = \"rsh-bottom\", port = 1 }\nreset = [\"power\"]\n",
    )
    .unwrap();
    raw["board"].as_array_mut().unwrap().push(board);
}

#[test]
fn linux_fixture_requires_explicit_radio_and_network_settings() {
    use std::io::Write;
    let mut raw: toml::Value = toml::from_str(EXAMPLE).unwrap();
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
        assert_eq!(load(file.path(), "esp32s31").is_ok(), valid, "{key}");
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
            access_point_beacon: None,
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
        access_point_beacon: None,
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

    let mut config: toml::Value = toml::from_str(EXAMPLE).unwrap();
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
        let config = load(file.path(), "esp32s31").unwrap();
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
        assert!(load(file.path(), "esp32s31").is_err());
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
fn independent_observer_accepts_only_safe_identifiers_and_managed_ap() {
    use std::io::Write;
    let mut raw: toml::Value = toml::from_str(EXAMPLE).unwrap();
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
        assert_eq!(load(file.path(), "esp32s31").is_ok(), !invalid);
    }
    raw["station_fixture"] = toml::from_str("kind='external'\nphys=['ht40']\n").unwrap();
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(toml::to_string(&raw).unwrap().as_bytes())
        .unwrap();
    assert!(load(file.path(), "esp32s31").is_err());
}

#[test]
fn a_wifi_link_occupies_its_channel_and_secondary_channel() {
    let lab = LabConfig::for_test();
    let link = |phy| oer_hil_scenario::link::WifiLabUse {
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
fn the_device_under_test_and_the_peer_come_from_the_pool() {
    let file = stand_file(|_| {});
    let lab = load(file.path(), "esp32s31").unwrap();
    assert_eq!((lab.dut.id.as_str(), lab.chip()), ("s31-a", "esp32s31"));
    assert_eq!(lab.dut.serial, std::path::PathBuf::from("/dev/tty-s31-a"));
    assert_eq!(lab.cell_id(), "berlin-open-radio");
    // The peer is the other board; the device under test never is.
    let peer = lab.peer_resolving(&attached).unwrap();
    assert_eq!(
        peer,
        PeerBoardConfig::new("c5-a".into(), "esp32c5".into(), "/dev/tty-c5-a".into())
    );
    let lab = load(file.path(), "esp32c5").unwrap();
    assert_eq!(lab.dut.id, "c5-a");
    assert_eq!(lab.peer_resolving(&attached).unwrap().id, "s31-a");
    // The C5 peer images do not run on the S31 the pool leaves.
    let error = lab
        .peer_for_image("ieee802154-peer")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("`ieee802154-peer` targets esp32c5"),
        "{error}"
    );
    assert!(load(file.path(), "esp32h2").is_err());
}

#[test]
fn a_run_names_its_boards_when_the_pool_has_several() {
    let file = stand_file(second_s31);
    let several = load(file.path(), "esp32s31").err().unwrap().to_string();
    assert!(several.contains("s31-a, s31-b"), "{several}");
    let choice = BoardChoice {
        dut: Some("s31-b".into()),
        peer: None,
    };
    let lab = LabConfig::load_resolving(file.path(), "esp32s31", &choice, &attached).unwrap();
    assert_eq!(lab.dut.id, "s31-b");
    let several = lab.peer_resolving(&attached).unwrap_err().to_string();
    assert!(several.contains("--peer-board"), "{several}");
    let choice = BoardChoice {
        dut: Some("s31-b".into()),
        peer: Some("c5-a".into()),
    };
    let lab = LabConfig::load_resolving(file.path(), "esp32s31", &choice, &attached).unwrap();
    assert_eq!(lab.peer_resolving(&attached).unwrap().id, "c5-a");
    let choice = BoardChoice {
        dut: Some("s31-b".into()),
        peer: Some("s31-b".into()),
    };
    let lab = LabConfig::load_resolving(file.path(), "esp32s31", &choice, &attached).unwrap();
    assert!(
        lab.peer_resolving(&attached).is_err(),
        "the device under test is no peer"
    );
}

#[test]
fn a_board_without_a_chip_profile_fails_only_its_own_runs() {
    let file = stand_file(|raw| {
        let board: toml::Value = toml::from_str(
            "id = \"c6-a\"\nusb-serial = \"40:4C:CA:00:00:01\"\nchip = \"esp32c6\"\n\
             radios = [\"ieee802154\"]\nroles = [\"dut\", \"peer\"]\n\
             port = { hub = \"rsh-bottom\", port = 2 }\nreset = [\"power\"]\n",
        )
        .unwrap();
        raw["board"].as_array_mut().unwrap().push(board);
    });
    let error = load(file.path(), "esp32c6").err().unwrap().to_string();
    assert!(error.contains("chip `esp32c6` has no profile"), "{error}");
    let choice = BoardChoice {
        dut: None,
        peer: Some("c6-a".into()),
    };
    let lab = LabConfig::load_resolving(file.path(), "esp32s31", &choice, &attached).unwrap();
    assert_eq!(lab.peer_resolving(&attached).unwrap().chip, "esp32c6");
}

#[test]
fn a_peer_that_is_not_attached_fails_only_the_runs_that_use_it() {
    let file = stand_file(|_| {});
    let only_s31 = |board: &oer_hil_stand_schema::Board| -> crate::Result<std::path::PathBuf> {
        match board.id.as_str() {
            "s31-a" => Ok("/dev/ttyACM0".into()),
            other => Err(format!("board `{other}` is not attached").into()),
        }
    };
    let lab =
        LabConfig::load_resolving(file.path(), "esp32s31", &BoardChoice::default(), &only_s31)
            .unwrap();
    let error = lab.peer_resolving(&only_s31).unwrap().serial().unwrap_err();
    assert!(error.to_string().contains("not attached"), "{error}");
}

#[test]
fn the_lab_file_sections_are_gone() {
    for removed in ["lab", "duts", "peer"] {
        let file = stand_file(|raw| {
            raw.as_table_mut()
                .unwrap()
                .insert(removed.into(), toml::Value::Table(toml::Table::new()));
        });
        assert!(load(file.path(), "esp32s31").is_err(), "{removed}");
    }
}
