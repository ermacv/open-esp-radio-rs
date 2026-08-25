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
fn inspect_register_schema_four_drafts_only_owned_unreviewed_facts() {
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
    assert_eq!(owned["schema_version"], 4);
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
    assert!(raw.contains("REVIEW_REQUIRED.register-declaration"));
    assert!(!raw.contains("hardware-write-semantics"));
    let commands = owned["review_draft"]["validation_commands"]
        .as_array()
        .unwrap();
    assert_eq!(commands.len(), 3);
    assert!(commands[0].as_str().unwrap().contains("registers validate"));
    assert!(commands[1].as_str().unwrap().contains("project analyze"));
    assert!(
        commands[2]
            .as_str()
            .unwrap()
            .contains("project research next --finding register-0x20103100-32")
    );
    assert!(
        commands
            .iter()
            .all(|command| command.as_str().unwrap().contains("vendor-project.toml"))
    );

    for (address, expected_state) in [
        ("0x2010f4a0", "ignored"),
        ("0x20100010", "reviewed"),
        ("0x20100000", "non-operational"),
        ("0x2010fcb0", "manual"),
    ] {
        let report = inspect(address);
        assert_eq!(report["schema_version"], 4);
        assert_eq!(report["register"]["review_status"], expected_state);
        assert!(report["review_draft"].is_null());
    }
}

#[test]
fn research_schema_nine_exact_finding_lookup_is_not_a_completion_verdict() {
    let lookup = |finding: &str| {
        let output = blobray()
            .args([
                "project",
                "research",
                "next",
                "--scope",
                "ieee802154-baseband-leaves",
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

    let open = lookup("register-0x20103100-32");
    assert_eq!(open["schema_version"], 9);
    assert_eq!(open["completion_claim"], false);
    assert_eq!(open["finding_query"]["state"], "open");
    assert_eq!(open["finding_query"]["completion_claim"], false);
    assert_eq!(open["inventory"]["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        open["inventory"]["findings"][0]["id"],
        "register-0x20103100-32"
    );

    let missing = lookup("register-not-current");
    assert_eq!(missing["schema_version"], 9);
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
    assert_eq!(document["schema"], 3);
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
