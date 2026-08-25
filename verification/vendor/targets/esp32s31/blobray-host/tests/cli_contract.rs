use std::{path::Path, process::Command};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("ESP32-S31 host remains under verification/vendor/targets")
}

fn project() -> std::path::PathBuf {
    repository_root().join("verification/vendor/targets/esp32s31/vendor-project.toml")
}

fn blobray() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray"));
    command
        .current_dir(repository_root())
        .env_remove("RUST_LOG");
    command
}

#[test]
fn removed_self_verdict_command_is_not_in_the_cli_grammar() {
    let output = blobray()
        .args(["advanced", "verify", "contract", "channel", "--project"])
        .arg(project())
        .args([
            "--vendor-artifact",
            "/missing/vendor-contract.elf",
            "--vendor-companion",
            "/missing/vendor-rom.elf",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("run removed command");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument 'contract'")
            || stderr.contains("unrecognized subcommand 'contract'")
    );
    assert!(!stderr.contains("No such file or directory"));
    assert!(!stderr.contains("provider"));
}

#[test]
fn checked_register_publications_are_typed_reports() {
    for (arguments, expected_status) in [
        (vec!["registers", "validate"], "valid"),
        (vec!["registers", "export-svd", "--check"], "verified"),
        (vec!["registers", "generate-pac-raw", "--check"], "verified"),
        (
            vec!["registers", "generate-bindings", "--check"],
            "verified",
        ),
    ] {
        let output = blobray()
            .args(arguments)
            .arg("--project")
            .arg(project())
            .args(["--format", "json", "--color", "never"])
            .output()
            .expect("run register publication check");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["status"], expected_status);
    }
}

#[test]
fn inspect_register_schema_seven_exposes_typed_validation_actions() {
    let inspect = |address: &str| {
        let output = blobray()
            .args(["inspect", "register", address, "--project"])
            .arg(project())
            .args(["--format", "json", "--color", "never"])
            .output()
            .expect("inspect register");
        assert!(
            output.status.success(),
            "{address} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };

    let owned = inspect("0x20103100");
    assert_eq!(owned["schema_version"], 7);
    assert_eq!(owned["register"]["review_status"], "unreviewed");
    assert_eq!(owned["review_draft"]["state"], "review-required");
    assert_eq!(owned["review_draft"]["completion_claim"], false);
    assert_eq!(
        owned["review_draft"]["finding_id"],
        "register-0x20103100-32"
    );
    assert!(
        owned["review_draft"]["destination"]
            .as_str()
            .unwrap()
            .ends_with("reviewed/project-facts.toml")
    );
    let raw = owned["review_draft"]["raw_toml"].as_str().unwrap();
    assert!(raw.parse::<toml_edit::DocumentMut>().is_ok());
    assert!(raw.contains("REVIEW_REQUIRED.register-identity"));
    assert!(raw.contains("subject = \"register:esp32s31/cpu/0x20103100/32\""));
    assert!(raw.contains("kind = \"register-identity\""));
    assert!(raw.contains("value = \"REVIEW_REQUIRED_REGION.REVIEW_REQUIRED_REGISTER_NAME\""));
    assert_eq!(raw.matches("[[assertions]]").count(), 1);
    assert!(!raw.contains("hardware-write-semantics"));
    assert_eq!(
        owned["recording"]["supported_register_facts"][0],
        "register-identity"
    );
    assert!(
        !owned["recording"]["supported_register_facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|kind| kind == "register-declaration" || kind == "register-name")
    );
    let actions = owned["review_draft"]["validation_actions"]
        .as_array()
        .unwrap();
    assert_eq!(actions.len(), 3);
    assert_eq!(
        &actions[0]["argv"].as_array().unwrap()[..3],
        serde_json::json!(["blobray", "registers", "validate"])
            .as_array()
            .unwrap()
    );
    assert_eq!(actions[0]["context"], "target");
    assert_eq!(
        &actions[1]["argv"].as_array().unwrap()[..3],
        serde_json::json!(["blobray", "project", "analyze"])
            .as_array()
            .unwrap()
    );
    assert_eq!(actions[1]["context"], "analysis");
    assert!(
        actions[2]["argv"]
            .as_array()
            .unwrap()
            .windows(2)
            .any(|pair| pair == ["--finding", "register-0x20103100-32"])
    );
    assert!(
        actions.iter().all(
            |action| action["argv"].as_array().unwrap().iter().any(|argument| {
                argument
                    .as_str()
                    .is_some_and(|value| value.ends_with("vendor-project.toml"))
            })
        )
    );
    assert!(owned["review_draft"].get("validation_commands").is_none());

    for (address, expected_state) in [
        ("0x2010f4a0", "ignored"),
        ("0x20100010", "reviewed"),
        ("0x20100000", "non-operational"),
        ("0x2010fcb0", "manual"),
    ] {
        let report = inspect(address);
        assert_eq!(report["schema_version"], 7);
        assert_eq!(report["register"]["review_status"], expected_state);
        assert!(report["review_draft"].is_null());
    }

    let event_status = inspect("0x20103064");
    assert_eq!(event_status["schema_version"], 7);
    assert_eq!(
        event_status["reviewed_assertions"]["subject"],
        "register:esp32s31/cpu/0x20103064/32"
    );
    assert_eq!(
        event_status["reviewed_assertions"]["completion_claim"],
        false
    );
    let assertions = event_status["reviewed_assertions"]["assertions"]
        .as_array()
        .unwrap();
    assert_eq!(assertions.len(), 2);
    assert_eq!(
        assertions
            .iter()
            .map(|assertion| assertion["id"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "ieee802154.event-status.identity",
            "ieee802154.event-status.write-semantics",
        ]
        .into()
    );
    assert!(assertions.iter().all(|assertion| {
        assertion["pack"] == "esp32s31-radio-rev0-project-facts"
            && assertion["subject"] == "register:esp32s31/cpu/0x20103064/32"
            && !assertion["kind"].as_str().unwrap().is_empty()
            && !assertion["evidence"].as_array().unwrap().is_empty()
    }));
    let identity = assertions
        .iter()
        .find(|assertion| assertion["id"] == "ieee802154.event-status.identity")
        .unwrap();
    assert_eq!(identity["kind"], "register-identity");
    assert_eq!(identity["value"], "IEEE802154_MAC.EVENT_STATUS");
    assert!(event_status["review_draft"].is_null());
}

#[test]
fn research_schema_fourteen_exact_finding_resolution_is_current_and_not_a_completion_verdict() {
    let lookup = |scope: &str, finding: &str| {
        let output = blobray()
            .args([
                "project",
                "research",
                "next",
                "--scope",
                scope,
                "--finding",
                finding,
                "--project",
            ])
            .arg(project())
            .args([
                "--format",
                "json",
                "--color",
                "never",
                "--progress",
                "never",
            ])
            .output()
            .expect("look up exact research finding");
        assert!(
            output.status.success(),
            "{finding} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };

    let open = lookup("ieee802154-baseband-leaves", "register-0x20103100-32");
    assert_eq!(open["schema_version"], 14);
    assert_eq!(open["completion_claim"], false);
    assert_eq!(open["finding_query"]["state"], "open");
    assert_eq!(open["finding_query"]["completion_claim"], false);
    assert_eq!(open["finding_query"]["historical_finding_claim"], false);
    assert_eq!(open["inventory"]["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        open["inventory"]["findings"][0]["id"],
        "register-0x20103100-32"
    );
    assert_eq!(
        open["inventory"]["findings"][0]["consumers"][0]["assertion_kinds"],
        serde_json::json!(["register-identity"])
    );
    assert_eq!(
        open["inventory"]["actions"][0]["next_action"]["argv"][0],
        "blobray"
    );
    assert!(
        open["inventory"]["actions"][0]
            .get("inspect_command")
            .is_none()
    );
    assert_eq!(
        open["inventory"]["findings"][0]["requery_action"]["context"],
        "analysis"
    );

    let input_not_observed = lookup("ieee802154-baseband-leaves", "register-0x20103064-32");
    assert_eq!(
        input_not_observed["finding_query"]["state"],
        "input-not-observed"
    );
    assert_eq!(
        input_not_observed["finding_query"]["resolution_evidence"]["subject"]["address"],
        0x20103064_u32
    );
    assert!(
        input_not_observed["inventory"]["findings"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let filtered = lookup("wifi-rx", "register-0x20103100-32");
    assert_eq!(filtered["finding_query"]["state"], "filtered-out");
    assert!(
        !filtered["finding_query"]["resolution_evidence"]["matching_scopes"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let missing = lookup("ieee802154-baseband-leaves", "register-not-current");
    assert_eq!(missing["schema_version"], 14);
    assert_eq!(missing["completion_claim"], false);
    assert_eq!(missing["finding_query"]["state"], "not-present");
    assert_eq!(missing["finding_query"]["completion_claim"], false);
    assert!(
        missing["finding_query"]["interpretation"]
            .as_str()
            .unwrap()
            .contains("not proof")
    );
    assert!(
        missing["inventory"]["findings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        missing["inventory"]["actions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn inspect_function_schema_fifteen_has_one_fail_closed_blocker_route() {
    let output = blobray()
        .args([
            "inspect",
            "function",
            "ble-controller:r_ble_controller_init",
            "--project",
        ])
        .arg(project())
        .args([
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("inspect stable BLE controller entry");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema_version"], 15);
    let blockers = document["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|semantic| semantic["blockers"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert!(!blockers.is_empty());
    for blocker in blockers {
        assert!(blocker.get("required_model").is_none());
        let route = &blocker["resolution_route"];
        assert!(!route["required_model"].as_str().unwrap().is_empty());
        assert!(!route["evidence_required"].as_array().unwrap().is_empty());
        assert_eq!(route["completion_predicate"]["root_id"], blocker["root_id"]);
        assert_eq!(
            route["closes_producer"].as_bool().unwrap(),
            route["producer_effect"] == "closes"
        );
        if matches!(
            route["owner"].as_str().unwrap(),
            "generic-backend" | "analysis-addon" | "unsupported"
        ) {
            assert!(route.get("destination").is_none());
            assert!(route.get("record_action").is_none());
        }
    }
}

#[test]
fn research_surfaces_are_protocol_exact_and_keep_inspection_visible() {
    let query = |arguments: &[&str]| {
        let output = blobray()
            .args(["project", "research", "next"])
            .args(arguments)
            .arg("--project")
            .arg(project())
            .args([
                "--limit",
                "4",
                "--format",
                "json",
                "--color",
                "never",
                "--progress",
                "never",
            ])
            .output()
            .expect("query protocol research surface");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };

    let ieee = query(&["--protocol", "ieee802154"]);
    let ieee_surface = ieee["inventory"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["kind"] == "analysis-surface")
        .expect("missing IEEE 802.15.4 surface is a typed finding");
    assert_eq!(
        ieee_surface["subject"]["surface"],
        "ieee802154-public-controller"
    );
    assert_eq!(ieee_surface["subject"]["state"], "missing-vendor-artifact");
    let ieee_finding_id = ieee_surface["id"].as_str().unwrap();
    assert_eq!(ieee["selection"]["steps"][0]["kind"], "prerequisite");
    assert!(
        ieee["selection"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["kind"] == "action")
    );
    let exact_ieee = query(&["--protocol", "ieee802154", "--finding", ieee_finding_id]);
    assert_eq!(exact_ieee["finding_query"]["state"], "open");
    assert_eq!(
        exact_ieee["inventory"]["findings"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        exact_ieee["inventory"]["findings"][0]["requery_action"]["context"],
        "analysis"
    );

    let ble = query(&["--protocol", "ble"]);
    assert!(
        ble["inventory"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["kind"] != "analysis-surface")
    );
    let ble_roots = ble["inventory"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["subject"]["kind"] == "analysis-root")
        .collect::<Vec<_>>();
    assert!(!ble_roots.is_empty());
    for finding in ble_roots {
        let route = &finding["blocker_resolution_route"];
        assert_eq!(route["required_model"], finding["knowledge_required"]);
        assert_eq!(route["evidence_required"], finding["evidence_required"]);
        assert_eq!(
            route["completion_predicate"]["root_id"],
            finding["subject"]["root_id"]
        );
    }

    let bluetooth = query(&["--protocol", "bluetooth"]);
    assert!(
        bluetooth["inventory"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["kind"] != "analysis-surface"),
        "the authenticated BR/EDR controller must not remain a missing analysis surface"
    );
    let analyzed_bluetooth = bluetooth["analyzed_scopes"]
        .as_array()
        .expect("Bluetooth analyzed scopes")
        .iter()
        .filter_map(|scope| scope.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(analyzed_bluetooth.contains("bredr-controller-lifecycle"));
    assert!(analyzed_bluetooth.contains("bredr-host-controller-interface"));

    let exact_scope = query(&["--scope", "ieee802154-baseband-leaves"]);
    assert!(
        exact_scope["inventory"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["kind"] != "analysis-surface")
    );
}

#[test]
fn checked_in_project_and_target_owned_review_packs_pass_doctor() {
    let output = blobray()
        .args(["project", "doctor", "--project"])
        .arg(project())
        .args([
            "--format",
            "json",
            "--color",
            "never",
            "--progress",
            "never",
        ])
        .output()
        .expect("validate the checked-in ESP32-S31 project");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema"], 4);
    assert_eq!(document["validation"]["depth"], "deep");
    assert_eq!(document["command"], "project doctor");
    assert!(matches!(
        document["status"].as_str(),
        Some("valid" | "valid-with-warnings")
    ));
    assert_eq!(document["project"]["id"], "esp32s31-radio-rev0");
    assert_eq!(document["target"]["id"], "esp32s31-rev0");
    assert!(
        document["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().all(|capability| {
                matches!(
                    capability["status"].as_str(),
                    Some("available" | "ready" | "valid" | "not-generated" | "facts-not-generated")
                ) || (capability["name"] == "verification-report"
                    && capability["status"] == "incomplete")
                    || (capability["name"] == "revision-workflow"
                        && capability["status"] == "baseline-missing")
                    || (capability["name"] == "reusable-capabilities"
                        && capability["status"] == "incomplete")
            }))
    );
}

#[test]
fn reusable_radio_capability_report_is_deterministic_and_fail_closed() {
    let evaluate = || {
        let output = blobray()
            .args(["advanced", "interfaces", "validate", "--project"])
            .arg(project())
            .args([
                "--format",
                "json",
                "--color",
                "never",
                "--progress",
                "never",
            ])
            .output()
            .expect("evaluate reusable interface capability rules");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };

    let first = evaluate();
    let second = evaluate();
    assert_eq!(first["status"], "valid-with-capability-gaps");
    assert_eq!(first["interface_template_packs"], 1);
    assert_eq!(first["interface_templates"], 2);
    assert_eq!(first["templated_anchors"], 2);
    assert_eq!(
        first["template_pack_ids"],
        serde_json::json!(["espressif.esp-idf.interface-templates"])
    );
    assert_eq!(
        first["templates"],
        serde_json::json!([
            {
                "id": "esp-idf.coex-adapter-v2",
                "repository": "https://github.com/esp-rs/esp-wifi-sys",
                "revision": "ff57fc7a50ef56a631e81ceed36b66ff8e2a21c4",
                "path": "c/headers/esp32s31/esp_coexist_adapter.h"
            },
            {
                "id": "esp-idf.wifi-osi-v9",
                "repository": "https://github.com/espressif/esp-idf",
                "revision": "08e0d30a74ad0bfd5a34933142b80f45619ee410",
                "path": "components/esp_wifi/include/esp_private/wifi_os_adapter.h"
            }
        ])
    );
    let templated_contracts = first["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|contract| {
            matches!(
                contract["anchor"].as_str(),
                Some("wifi-osi-v9" | "coex-adapter-v2")
            )
        })
        .map(|contract| {
            (
                contract["anchor"].as_str().unwrap(),
                (
                    contract["layout_version"].as_str().unwrap(),
                    (
                        contract["slots"].as_u64().unwrap(),
                        contract["template_overrides"].as_array().unwrap().len(),
                    ),
                ),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        templated_contracts,
        std::collections::BTreeMap::from([
            ("coex-adapter-v2", ("esp-idf-coex-adapter-v2", (18, 18))),
            ("wifi-osi-v9", ("esp-idf-wifi-osi-v9", (61, 59))),
        ])
    );
    assert!(
        first["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|contract| {
                contract["template_overrides"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .all(|overridden| {
                !overridden["reason"].as_str().unwrap().is_empty()
                    && !overridden["fields"].as_array().unwrap().is_empty()
            })
    );
    assert_eq!(first["capabilities"], second["capabilities"]);
    let capabilities = &first["capabilities"];
    assert_eq!(capabilities["schema"], 1);
    assert_eq!(capabilities["status"], "incomplete");
    assert_eq!(
        capabilities["packs"],
        serde_json::json!(["espressif.radio.capabilities"])
    );
    assert_eq!(capabilities["matched"], 3);
    assert_eq!(capabilities["incomplete"], 2);
    assert_eq!(capabilities["unknown"], 0);

    let statuses = capabilities["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| {
            (
                rule["id"].as_str().unwrap(),
                rule["status"].as_str().unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        statuses,
        std::collections::BTreeMap::from([
            ("espressif.radio.bluetooth-hci-transport", "incomplete"),
            ("espressif.radio.coexistence-grant", "matched"),
            ("espressif.radio.ieee802154-rx-boundary", "incomplete"),
            ("espressif.radio.wifi-coexistence", "matched"),
            ("espressif.radio.wifi-rx-boundary", "matched"),
        ])
    );
    assert!(
        capabilities["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| {
                rule["id"] == "espressif.radio.wifi-rx-boundary"
                    && rule["requirements"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|requirement| {
                            requirement["kind"] == "call"
                                && requirement["matches"].as_array().is_some_and(|matches| {
                                    matches.iter().any(|evidence| {
                                        evidence["function"].is_string()
                                            && evidence["site"].is_u64()
                                    })
                                })
                        })
            })
    );
}
