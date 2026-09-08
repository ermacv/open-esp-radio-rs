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
    let catalog =
        crate::scenario::Catalog::load(&repository_root().unwrap().join("hil/scenarios")).unwrap();
    let mut tested = 0;
    for scenario in catalog.all() {
        if !matches!(
            scenario.workload,
            crate::scenario::Workload::AccessPoint { .. }
        ) {
            continue;
        }
        let resolved = lab.resolve_scenario(scenario);
        let StationFixtureConfig::OpenWrt(config) = &resolved.station_fixture else {
            panic!("OpenWrt test lab");
        };
        assert_eq!(config.channel, resolved.access_point.channel());
        let expected = if resolved.fixture_phy(scenario) == PhyExpectation::Ht40 {
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
