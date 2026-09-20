use super::*;
use serde_json::json;
use std::{
    io::{BufRead, BufReader},
    process::Command,
};

fn artifact(path: &str, test: bool) -> serde_json::Value {
    json!({
        "reason": "compiler-artifact", "package_id": "path+file:///phy#0.1.0",
        "manifest_path": "/phy/Cargo.toml",
        "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "oer_esp32s31_phy",
            "src_path": "/phy/src/lib.rs", "edition": "2024", "doc": true, "doctest": true, "test": true},
        "profile": {"opt_level": "3", "debuginfo": 0, "debug_assertions": false, "overflow_checks": false, "test": test},
        "features": [], "filenames": [path], "executable": null, "fresh": true
    })
}

#[test]
fn selects_actual_library_output_and_ignores_test_artifacts() {
    let messages = format!(
        "{}\n{}\n",
        artifact("/arbitrary target/PHY.rlib", false),
        artifact("/test/PHY.rlib", true)
    );
    assert_eq!(
        phy_artifact(messages.as_bytes()).unwrap(),
        PathBuf::from("/arbitrary target/PHY.rlib")
    );
}

#[test]
fn missing_or_ambiguous_library_output_fails() {
    assert!(phy_artifact(b"{\"reason\":\"build-finished\",\"success\":true}\n").is_err());
    let messages = format!(
        "{}\n{}\n",
        artifact("/one.rlib", false),
        artifact("/two.rlib", false)
    );
    assert!(phy_artifact(messages.as_bytes()).is_err());
}

#[test]
fn checkpoint_never_selects_public_or_private_api_documentation() {
    assert!(matches!(checkpoint_docs_scope(), docs::Scope::Static));
}

fn fixture_report(root: &Path, class: FinalImageClass, start: &Path) -> serde_json::Value {
    fs::write(start, b"start").unwrap();
    let base = root.join("target/hil/esp32s31").join(format!(
        "{FINAL_IMAGE_PROFILE}-{}-{FINAL_IMAGE_NETWORK}",
        class.id()
    ));
    let runtime_elf = base
        .join("cargo/runtime")
        .join(TARGET)
        .join("release/runtime.elf");
    let bootstrap_elf = base
        .join("cargo/bootstrap")
        .join(TARGET)
        .join("release/bootstrap.elf");
    let make = |path: &Path| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"fixture").unwrap();
        path.display().to_string()
    };
    json!({
        "schema": 2,
        "image_class": class.id(), "target": TARGET,
        "network": FINAL_IMAGE_NETWORK, "profile": FINAL_IMAGE_PROFILE,
        "runtime_elf": make(&runtime_elf), "runtime_bin": make(&base.join("runtime.bin")),
        "runtime_stack_report": make(&base.join("runtime-stack.txt")),
        "placement_report": make(&base.join("placement.txt")),
        "bootstrap_elf": make(&bootstrap_elf),
        "bootstrap_stack_report": make(&base.join("bootstrap-stack.txt")),
        "effective_embedded_lock": make(&base.join("effective-Cargo.lock")),
        "effective_bootstrap_lock": make(&base.join("bootstrap-Cargo.lock")),
        "application_image": make(&base.join("application.bin")),
        "application_sha256": format!("{:x}", Sha256::digest(b"fixture")),
        "stack_frame_audit": "PASS", "move_size_audit": "PASS",
        "placement_audit": "PASS", "application_audit": "PASS",
        "autonomous_source_graph": "PASS", "flashed": false
    })
}

fn checked(
    report: &serde_json::Value,
    class: FinalImageClass,
    root: &Path,
    start: &Path,
) -> Result<FinalImageArtifact> {
    validate_final_image_report(&serde_json::to_vec(report)?, class, root, start)
}

#[test]
fn both_final_image_reports_select_distinct_existing_class_owned_elfs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let performance_start = root.join("performance.start");
    let correctness_start = root.join("correctness.start");
    let performance = fixture_report(root, FinalImageClass::Performance, &performance_start);
    let correctness = fixture_report(root, FinalImageClass::Correctness, &correctness_start);
    let mut classes = FinalImageClass::ALL.into_iter();
    let mut audits = Vec::new();
    audit_final_images(
        |class| {
            assert_eq!(Some(class), classes.next());
            let (report, start) = if class == FinalImageClass::Performance {
                (&performance, &performance_start)
            } else {
                (&correctness, &correctness_start)
            };
            checked(report, class, root, start)
        },
        |artifact| {
            audits.push(artifact.runtime_elf.clone());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(audits.len(), 2);
    assert_ne!(audits[0], audits[1]);
}

#[test]
fn performance_pass_cannot_hide_correctness_build_or_target_audit_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let start = root.join("performance.start");
    let performance = fixture_report(root, FinalImageClass::Performance, &start);
    let mut audited = Vec::new();
    let error = audit_final_images(
        |class| {
            if class == FinalImageClass::Correctness {
                Err("correctness build failed".into())
            } else {
                checked(&performance, class, root, &start)
            }
        },
        |artifact| {
            audited.push(artifact.runtime_elf.clone());
            Ok(())
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("correctness build failed"), "{error}");
    assert_eq!(audited.len(), 1);
    let correctness_start = root.join("correctness.start");
    let correctness = fixture_report(root, FinalImageClass::Correctness, &correctness_start);
    let error = audit_final_images(
        |class| {
            let (report, start) = if class == FinalImageClass::Performance {
                (&performance, &start)
            } else {
                (&correctness, &correctness_start)
            };
            checked(report, class, root, start)
        },
        |artifact| {
            if artifact.report["image_class"] == "correctness" {
                Err("target audit refused correctness".into())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("target audit refused correctness"),
        "{error}"
    );
}

#[test]
fn wrong_class_missing_files_and_stale_artifacts_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let start = root.join("performance.start");
    let mut report = fixture_report(root, FinalImageClass::Performance, &start);
    assert!(validate_final_image_report(b"", FinalImageClass::Performance, root, &start).is_err());
    report["image_class"] = json!("correctness");
    assert!(checked(&report, FinalImageClass::Performance, root, &start).is_err());
    report["image_class"] = json!("performance");
    report.as_object_mut().unwrap().remove("runtime_elf");
    assert!(checked(&report, FinalImageClass::Performance, root, &start).is_err());
    report["runtime_elf"] = json!(root.join("missing.elf"));
    assert!(checked(&report, FinalImageClass::Performance, root, &start).is_err());
    report = fixture_report(root, FinalImageClass::Performance, &start);
    report
        .as_object_mut()
        .unwrap()
        .remove("runtime_stack_report");
    assert!(checked(&report, FinalImageClass::Performance, root, &start).is_err());
    report = fixture_report(root, FinalImageClass::Performance, &start);
    let old_stack = PathBuf::from(report["runtime_stack_report"].as_str().unwrap());
    let old_app = PathBuf::from(report["application_image"].as_str().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&start, b"later build start").unwrap();
    for path in [old_stack, old_app] {
        let error = checked(&report, FinalImageClass::Performance, root, &start)
            .unwrap_err()
            .to_string();
        assert!(error.contains("stale"), "{error}");
        fs::write(path, b"fixture").unwrap();
    }
}

#[cfg(unix)]
#[test]
fn pending_image_child_is_cleaned_up_after_early_gate_failure() {
    let mut child = owned::Child::spawn_with_shutdown_grace(
        Command::new("sh")
            .args(["-c", "echo $$; exec sleep 30"])
            .stdout(Stdio::piped()),
        std::time::Duration::from_millis(100),
    )
    .unwrap();
    let mut output = String::new();
    BufReader::new(child.take_stdout().unwrap())
        .read_line(&mut output)
        .unwrap();
    // A failed concurrent stage drops the owned process group without waiting
    // for the image's nominal 30-second sleep.
    let group: i32 = output.lines().next().unwrap().parse().unwrap();
    drop(child);
    // SAFETY: signal zero observes only the PID emitted by this test child.
    assert_ne!(unsafe { libc::kill(-group, 0) }, 0);
}

#[test]
fn docs_failure_stops_source_only_before_the_image_owner_starts() {
    let mut executed = Vec::new();
    let error = run_pre_image_stages(|stage| {
        executed.push(stage);
        if stage == PreImageStage::Docs {
            Err("docs fixture failed".into())
        } else {
            Ok(())
        }
    })
    .unwrap_err()
    .to_string();
    assert_eq!(executed, PRE_IMAGE_STAGES);
    assert!(error.contains("docs fixture failed"), "{error}");
    // The production image owner is created only after this shared sequence
    // returns successfully, so this failure path cannot reach hardware/image work.
}
