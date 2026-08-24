use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::{fs, path::Path};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("ESP32-S31 host remains under verification/vendor/targets")
}

fn rust_files(root: &Path, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            rust_files(&path, output);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            output.push(path);
        }
    }
}

fn named_files(root: &Path, name: &str, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            named_files(&path, name, output);
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            output.push(path);
        }
    }
}

fn rust_source_tree(root: &Path) -> String {
    let mut files = Vec::new();
    if root.is_dir() {
        rust_files(root, &mut files);
    } else {
        files.push(root.to_path_buf());
    }
    files.sort();
    files
        .into_iter()
        .map(|path| fs::read_to_string(&path).expect("read Rust architecture owner"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn toml_document(path: &Path) -> toml_edit::DocumentMut {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .parse()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn rust_struct_body<'source>(source: &'source str, name: &str) -> &'source str {
    let header = format!("pub struct {name} {{");
    let header_start = source
        .find(&header)
        .unwrap_or_else(|| panic!("generated raw PAC lacks `{header}`"));
    let open = header_start + header.len() - 1;
    let mut depth = 1_usize;
    for (offset, character) in source[open + 1..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + 1 + offset];
                }
            }
            _ => {}
        }
    }
    panic!("generated raw PAC has an unterminated `{name}` owner")
}

#[test]
fn target_declares_exhaustive_raw_pac_ownership_partitions() {
    let repository = repository_root();
    let target = repository.join("verification/vendor/targets/esp32s31");
    let api_path = target.join("registers/api.toml");
    let api_source = fs::read_to_string(&api_path).expect("read target PAC API pack");
    let api = toml_document(&api_path);
    assert_eq!(api["schema"].as_integer(), Some(3));
    assert!(
        !api_source.contains("peripheral-ownership"),
        "the removed inferred-ownership option must not return"
    );

    let partitions = api["ownership-partitions"]
        .as_array_of_tables()
        .expect("schema-3 target pack must declare ownership partitions");
    let expected = [
        ("WifiMacPeripherals", "wifi_mac", 50_usize),
        ("WifiInterruptPeripherals", "wifi_interrupts", 2),
        ("RadioPhyPeripherals", "radio_phy", 18),
        ("CoexistencePeripherals", "coexistence", 4),
        ("BluetoothControllerPeripherals", "bluetooth", 15),
        ("BluetoothInterruptPeripherals", "bluetooth_interrupts", 1),
        ("SharedRadioPeripherals", "shared_radio", 4),
    ];
    let actual = partitions
        .iter()
        .map(|partition| {
            let name = partition["name"]
                .as_str()
                .expect("ownership partition name");
            let member = partition["member"]
                .as_str()
                .expect("ownership partition member");
            let count = partition["peripherals"]
                .as_array()
                .expect("ownership partition peripherals")
                .len();
            (name, member, count)
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);

    let model_path = target.join("registers/device.toml");
    let model = toml_document(&model_path);
    let fragments = model["fragments"]
        .as_array()
        .expect("register model fragment list");
    let mut model_peripherals = BTreeSet::new();
    for fragment in fragments {
        let relative = fragment.as_str().expect("register model fragment path");
        let document = toml_document(&target.join("registers").join(relative));
        for peripheral in document["peripherals"]
            .as_array_of_tables()
            .expect("register model peripheral declarations")
        {
            let name = peripheral["name"]
                .as_str()
                .expect("register model peripheral name");
            assert!(
                model_peripherals.insert(name.to_owned()),
                "register model repeats peripheral {name}"
            );
        }
    }

    let mut declared_owner = BTreeMap::new();
    for partition in partitions {
        let owner = partition["name"]
            .as_str()
            .expect("ownership partition name");
        let peripherals = partition["peripherals"]
            .as_array()
            .expect("ownership partition peripherals");
        assert!(
            !peripherals.is_empty(),
            "ownership partition {owner} must not be empty"
        );
        for peripheral in peripherals {
            let peripheral = peripheral.as_str().expect("owned peripheral name");
            assert!(
                declared_owner
                    .insert(peripheral.to_owned(), owner.to_owned())
                    .is_none(),
                "target assigns peripheral {peripheral} more than once"
            );
        }
    }
    let declared_peripherals = declared_owner.keys().cloned().collect::<BTreeSet<_>>();
    let missing = model_peripherals
        .difference(&declared_peripherals)
        .cloned()
        .collect::<Vec<_>>();
    let unknown = declared_peripherals
        .difference(&model_peripherals)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty() && unknown.is_empty(),
        "target PAC ownership is not an exact model cover; missing={missing:?}, unknown={unknown:?}"
    );

    let raw = fs::read_to_string(repository.join("driver/chips/esp32s31/pac-raw/src/lib.rs"))
        .expect("read generated raw PAC");
    for removed in [
        "pub struct RadioPeripherals",
        "pub struct InterruptPeripherals",
        "pub fn split(",
        "peripheral-ownership",
    ] {
        assert!(
            !raw.contains(removed),
            "generated raw PAC restored removed ownership surface `{removed}`"
        );
    }
    assert!(raw.contains("pub fn partition(peripherals: crate::Peripherals)"));
    let root = rust_struct_body(&raw, "PeripheralPartitions");
    for (name, member, _) in expected {
        assert!(
            root.contains(&format!("pub {member}: {name}")),
            "generated root does not expose target partition {member}: {name}"
        );
        let body = rust_struct_body(&raw, name);
        for (peripheral, owner) in &declared_owner {
            if owner == name {
                let field = peripheral.to_ascii_lowercase();
                assert!(
                    body.contains(&format!("pub {field}:")),
                    "generated {name} lacks target-owned peripheral {peripheral}"
                );
            }
        }
    }
}

#[test]
fn closed_chip_pac_is_the_only_driver_dependency_on_the_raw_pac() {
    let repository = repository_root();
    let driver = repository.join("driver");
    let closed_manifest = driver.join("chips/esp32s31/pac/Cargo.toml");
    let raw_manifest = driver.join("chips/esp32s31/pac-raw/Cargo.toml");
    let mut manifests = Vec::new();
    named_files(&driver, "Cargo.toml", &mut manifests);
    let violations = manifests
        .into_iter()
        .filter(|path| path != &closed_manifest && path != &raw_manifest)
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read driver manifest")
                .contains("open-esp-radio-esp32s31-pac-raw")
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "driver crates bypass the closed PAC: {violations:#?}"
    );

    let generated = fs::read_to_string(driver.join("chips/esp32s31/pac/src/generated.rs"))
        .expect("read generated closed-PAC domains");
    assert!(generated.contains("pub struct MacInterruptMask(u32);"));
    assert!(generated.contains("pub enum MacRxBeaconClearRequest"));
    assert!(!generated.contains("from_bits"));
    assert!(!generated.contains("open_esp_radio_esp32s31_pac_raw"));
    let raw = fs::read_to_string(driver.join("chips/esp32s31/pac-raw/src/lib.rs"))
        .expect("read generated raw PAC");
    assert!(!raw.contains("input & 0x00000000"));

    let mut closed_files = Vec::new();
    rust_files(&driver.join("chips/esp32s31/pac/src"), &mut closed_files);
    let direct_raw_imports = closed_files
        .iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("lib.rs"))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read closed PAC module")
                .contains("open_esp_radio_esp32s31_pac_raw")
        })
        .collect::<Vec<_>>();
    assert!(
        direct_raw_imports.is_empty(),
        "closed PAC modules bypass the single private raw-PAC alias: {direct_raw_imports:#?}"
    );
    let bypasses = closed_files
        .into_iter()
        .filter(|path| {
            !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("generated.rs" | "validation.rs")
            )
        })
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read closed PAC module");
            source.contains("::full_register_write::")
                || source.contains("::register_image_write::")
                || source.contains("::masked_register_modify::")
                || source.contains("::indexed_bit_set_modify::")
        })
        .collect::<Vec<_>>();
    assert!(
        bypasses.is_empty(),
        "closed PAC modules bypass generated transactions: {bypasses:#?}"
    );

    let facade = fs::read_to_string(driver.join("chips/esp32s31/pac/src/lib.rs"))
        .expect("read closed PAC facade");
    for rejected in [
        "use open_esp_radio_esp32s31_pac::svd;",
        "use open_esp_radio_esp32s31_pac::Register32;",
        "MacInterruptMask(0xdead_beef)",
        "WifiRadioRegisters::write_register",
    ] {
        assert!(
            facade.contains(rejected),
            "closed PAC must retain compile-fail coverage for `{rejected}`"
        );
    }
}

#[test]
fn only_esp32s31_hardware_boundaries_depend_on_the_closed_pac() {
    let repository = repository_root();
    let driver = repository.join("driver");
    let hal_manifest = driver.join("chips/esp32s31/hal/Cargo.toml");
    let bluetooth_manifest = driver.join("chips/esp32s31/bluetooth/Cargo.toml");
    let pac_manifest = driver.join("chips/esp32s31/pac/Cargo.toml");
    let raw_manifest = driver.join("chips/esp32s31/pac-raw/Cargo.toml");
    let permitted = [
        hal_manifest.as_path(),
        bluetooth_manifest.as_path(),
        pac_manifest.as_path(),
        raw_manifest.as_path(),
    ];
    let mut manifests = Vec::new();
    named_files(&driver, "Cargo.toml", &mut manifests);
    let violations = manifests
        .into_iter()
        .filter(|path| !permitted.contains(&path.as_path()))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read driver manifest")
                .contains("open-esp-radio-esp32s31-pac")
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "only the exact ESP32-S31 Wi-Fi and Bluetooth hardware boundaries may depend on the closed PAC: {violations:#?}"
    );

    let pac_manifest = fs::read_to_string(pac_manifest).expect("read closed PAC manifest");
    assert!(
        !pac_manifest.contains("test-register-catalog"),
        "closed PAC restored its removed external test register catalog"
    );
}

#[test]
fn powered_phy_capability_cannot_recover_the_pac_owner() {
    let repository = repository_root();
    let chip = repository.join("driver/chips/esp32s31");
    let facade = fs::read_to_string(chip.join("hal/src/lib.rs")).expect("read HAL facade");

    for removed in [
        "PhyRegisterAccess",
        "impl Deref for PhyHal",
        "impl DerefMut for PhyHal",
        "phy_parts_mut",
        "registers_mut",
    ] {
        assert!(
            !facade.contains(removed),
            "HAL restored removed generic PHY escape `{removed}`"
        );
    }

    let mut phy_files = Vec::new();
    rust_files(&chip.join("phy/src"), &mut phy_files);
    let violations = phy_files
        .into_iter()
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read PHY source");
            source.contains("open_esp_radio_esp32s31_pac")
                || source.contains("WifiRadioRegisters")
                || source.contains("WifiColdRegisters")
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "PHY source recovered a PAC owner outside HAL: {violations:#?}"
    );
}

#[test]
fn chip_hardware_boundaries_do_not_expose_restricted_pac_owners() {
    let chip = repository_root().join("driver/chips/esp32s31");
    let forbidden_owners = [
        "WifiRadioRegisters",
        "WifiColdRegisters",
        "BluetoothColdRegisters",
        "BluetoothTaskRegisters",
        "BluetoothInterruptSetup",
        "BluetoothInterruptRegisters",
    ];
    let mut files = Vec::new();
    rust_files(&chip.join("hal/src"), &mut files);
    rust_files(&chip.join("bluetooth/src"), &mut files);
    let violations = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read chip hardware boundary source");
            let mut public_declarations = Vec::new();
            let mut lines = source.lines();
            while let Some(line) = lines.next() {
                let trimmed = line.trim_start();
                let public_function = trimmed.starts_with("pub ") && trimmed.contains("fn ");
                if public_function {
                    let mut declaration = line.to_owned();
                    while !declaration.contains('{') && !declaration.contains(';') {
                        let Some(next) = lines.next() else {
                            break;
                        };
                        declaration.push_str(next);
                    }
                    public_declarations.push(declaration);
                } else if trimmed.starts_with("pub use ") {
                    let mut declaration = line.to_owned();
                    while !declaration.contains(';') {
                        let Some(next) = lines.next() else {
                            break;
                        };
                        declaration.push_str(next);
                    }
                    public_declarations.push(declaration);
                }
            }
            let exposed = public_declarations.into_iter().find(|declaration| {
                forbidden_owners
                    .iter()
                    .any(|owner| declaration.contains(owner))
            });
            exposed.map(|declaration| (path, declaration))
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "chip hardware boundaries expose restricted PAC owner types: {violations:#?}"
    );
}

#[test]
fn completed_trust_reset_inventory_cannot_return() {
    let repository = repository_root();
    assert!(
        !repository.join("TRUST_RESET_INVENTORY.md").exists(),
        "the retired narrative inventory must not return"
    );
    assert!(
        !repository
            .join("verification/vendor/targets/esp32s31/audits/verification-bindings.toml")
            .exists(),
        "the cutover inventory must be deleted after every decision is encoded in current manifests"
    );
}

#[test]
fn reviewed_sta_ap_and_coex_boundaries_hide_register_mechanics() {
    let repository = repository_root();
    let boundaries = [
        repository.join("driver/chips/esp32s31/hal/src/wifi_mac.rs"),
        repository.join("driver/chips/esp32s31/hal/src/coex.rs"),
        repository.join("driver/chips/esp32s31/wifi/mac/src/sta_ap_registers.rs"),
        repository.join("driver/chips/esp32s31/wifi/mac/src/coex_runtime.rs"),
    ];
    for path in boundaries {
        let source = fs::read_to_string(&path).expect("read reviewed register boundary");
        for forbidden in [
            "open_esp_radio_esp32s31_pac_raw",
            "Register32",
            "0x2010_",
            "write_volatile",
            "read_volatile",
        ] {
            assert!(
                !source.contains(forbidden),
                "reviewed boundary {} exposes {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn access_point_ht_width_is_one_end_to_end_typed_contract() {
    let repository = repository_root();
    let requests = fs::read_to_string(repository.join("driver/radio/src/requests.rs"))
        .expect("read public AP request");
    assert!(!requests.contains("channel.width() != WifiChannelWidth::Mhz20"));

    let ht = fs::read_to_string(repository.join("driver/wifi/ieee80211/src/ht.rs"))
        .expect("read portable HT profile");
    assert!(ht.contains("pub const fn ht_capability_ie(channel: WifiChannel)"));
    assert!(ht.contains("WifiChannelWidth::Mhz40Above"));
    assert!(ht.contains("WifiChannelWidth::Mhz40Below"));

    for path in [
        "driver/wifi/ieee80211/src/ap.rs",
        "driver/wifi/ieee80211/src/beacon.rs",
        "driver/chips/esp32s31/wifi/ap/src/engine.rs",
    ] {
        let source = fs::read_to_string(repository.join(path)).expect("read AP HT owner");
        assert!(!source.contains("write_ht20_association_response"));
        assert!(!source.contains("write_wpa2_ht20_beacon"));
    }

    let runtime = fs::read_to_string(
        repository.join("driver/integration/esp32s31/embassy-wifi/src/supervisor/access_point.rs"),
    )
    .expect("read AP supervisor");
    assert!(runtime.contains("lower_wifi_channel(requested_channel)"));
    assert!(runtime.contains("lowered_channel.channel_or_frequency"));
    assert!(runtime.contains("lowered_channel.cbw"));

    let network_tx = fs::read_to_string(
        repository
            .join("driver/adapters/embassy/esp32s31-wifi/src/roles/access_point/network_tx.rs"),
    )
    .expect("read AP network TX");
    assert!(network_tx.contains(".aggregate_admission("));
    assert!(!network_tx.contains(".peer_ht_rate("));
    assert!(!network_tx.contains("HtChannelWidth::Mhz20"));
    let ap_mac = fs::read_to_string(repository.join("driver/chips/esp32s31/wifi/ap/src/mac.rs"))
        .expect("read AP MAC policy owner");
    assert!(ap_mac.contains("pub fn aggregate_admission("));
    assert!(
        ap_mac.contains("let (binding, status) = self.engine.bind_aggregate_peer(peer).ok()?;")
    );
    assert!(ap_mac.contains("let rate = peer_ht_rate(self.engine.channel(), status.ht?)?;"));

    let ap_engine =
        fs::read_to_string(repository.join("driver/chips/esp32s31/wifi/ap/src/engine.rs"))
            .expect("read AP engine");
    assert!(ap_engine.contains("pub struct Esp32s31ApAggregateBinding"));
    assert!(ap_engine.contains(".bound_peer_status(binding.peer)"));

    let protocol = fs::read_to_string(repository.join("hil/protocol/src/message.rs"))
        .expect("read HIL AP request");
    assert!(protocol.contains("pub channel_width: WifiChannelWidth"));
}

#[test]
fn lmac_and_datapath_boundaries_do_not_recover_role_policy_or_legacy_runners() {
    let repository = repository_root();
    let chip_wifi = repository.join("driver/chips/esp32s31/wifi");
    let adapter = repository.join("driver/adapters/embassy/esp32s31-wifi/src");

    assert!(
        !adapter.join("connected_runner.rs").exists() && !adapter.join("connected_runner").exists(),
        "the removed ConnectedRunner compatibility surface must not return"
    );
    assert!(
        !adapter.join("connected_services.rs").exists()
            && adapter.join("datapath/services.rs").is_file(),
        "role-neutral datapath services must not return to a STA-connected module"
    );
    let access_point = format!(
        "{}\n{}",
        rust_source_tree(&adapter.join("roles/access_point.rs")),
        rust_source_tree(&adapter.join("roles/access_point")),
    );
    assert!(
        access_point.contains("DatapathRunner::new")
            && !access_point.contains("ApDataPlaneArbiter")
            && !access_point.contains("wait_for_ap_work"),
        "AP must use the role-neutral datapath owner without a parallel scheduler"
    );
    let access_point_datapath = fs::read_to_string(adapter.join("roles/access_point/datapath.rs"))
        .expect("read AP datapath binding");
    let compact_access_point_datapath = access_point_datapath
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(
        compact_access_point_datapath.contains(".network_tx.start(")
            && compact_access_point_datapath.contains(".network_tx.service(")
            && compact_access_point_datapath.contains(".network_tx.wait_deadline("),
        "the AP datapath binding must delegate the complete network TX transaction"
    );
    for forbidden in [
        ".mac.engine(",
        ".mac.try_aggregate_adapter(",
        ".aggregate.active_mut(",
        "HtMcs::",
        "MAC_INT_TX_COMPLETE",
        "MAC_INT_TX_TIMEOUT",
        "MAC_INT_COLLISION",
    ] {
        assert!(
            !compact_access_point_datapath.contains(forbidden),
            "the AP datapath binding recovered role TX policy through {forbidden}"
        );
    }
    assert!(
        !chip_wifi.join("mac/src/connected_rx.rs").exists()
            && !chip_wifi.join("mac/src/sta_ap_lifecycle.rs").exists(),
        "STA dispatch and datapath lifecycle policy must not return to LMAC"
    );
    assert!(
        chip_wifi.join("src/cooperative_hardware.rs").exists()
            && !chip_wifi.join("sta/src/cooperative_hardware.rs").exists(),
        "the AP/STA cooperative register facade must remain role-neutral without a legacy STA path"
    );
    assert!(
        chip_wifi.join("src/protected_data_rx.rs").is_file(),
        "AP/STA protected data validation must retain one role-neutral implementation"
    );
    let ap_rx = fs::read_to_string(chip_wifi.join("ap/src/rx.rs")).expect("read AP RX");
    let sta_rx =
        fs::read_to_string(chip_wifi.join("sta/src/connected_rx.rs")).expect("read STA RX");
    for source in [&ap_rx, &sta_rx] {
        assert!(source.contains("view_protected_data"));
        assert!(!source.contains("view_ccmp_data"));
        assert!(!source.contains("decapsulate_data_frames"));
    }
    let ap_datapath = fs::read_to_string(adapter.join("roles/access_point/datapath.rs"))
        .expect("read AP network RX publication");
    assert!(ap_datapath.contains("commit_rx_batch_record"));
    assert!(!ap_datapath.contains("ethernet_frames_dropped_network_backpressure"));
    assert!(
        !adapter.join("aggregate_tx.rs").exists() && !adapter.join("aggregate_tx").exists(),
        "the removed role-ambiguous aggregate_tx module must not return"
    );
    let station_tx =
        fs::read_to_string(adapter.join("roles/station/tx.rs")).expect("read STA aggregate TX");
    let ampdu_resources = fs::read_to_string(adapter.join("datapath/tx/resources.rs"))
        .expect("read role-neutral A-MPDU resources");
    assert!(
        !station_tx.contains("struct AggregateTxResources")
            && ampdu_resources.contains("pub struct AggregateTxResources"),
        "AP/STA A-MPDU arenas must remain owned by the role-neutral resource module"
    );

    let mut mac_sources = Vec::new();
    rust_files(&chip_wifi.join("mac/src"), &mut mac_sources);
    let mac_violations = mac_sources
        .into_iter()
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read LMAC source");
            source.contains("open_esp_radio_esp32s31_wifi_sta")
                || source.contains("open_esp_radio_esp32s31_wifi_ap")
                || source.contains("open_esp_radio_wifi_sta")
                || source.contains("open_esp_radio_wifi_ap")
        })
        .collect::<Vec<_>>();
    assert!(
        mac_violations.is_empty(),
        "LMAC acquired role-policy imports: {mac_violations:#?}"
    );
    let sta_link_hardware = fs::read_to_string(chip_wifi.join("mac/src/sta_link_policy.rs"))
        .expect("read STA link hardware boundary");
    assert!(!sta_link_hardware.contains("ScanRecord"));
    assert!(!sta_link_hardware.contains("AssociationResponse"));
    assert!(chip_wifi.join("sta/src/peer_policy.rs").is_file());

    let mut datapath_sources = vec![adapter.join("datapath/mod.rs")];
    rust_files(&adapter.join("datapath"), &mut datapath_sources);
    let datapath_violations = datapath_sources
        .into_iter()
        .filter(|path| {
            if path.file_name().is_some_and(|name| name == "tests.rs")
                || path
                    .components()
                    .any(|component| component.as_os_str() == "tests")
            {
                return false;
            }
            let source = fs::read_to_string(path).expect("read datapath source");
            source.contains("open_esp_radio_esp32s31_wifi_sta")
                || source.contains("open_esp_radio_esp32s31_wifi_ap")
                || source.contains("ConnectedDisconnectReason")
        })
        .collect::<Vec<_>>();
    assert!(
        datapath_violations.is_empty(),
        "role-neutral datapath acquired role-policy imports: {datapath_violations:#?}"
    );

    let integration = repository.join("driver/integration/esp32s31/embassy-wifi/src");
    let resources = fs::read_to_string(integration.join("radio_resources.rs"))
        .expect("read role-neutral integration resources");
    for owner in [
        "RadioTxBacking",
        "RadioAmpduStorage",
        "WifiNetworkResources",
    ] {
        assert!(resources.contains(owner), "radio resources lost {owner}");
    }
    let connected = fs::read_to_string(integration.join("supervisor/station.rs"))
        .expect("read connected station supervisor");
    for stale in [
        "ConnectedTxBacking",
        "ConnectedAmpduStorage",
        "pub type StationNetwork",
    ] {
        assert!(
            !connected.contains(stale),
            "station composition restored stale shared-resource name {stale}"
        );
    }
}

#[test]
fn production_wifi_supervisor_keeps_physical_and_role_owners_separate() {
    let repository = repository_root();
    let supervisor = fs::read_to_string(
        repository.join("driver/integration/esp32s31/embassy-wifi/src/composition/supervisor.rs"),
    )
    .expect("read ESP32-S31 physical Wi-Fi supervisor composition");
    assert!(supervisor.contains("pub physical: H"));

    let runtime = rust_source_tree(
        &repository.join("driver/integration/esp32s31/embassy-wifi/src/supervisor"),
    );
    let access_point = fs::read_to_string(
        repository.join("driver/integration/esp32s31/embassy-wifi/src/supervisor/access_point.rs"),
    )
    .expect("read production AP supervisor");

    for removed in [
        "ProductionAccessPointStationResources",
        "try_prepare_access_point_station_resources",
        "restore_station_resources_after_access_point",
        "parked_monitor",
        "RefCell<Option<ProductionMonitorResources>>",
    ] {
        assert!(
            !supervisor.contains(removed)
                && !runtime.contains(removed)
                && !access_point.contains(removed),
            "removed compatibility owner {removed} must not return"
        );
    }
    assert!(access_point.contains("pub(super) struct ProductionWifiPhysicalResources"));
    assert!(access_point.contains("pub(super) struct ProductionStationRoleResources"));
    assert!(access_point.contains("join_station_activation_resources"));
    assert!(access_point.contains("try_split_wifi_stopped_resources"));
}

#[test]
fn finite_rx_roles_share_one_frontier_owner_without_legacy_surfaces() {
    let adapter = repository_root().join("driver/adapters/embassy/esp32s31-wifi/src");

    for removed in [
        adapter.join("preconnected_rx.rs"),
        adapter.join("preconnected_rx"),
        adapter.join("rx_ring_owner.rs"),
    ] {
        assert!(
            !removed.exists(),
            "removed RX ownership surface returned: {}",
            removed.display()
        );
    }

    let mut sources = Vec::new();
    rust_files(&adapter, &mut sources);
    let stale = sources
        .iter()
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read RX adapter source");
            source.contains("preconnected_rx")
                || source.contains("PreconnectedRx")
                || source.contains("RxRingOwner")
                || source.contains("RxRingPhase")
        })
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "legacy or parallel RX ownership vocabulary returned: {stale:#?}"
    );

    let frontier = fs::read_to_string(adapter.join("datapath/rx/frontier.rs"))
        .expect("read canonical RX frontier facade");
    assert!(frontier.contains("pub use state::{"));
    for consumer in [
        "roles/scan/rx.rs",
        "roles/monitor/rx.rs",
        "roles/access_point/rx_pipeline.rs",
    ] {
        let source = fs::read_to_string(adapter.join(consumer)).expect("read finite RX consumer");
        assert!(
            source.contains("Esp32s31RxFrontier"),
            "{consumer} bypasses the canonical RX frontier owner"
        );
    }
}

#[test]
fn cargo_alias_selects_the_product_host_and_keeps_build_output_visible() {
    let config = fs::read_to_string(repository_root().join(".cargo/config.toml")).unwrap();
    let alias = config
        .lines()
        .find(|line| line.trim_start().starts_with("blobray ="))
        .expect("repository provides the documented Blobray alias");
    assert!(alias.contains("blobray-esp32s31"));
    assert!(alias.contains("run --profile blobray"));
    assert!(!alias.contains("run --quiet"));
}

#[test]
fn analysis_input_builder_owns_every_generated_project_role() {
    let builder =
        repository_root().join("verification/vendor/targets/esp32s31/build-analysis-inputs");
    let output = Command::new(&builder)
        .arg("--list-roles")
        .output()
        .expect("query analysis-input producer contract");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [
        "source-artifact:archive",
        "source-artifact:libpp",
        "source-artifact:libpp-replay",
        "source-artifact:libnet80211",
        "source-artifact:wifi-sta-ap-receive",
        "source-artifact:wifi-sta-lifecycle",
        "source-artifact:wifi-key-role",
        "source-artifact:coex",
        "source-artifact:btbb",
        "source-artifact:ble-controller",
        "rust-artifact",
        "rust-artifact:wifi-registers",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn ecosystem_pack_composes_the_shared_espressif_family_knowledge() {
    let vendor = repository_root().join("verification/vendor");
    let target = vendor.join("targets/esp32s31");
    let ecosystem = fs::read_to_string(target.join("ecosystem.toml")).expect("read ecosystem pack");
    assert!(ecosystem.contains("knowledge-packs"));
    assert!(ecosystem.contains("neutral-embedded.toml"));
    assert!(ecosystem.contains("../../knowledge/espressif/esp-idf.toml"));
    assert!(vendor.join("knowledge/espressif/esp-idf.toml").is_file());
    assert!(!target.join("semantics/embedded-platform.toml").exists());
}

#[test]
fn chip_knowledge_cannot_acquire_a_production_or_qualification_dependency() {
    let provider = repository_root().join("verification/vendor/targets/esp32s31/blobray-provider");
    let knowledge = fs::read_to_string(provider.join("knowledge/Cargo.toml")).unwrap();
    for forbidden in [
        "driver/chips",
        "open-esp-radio-esp32s31",
        "qualification-check",
        "qualification/",
    ] {
        assert!(
            !knowledge.contains(forbidden),
            "chip knowledge provider acquired forbidden dependency {forbidden}"
        );
    }

    assert!(
        !provider.join("verification").exists(),
        "compiled target code must not own comparison verdicts"
    );

    assert!(
        knowledge.contains("open-radio-vendor-addon-c")
            && knowledge.contains("open-radio-vendor-addon-esp-idf"),
        "the target provider must compose language and ecosystem semantics explicitly"
    );
}

#[test]
fn removed_provider_and_provenance_paths_do_not_return() {
    let repository = repository_root();
    let provider = repository.join("verification/vendor/targets/esp32s31/blobray-provider");
    assert!(!provider.join("semantic").exists());
    assert!(!provider.join("verification/src/qualification").exists());
    assert!(!provider.join("verification").exists());
    assert!(!provider.join("verification/src/semantic_replay").exists());

    let evidence = repository.join("verification/vendor/targets/esp32s31/registers/evidence");
    assert!(!evidence.join("migration.toml").exists());
    assert!(evidence.join("promoted-driver-provenance.toml").is_file());

    let mut production = Vec::new();
    rust_files(&repository.join("driver"), &mut production);
    let stale = production
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read production source")
                .contains("migration/")
        })
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "production source cites removed migration paths: {stale:#?}"
    );
}

#[test]
fn channel_lifecycle_reaches_the_pac_only_through_the_hal() {
    let repository = repository_root();
    let driver = repository.join("driver/chips/esp32s31");
    for manifest in [
        driver.join("phy/Cargo.toml"),
        driver.join("wifi/Cargo.toml"),
    ] {
        let input = fs::read_to_string(&manifest).expect("read role-neutral manifest");
        let dependencies = input
            .split("[dev-dependencies]")
            .next()
            .expect("manifest prefix");
        assert!(
            !dependencies.contains("open-esp-radio-esp32s31-pac"),
            "{} bypasses the HAL in production dependencies",
            manifest.display()
        );
    }

    let target_port =
        fs::read_to_string(driver.join("phy/src/target_port.rs")).expect("read PHY target port");
    assert!(target_port.contains("channel::RadioChannelHal"));
    assert!(!target_port.contains("request_mac_channel_stop_without_power_save"));

    let cold_start = fs::read_to_string(driver.join("wifi/src/cold_start.rs"))
        .expect("read production cold-start channel path");
    assert!(cold_start.contains("channel_hal()"));
    assert!(cold_start.contains("select_phy_channel_with_hal"));
    assert!(!cold_start.contains("powered.parts_mut()"));

    let production_trace = fs::read_to_string(
        repository
            .join("verification/vendor/targets/esp32s31/probes/library/src/production_trace.rs"),
    )
    .expect("read exact production trace wrapper");
    assert!(production_trace.contains("Radio::claim_for_validation(platform)"));
    assert!(production_trace.contains("channel_hal()"));
    assert!(!production_trace.contains("pac::validation"));
    assert!(!production_trace.contains("WifiRadioRegisters"));
    assert!(!production_trace.contains("WifiColdRegisters"));

    let capability =
        fs::read_to_string(driver.join("hal/src/channel.rs")).expect("read channel HAL capability");
    assert!(!capability.contains("pub fn registers_mut"));
    assert!(!capability.contains("pub fn into_registers"));
}

#[test]
fn compiled_probe_operations_do_not_call_pac_validation_directly() {
    let repository = repository_root();
    let probe = fs::read_to_string(
        repository.join("verification/vendor/targets/esp32s31/probes/library/src/lib.rs"),
    )
    .expect("read compiled probe library");
    assert!(
        !probe.contains("open_esp_radio_esp32s31_pac::validation"),
        "compiled operations must acquire validation ownership through HAL"
    );
    for retired_prefix in [
        "open_libpp_coex_trace_",
        "open_libpp_power_trace_",
        "open_libpp_power_tsf_trace_",
        "open_btbb_coex_trace_",
        "open_coex_scheduler_trace_",
        "open_libpp_trace_wdev_process_fiq_mac_slice",
        "open_phy_trace_iq_est_enable",
        "open_libpp_tx_trace_hal_mac_txq_publish_owned",
        "open_libnet80211_trace_sta_join_state",
        "open_wifi_key_role_trace_wdev_insert_key_entry",
    ] {
        assert!(
            !probe.contains(retired_prefix),
            "quarantined probe entry returned: {retired_prefix}"
        );
    }

    let pac_validation =
        fs::read_to_string(repository.join("driver/chips/esp32s31/pac/src/validation.rs"))
            .expect("read PAC validation capability factory");
    let exported = pac_validation
        .lines()
        .filter(|line| line.trim_start().starts_with("pub fn "))
        .map(|line| {
            line.trim_start()
                .strip_prefix("pub fn ")
                .expect("filtered public function")
                .split('(')
                .next()
                .expect("public function name")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        exported,
        [
            "wifi_radio_registers",
            "mac_interrupt_setup",
            "mac_interrupt_registers",
            "mac_power_interrupt_registers",
            "bluetooth_interrupt_registers",
            "initialize_bluetooth_baseband_v2",
        ],
        "PAC validation surface changed without updating the exact compiled-probe contract"
    );

    let hal = repository.join("driver/chips/esp32s31/hal/src");
    let mut hal_files = Vec::new();
    rust_files(&hal, &mut hal_files);
    let direct_acquisition = hal_files
        .into_iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("lib.rs"))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read HAL source")
                .contains("open_esp_radio_esp32s31_pac::validation")
        })
        .collect::<Vec<_>>();
    assert!(
        direct_acquisition.is_empty(),
        "HAL modules bypass the single validation owner factory: {direct_acquisition:#?}"
    );
}

#[test]
fn polling_and_multi_register_channel_sequences_are_owned_by_the_hal() {
    let repository = repository_root();
    let pac = repository.join("driver/chips/esp32s31/pac/src");
    let hal = repository.join("driver/chips/esp32s31/hal/src");

    let cold_pac = fs::read_to_string(pac.join("mac_cold_start.rs")).unwrap();
    assert!(!cold_pac.contains("loop {"));
    assert!(!cold_pac.contains("sample_limit"));
    let mac_hal = fs::read_to_string(hal.join("wifi_mac.rs")).unwrap();
    assert!(mac_hal.contains("begin_cold_mac_handshake"));
    assert!(mac_hal.contains("sample_limit"));
    assert!(mac_hal.contains("resume_mac_channel_without_power_save"));
    assert!(mac_hal.contains("select_wifi_no_power_save_regdma_link"));
}

#[test]
fn rx_descriptor_projection_is_owned_by_the_hal() {
    let repository = repository_root();
    let pac =
        fs::read_to_string(repository.join("driver/chips/esp32s31/pac/src/mac_rx_dma.rs")).unwrap();
    assert!(!pac.contains("mac_rx_last_descriptor_low"));
    assert!(!pac.contains("mac_rx_next_descriptor_low"));

    let hal =
        fs::read_to_string(repository.join("driver/chips/esp32s31/hal/src/wifi_mac.rs")).unwrap();
    assert!(hal.contains("rx_last_descriptor_word() & 0x000f_ffff"));
    assert!(hal.contains("rx_next_descriptor_word() & 0x000f_ffff"));

    let validation =
        fs::read_to_string(repository.join("driver/chips/esp32s31/hal/src/validation.rs")).unwrap();
    assert!(validation.contains("wifi_mac_hal().rx_last_descriptor_word()"));
    assert!(validation.contains("wifi_mac_hal().rx_next_descriptor_word()"));
}

#[test]
fn radio_arena_escape_surface_is_frozen() {
    let repository = repository_root();
    assert!(
        !repository
            .join("driver/chips/esp32s31/wifi/src/register_arena.rs")
            .exists(),
        "runtime MMIO serialization belongs to the HAL"
    );
    let arena = fs::read_to_string(repository.join("driver/chips/esp32s31/hal/src/radio_arena.rs"))
        .unwrap();
    for forbidden in [
        "pub fn with_mut",
        "pub fn with_ref",
        "pub fn try_with_ref",
        "unsafe impl Sync",
        "unsafe impl Send",
    ] {
        assert!(
            !arena.contains(forbidden),
            "register arena widened its generic escape surface with {forbidden}"
        );
    }
    assert!(arena.contains("explicit serialization owner"));
    assert!(arena.contains("try_station_receive_policy_snapshot"));
    assert!(arena.contains("try_receive_statistics_snapshot"));
    assert!(arena.contains("try_receive_dma_snapshot"));
    assert!(arena.contains("try_configure_station_receive_policy"));
    assert!(arena.contains("try_noise_floor_dbm"));
    assert!(arena.contains("try_install_station_ccmp_entry"));
    assert!(arena.contains("try_clear_ccmp_entry"));
    assert!(arena.contains("try_ccmp_entry_is_valid"));
    assert!(arena.contains("try_channel_hal"));
    assert!(arena.contains("try_wifi_mac_hal"));

    // Neither the unique root lease nor its copyable access handle may expose
    // the published PAC owner. Runtime code receives only named operations or
    // narrow guards such as `WifiMacHal` and `RadioChannelHal`.
    assert_eq!(arena.matches("pub fn borrow").count(), 0);

    for path in [
        "driver/adapters/embassy/esp32s31-wifi/src/roles/scan/target.rs",
        "driver/adapters/embassy/esp32s31-wifi/src/roles/station/attempt/channel.rs",
    ] {
        let source = fs::read_to_string(repository.join(path)).unwrap();
        assert!(source.contains("switch_published_channel"));
        assert!(!source.contains("borrow_mut()"));
    }
    let connected = fs::read_to_string(
        repository.join("driver/integration/esp32s31/embassy-wifi/src/supervisor/station.rs"),
    )
    .unwrap();
    assert!(connected.contains("try_prepare_connected_sta_without_power_save"));
    assert!(!connected.contains("access.borrow_mut()"));
    assert!(!connected.contains(".borrow()\n            .rx_statistics_snapshot()"));
}

#[test]
fn esp32c5_fixture_composes_only_neutral_and_family_knowledge() {
    let target = repository_root().join("verification/vendor/targets/esp32c5");
    let ecosystem = fs::read_to_string(target.join("ecosystem.toml")).unwrap();
    assert!(ecosystem.contains("../../knowledge/espressif/esp-idf.toml"));
    assert!(!ecosystem.contains("knowledge-provider"));
    assert!(!ecosystem.contains("esp32s31"));

    let target_spec = fs::read_to_string(target.join("target.toml")).unwrap();
    assert!(!target_spec.contains("memory-map"));
    assert!(!target_spec.contains("0x"));

    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "project",
            "configure",
            "--project",
            target.join("vendor-project.toml").to_str().unwrap(),
            "--check",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("validate C5 portability fixture");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
