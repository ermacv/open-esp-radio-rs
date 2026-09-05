use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::{fs, path::Path};

#[cfg(unix)]
mod analysis_input_builder;

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("ESP32-S31 host remains under verification/vendor/targets")
}

fn toml_document(path: &Path) -> toml_edit::DocumentMut {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .parse()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn target_ownership_partitions_exactly_cover_the_register_model() {
    let repository = repository_root();
    let target = repository.join("verification/vendor/targets/esp32s31");
    let chip = repository.join("verification/vendor/chips/esp32s31");
    let api_path = target.join("registers/api.toml");
    let api = toml_document(&api_path);
    assert_eq!(api["schema"].as_integer(), Some(5));

    let partitions = api["ownership-partitions"]
        .as_array_of_tables()
        .expect("schema-5 target pack must declare ownership partitions");
    let model_path = chip.join("registers/device.toml");
    let model = toml_document(&model_path);
    let fragments = model["fragments"]
        .as_array()
        .expect("register model fragment list");
    let mut model_peripherals = BTreeSet::new();
    for fragment in fragments {
        let relative = fragment.as_str().expect("register model fragment path");
        let document = toml_document(&chip.join("registers").join(relative));
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
    let mut owner_names = BTreeSet::new();
    let mut owner_members = BTreeSet::new();
    for partition in partitions {
        let owner = partition["name"]
            .as_str()
            .expect("ownership partition name");
        let member = partition["member"]
            .as_str()
            .expect("ownership partition member");
        assert!(
            owner_names.insert(owner),
            "duplicate ownership name {owner}"
        );
        assert!(
            owner_members.insert(member),
            "duplicate ownership member {member}"
        );
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
}

#[test]
fn analysis_input_builder_declares_the_roles_consumed_by_project_verification() {
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
    let target = builder.parent().expect("analysis-input target directory");
    let project = toml_document(&target.join("vendor-project.toml"));
    let addon = project["verification-addon"]
        .as_str()
        .expect("project verification add-on reference");
    let verification = toml_document(&target.join(addon));
    let expected = verification["suites"]
        .as_array_of_tables()
        .expect("project verification suites")
        .iter()
        .map(|suite| {
            suite["rust-artifact-role"]
                .as_str()
                .expect("suite Rust artifact role")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert!(
        !expected.is_empty(),
        "project must consume generated Rust roles"
    );
    assert_eq!(actual, expected);
}

#[test]
fn ieee802154_vendor_scaffold_is_fail_closed_and_source_scoped() {
    let target = repository_root().join("verification/vendor/targets/esp32s31");

    let disposition = fs::read_to_string(target.join("dispositions/ieee802154.toml"))
        .expect("read IEEE 802.15.4 dispositions")
        .parse::<toml_edit::DocumentMut>()
        .expect("parse IEEE 802.15.4 dispositions");
    assert_eq!(
        disposition["default-disposition"].as_str(),
        Some("not-yet-ported")
    );
    assert_eq!(disposition["default-protocol"].as_str(), Some("unknown"));

    let actual_prefixes = disposition["protocol-prefixes"]
        .as_array_of_tables()
        .expect("IEEE 802.15.4 protocol prefixes")
        .iter()
        .map(|entry| {
            (
                entry["source"].as_str().expect("prefix source").to_owned(),
                entry["prefix"].as_str().expect("symbol prefix").to_owned(),
                entry["protocol"]
                    .as_str()
                    .expect("prefix protocol")
                    .to_owned(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    let expected_prefixes = [
        ("ieee802154", "esp_ieee802154_", "ieee802154"),
        ("btbb", "ieee802154_", "ieee802154"),
        ("btbb", "zb_", "ieee802154"),
        ("coex", "esp_coex_ieee802154_", "ieee802154"),
    ]
    .into_iter()
    .map(|(source, prefix, protocol)| (source.to_owned(), prefix.to_owned(), protocol.to_owned()))
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual_prefixes, expected_prefixes);

    let addon = fs::read_to_string(target.join("verification-addon.toml"))
        .expect("read verification add-on")
        .parse::<toml_edit::DocumentMut>()
        .expect("parse verification add-on");
    let suites = addon["suites"]
        .as_array_of_tables()
        .expect("verification suites");
    for (id, source, vendor_prefix, rust_prefix) in [
        (
            "ieee802154-btbb",
            "btbb",
            "ieee802154_",
            "open_ieee802154_btbb_trace_",
        ),
        ("ieee802154-zb", "btbb", "zb_", "open_ieee802154_zb_trace_"),
        (
            "ieee802154-coex",
            "coex",
            "esp_coex_ieee802154_",
            "open_ieee802154_coex_trace_",
        ),
    ] {
        let suite = suites
            .iter()
            .find(|suite| suite["id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("missing IEEE 802.15.4 suite {id}"));
        assert_eq!(suite["gate"].as_str(), Some("completion"));
        assert_eq!(suite["rust-artifact-role"].as_str(), Some("rust-artifact"));
        assert_eq!(suite["rust-prefix"].as_str(), Some(rust_prefix));
        assert!(
            suite["profiles"]
                .as_array()
                .is_some_and(|items| items.is_empty())
        );
        assert!(
            suite["baselines"]
                .as_array()
                .is_some_and(|items| items.is_empty())
        );
        let dispositions = suite["dispositions"]
            .as_array()
            .expect("suite dispositions");
        assert_eq!(dispositions.len(), 1);
        assert_eq!(
            dispositions.get(0).and_then(|value| value.as_str()),
            Some("dispositions/ieee802154.toml")
        );
        let vendor = suite["vendor"]
            .as_array_of_tables()
            .expect("suite vendor selection");
        assert_eq!(vendor.len(), 1);
        let vendor = vendor.get(0).expect("one suite vendor selection");
        assert_eq!(vendor["source"].as_str(), Some(source));
        assert_eq!(vendor["prefix"].as_str(), Some(vendor_prefix));
        assert!(vendor.get("all").is_none());
        assert!(vendor.get("symbols").is_none());
    }

    assert!(suites.iter().all(|suite| {
        suite["vendor"]
            .as_array_of_tables()
            .expect("suite vendor selection")
            .iter()
            .all(|vendor| vendor["source"].as_str() != Some("ieee802154"))
    }));

    let project = fs::read_to_string(target.join("vendor-project.toml"))
        .expect("read vendor project")
        .parse::<toml_edit::DocumentMut>()
        .expect("parse vendor project");
    let ir_profiles = project["analysis"]["ir"]
        .as_array_of_tables()
        .expect("project IR profiles");
    for (id, source, prefix) in [
        ("ieee802154-btbb", "btbb", "ieee802154_"),
        ("ieee802154-zb", "btbb", "zb_"),
        ("ieee802154-coex", "coex", "esp_coex_ieee802154_"),
    ] {
        let profile = ir_profiles
            .iter()
            .find(|profile| profile["id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("missing IEEE 802.15.4 IR profile {id}"));
        assert_eq!(profile["roots"].as_str(), Some("symbol-prefix"));
        assert_eq!(profile["symbol-prefix"].as_str(), Some(prefix));
        let sources = profile["sources"].as_array().expect("IR profile sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources.get(0).and_then(|value| value.as_str()),
            Some(source)
        );
    }

    let public_families = project["analysis"]["public-symbol-families"]
        .as_array_of_tables()
        .expect("explicit public symbol family coverage");
    let ble_controller = public_families
        .iter()
        .find(|family| family["id"].as_str() == Some("ble-public-controller"))
        .expect("BLE public controller coverage declaration");
    assert_eq!(ble_controller["source"].as_str(), Some("ble-controller"));
    assert_eq!(
        ble_controller["profile"].as_str(),
        Some("ble-controller-all")
    );
    let bredr_controller = public_families
        .iter()
        .find(|family| family["id"].as_str() == Some("bredr-public-controller"))
        .expect("BR/EDR public controller coverage declaration");
    assert_eq!(
        bredr_controller["source"].as_str(),
        Some("bredr-controller")
    );
    assert_eq!(
        bredr_controller["profile"].as_str(),
        Some("bredr-controller-all")
    );
    let bredr_profile = ir_profiles
        .iter()
        .find(|profile| profile["id"].as_str() == Some("bredr-controller-all"))
        .expect("BR/EDR controller IR profile");
    assert_eq!(bredr_profile["roots"].as_str(), Some("all"));
    assert_eq!(
        bredr_profile["sources"]
            .as_array()
            .and_then(|sources| sources.get(0))
            .and_then(|source| source.as_str()),
        Some("bredr-controller")
    );
    let controller = public_families
        .iter()
        .find(|family| family["id"].as_str() == Some("ieee802154-public-controller"))
        .expect("IEEE 802.15.4 public controller coverage declaration");
    assert_eq!(controller["source"].as_str(), Some("ieee802154"));
    assert_eq!(
        controller["symbol-prefix"].as_str(),
        Some("esp_ieee802154_")
    );
    assert_eq!(controller["disposition"].as_str(), Some("required"));
    assert_eq!(
        controller["profile"].as_str(),
        Some("ieee802154-controller")
    );

    let scopes = project["review"]["scopes"]
        .as_array_of_tables()
        .expect("project review scopes");
    let baseband = scopes
        .iter()
        .find(|scope| scope["id"].as_str() == Some("ieee802154-baseband-leaves"))
        .expect("IEEE 802.15.4 baseband review scope");
    assert!(
        baseband["roots"]
            .as_array()
            .expect("baseband roots")
            .iter()
            .all(|root| root.as_str().is_some_and(|root| {
                root.strip_prefix("btbb:").is_some_and(|symbol| {
                    symbol.starts_with("ieee802154_") || symbol.starts_with("zb_")
                })
            }))
    );
    let coex = scopes
        .iter()
        .find(|scope| scope["id"].as_str() == Some("ieee802154-coex-client"))
        .expect("IEEE 802.15.4 coexistence review scope");
    assert!(
        coex["roots"]
            .as_array()
            .expect("coexistence roots")
            .iter()
            .all(|root| root
                .as_str()
                .is_some_and(|root| root.starts_with("coex:esp_coex_ieee802154_")))
    );
}

#[test]
fn review_scopes_have_explicit_many_to_many_protocol_membership() {
    let target = repository_root().join("verification/vendor/targets/esp32s31");
    let project = toml_document(&target.join("vendor-project.toml"));
    let scopes = project["review"]["scopes"]
        .as_array_of_tables()
        .expect("project review scopes");

    let allowed = ["wifi", "bluetooth", "ble", "ieee802154", "coex", "shared"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut by_id = BTreeMap::<String, BTreeSet<String>>::new();
    for scope in scopes {
        let id = scope["id"].as_str().expect("review scope id");
        let protocols = scope["protocols"]
            .as_array()
            .unwrap_or_else(|| panic!("review scope {id} has no protocols array"));
        assert!(!protocols.is_empty(), "review scope {id} has no protocol");
        let memberships = protocols
            .iter()
            .map(|protocol| {
                let protocol = protocol
                    .as_str()
                    .unwrap_or_else(|| panic!("review scope {id} has a non-string protocol"));
                assert!(
                    allowed.contains(protocol),
                    "review scope {id} uses non-canonical protocol {protocol}"
                );
                protocol.to_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(memberships.len(), protocols.len(), "duplicates in {id}");
        assert!(by_id.insert(id.to_owned(), memberships).is_none());
    }

    let all_radios = allowed.iter().map(|value| (*value).to_owned()).collect();
    assert_eq!(by_id["phy-init"], all_radios);
    assert_eq!(
        by_id["phy-pll-tracking"],
        BTreeSet::from([
            "wifi".to_owned(),
            "bluetooth".to_owned(),
            "ble".to_owned(),
            "ieee802154".to_owned(),
            "shared".to_owned(),
        ])
    );
    assert_eq!(by_id["station-state"], BTreeSet::from(["wifi".to_owned()]));
    assert_eq!(
        by_id["ble-controller-lifecycle"],
        BTreeSet::from(["ble".to_owned()])
    );
    assert_eq!(
        by_id["ble-stack-lifecycle"],
        BTreeSet::from(["ble".to_owned()])
    );
    assert_eq!(
        by_id["btdm-task-lifecycle"],
        BTreeSet::from(["ble".to_owned(), "bluetooth".to_owned()])
    );
    assert_eq!(
        by_id["bredr-controller-lifecycle"],
        BTreeSet::from(["bluetooth".to_owned()])
    );
    assert_eq!(
        by_id["bredr-host-controller-interface"],
        BTreeSet::from(["bluetooth".to_owned()])
    );
    assert_eq!(
        by_id["btbb-coex-client"],
        BTreeSet::from(["ble".to_owned(), "bluetooth".to_owned(), "coex".to_owned()])
    );
    assert!(!by_id.contains_key("ble-runtime-lifecycle"));
    assert!(!by_id.contains_key("ble-coex-client"));
    assert_eq!(
        by_id["ieee802154-coex-client"],
        BTreeSet::from(["coex".to_owned(), "ieee802154".to_owned()])
    );
    for scope in [
        "coex-core",
        "coex-timer",
        "coex-timer-control",
        "coex-external-register-leaves",
        "coex-scheduler",
    ] {
        assert_eq!(
            by_id[scope],
            allowed.iter().map(|value| (*value).to_owned()).collect()
        );
    }
}

#[test]
fn ecosystem_pack_parses_shared_espressif_family_knowledge() {
    let vendor = repository_root().join("verification/vendor");
    let target = vendor.join("targets/esp32s31");
    let ecosystem = toml_document(&vendor.join("knowledge/espressif/radio.toml"));

    assert_eq!(ecosystem["schema"].as_integer(), Some(3));
    assert_eq!(
        ecosystem["knowledge-packs"]
            .as_array()
            .expect("knowledge packs")
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .collect::<Vec<_>>(),
        [
            "../../../../tools/blobray/catalogs/neutral-embedded.toml",
            "esp-idf.toml",
        ]
    );
    assert_eq!(
        ecosystem["capability-packs"]
            .as_array()
            .expect("capability packs")
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .collect::<Vec<_>>(),
        ["capabilities.toml"]
    );
    assert_eq!(
        ecosystem["interface-template-packs"]
            .as_array()
            .expect("interface template packs")
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .collect::<Vec<_>>(),
        ["interface-templates.toml"]
    );
    assert!(vendor.join("knowledge/espressif/esp-idf.toml").is_file());
    assert!(
        vendor
            .join("knowledge/espressif/capabilities.toml")
            .is_file()
    );
    assert!(!target.join("ecosystem.toml").exists());
    assert!(!target.join("semantics/embedded-platform.toml").exists());
}

#[test]
fn chip_geometry_is_reusable_and_project_facts_stay_sparse() {
    let vendor = repository_root().join("verification/vendor");
    let target = vendor.join("targets/esp32s31");
    let chip = vendor.join("chips/esp32s31");

    for relative in ["chip.toml", "memory.toml", "registers/device.toml"] {
        assert!(
            chip.join(relative).is_file(),
            "shared chip pack lacks {relative}"
        );
        assert!(
            !target.join(relative).exists(),
            "investigation target duplicates reusable chip input {relative}"
        );
    }
    assert!(chip.join("registers/peripherals").is_dir());
    assert!(!target.join("registers/peripherals").exists());

    let project = toml_document(&target.join("vendor-project.toml"));
    assert_eq!(project["schema"].as_integer(), Some(4));
    assert_eq!(
        project["chip-pack"].as_str(),
        Some("../../chips/esp32s31/chip.toml")
    );
    assert_eq!(
        project["analysis-provider"].as_str(),
        Some("esp32s31-radio-knowledge-v1")
    );
    assert_eq!(
        project["reviewed-knowledge"]["packs"]
            .as_array()
            .and_then(|packs| packs.get(0))
            .and_then(toml_edit::Value::as_str),
        Some("reviewed/project-facts.toml")
    );
    assert_eq!(
        project["reviewed-knowledge"]["default-pack"].as_str(),
        Some("reviewed/project-facts.toml")
    );
    assert!(
        project
            .get("applicability")
            .and_then(toml_edit::Item::as_table)
            .is_none_or(|applicability| !applicability.contains_key("artifacts")),
        "project manifest must derive exact artifact applicability from live run-spec inputs"
    );

    let chip_manifest = toml_document(&chip.join("chip.toml"));
    assert_eq!(chip_manifest["schema"].as_integer(), Some(3));
    assert_eq!(
        chip_manifest["knowledge-provider"].as_str(),
        Some("esp32s31-rev0-chip-knowledge-v1")
    );
    assert!(chip.join("blobray-provider/contracts").is_dir());
    assert!(chip.join("blobray-provider/knowledge").is_dir());
    assert!(target.join("blobray-provider/OWNERSHIP.md").is_file());

    let reviewed = toml_document(&target.join("reviewed/project-facts.toml"));
    assert_eq!(reviewed["schema"].as_integer(), Some(2));
    assert_eq!(
        reviewed["id"].as_str(),
        Some("esp32s31-radio-rev0-project-facts")
    );
    let assertions = reviewed["assertions"]
        .as_array_of_tables()
        .expect("reviewed assertions");
    let event_status = assertions
        .iter()
        .find(|assertion| assertion["id"].as_str() == Some("ieee802154.event-status.identity"))
        .expect("reviewed IEEE 802.15.4 event status identity");
    assert_eq!(
        event_status["subject"].as_str(),
        Some("register:esp32s31/cpu/0x20103064/32")
    );
    assert_eq!(event_status["kind"].as_str(), Some("register-identity"));
    assert_eq!(
        event_status["value"].as_str(),
        Some("IEEE802154_MAC.EVENT_STATUS")
    );
    assert!(
        assertions
            .iter()
            .any(|assertion| { assertion["kind"].as_str() == Some("hardware-write-semantics") })
    );
    assert!(assertions.iter().all(|assertion| {
        !matches!(
            assertion["kind"].as_str(),
            Some("register-declaration" | "register-name")
        )
    }));
    assert!(!target.join("reviewed/ieee802154.toml").exists());

    let register_model = toml_document(&chip.join("registers/device.toml"));
    assert_eq!(register_model["schema"].as_integer(), Some(3));
    assert_eq!(register_model["chip"].as_str(), Some("esp32s31"));
}

#[test]
fn esp32c5_fixture_configures_successfully() {
    let target = repository_root().join("verification/vendor/targets/esp32c5");
    let target_spec = toml_document(&target.join("target.toml"));
    assert!(target_spec.get("memory-map").is_none());

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

#[test]
fn public_interface_templates_keep_exact_s31_bindings_in_the_project_overlay() {
    let repository = repository_root();
    let template_path =
        repository.join("verification/vendor/knowledge/espressif/interface-templates.toml");
    let templates = toml_document(&template_path);
    assert_eq!(templates["schema"].as_integer(), Some(1));
    let public = templates["templates"]
        .as_array_of_tables()
        .expect("public interface templates");
    let public_by_id = public
        .iter()
        .map(|template| {
            let id = template["id"].as_str().unwrap();
            let provenance = template["provenance"].as_inline_table().unwrap();
            assert!(
                provenance["repository"]
                    .as_str()
                    .unwrap()
                    .starts_with("https://")
            );
            assert_eq!(provenance["revision"].as_str().unwrap().len(), 40);
            assert!(!provenance["path"].as_str().unwrap().is_empty());
            (id, template["slots"].as_array().unwrap().len())
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        public_by_id,
        BTreeMap::from([("esp-idf.coex-adapter-v2", 18), ("esp-idf.wifi-osi-v9", 61),])
    );

    let overlay_path =
        repository.join("verification/vendor/targets/esp32s31/interfaces/reviewed.toml");
    let overlay = toml_document(&overlay_path);
    assert_eq!(overlay["schema"].as_integer(), Some(3));
    let anchors = overlay["anchors"].as_array_of_tables().unwrap();
    let templated = anchors
        .iter()
        .filter(|anchor| anchor.get("template").is_some())
        .collect::<Vec<_>>();
    assert_eq!(templated.len(), 2);
    for anchor in templated {
        assert!(
            anchor
                .get("source")
                .and_then(toml_edit::Item::as_str)
                .is_some()
        );
        assert!(
            anchor
                .get("root-kind")
                .and_then(toml_edit::Item::as_str)
                .is_some()
        );
        assert!(anchor.get("slots").is_none());
        for key in [
            "layout-version",
            "pointer-width",
            "layout-size",
            "slot-stride",
        ] {
            assert!(anchor.get(key).is_none(), "template owns {key}");
        }
        let guards = anchor["guards"].as_array_of_tables().unwrap();
        assert_eq!(
            guards
                .iter()
                .filter(|guard| guard["kind"].as_str() == Some("artifact-sha256"))
                .count(),
            1
        );
        assert!(
            guards
                .iter()
                .any(|guard| guard["kind"].as_str() == Some("runtime-value"))
        );
        assert!(anchor["execution-contract"].as_str().is_some());
        let overrides = anchor["overrides"].as_array_of_tables().unwrap();
        let offsets = overrides
            .iter()
            .map(|overridden| {
                assert!(!overridden["reason"].as_str().unwrap().is_empty());
                overridden["offset"].as_integer().unwrap()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(offsets.len(), overrides.len());
    }
}
