#![cfg(unix)]

use oer_xtask::{
    ci::{coverage, model::*, planning},
    registry::Workflow,
};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt as _, path::Path, process::Command};

fn executable(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn success(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn image_rollouts_and_tool_drift_do_not_fail_execution_or_issue_wrong_coverage() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    let bin = directory.path().join("bin");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
    success(Command::new("git").current_dir(&root).args(["init", "-q"]));
    success(Command::new("git").current_dir(&root).args(["add", "."]));
    success(Command::new("git").current_dir(&root).args([
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "-qm",
        "fixture",
    ]));
    for tool in ["clang", "ld.lld"] {
        executable(
            &bin.join(tool),
            "#!/bin/sh\nprintf 'fixture tool version\\n'\n",
        );
    }
    // Exercise the CLI and Git with controlled tools and provider downloads.
    executable(
        &bin.join("gh"),
        "#!/usr/bin/env python3\nimport os, pathlib, shutil, sys\nargs = sys.argv[1:]\nif os.environ.get('CI_TEST_FAIL_DOWNLOAD') == 'true': sys.exit(1)\nassert args[:2] == ['run', 'download']\nassert args[args.index('--name') + 1] == 'ci-plan'\nshutil.copyfile(os.environ['CI_TEST_PLAN'], pathlib.Path(args[args.index('--dir') + 1]) / 'ci-plan.json')\n",
    );
    let paths = std::env::join_paths(
        std::iter::once(bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let plan_path = directory.path().join("plan.json");
    let environment_path = directory.path().join("environment.json");
    let output_path = directory.path().join("outputs");
    let summary_path = directory.path().join("summary");
    let proof_path = directory.path().join("proof.json");
    let commit = oer_process::git::text(&root, ["rev-parse", "HEAD"]).unwrap();
    let cli = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oer-xtask"));
        command
            .args(["--root"])
            .arg(&root)
            .arg("ci")
            .env("PATH", &paths)
            .env("ImageOS", "ubuntu24")
            .env("ImageVersion", "20261004.327.1")
            .env("GITHUB_REPOSITORY", "owner/repo")
            .env("GITHUB_RUN_ID", "42")
            .env("GITHUB_RUN_ATTEMPT", "2")
            .env("GITHUB_SHA", &commit)
            .env("CI_TEST_PLAN", &plan_path)
            .env("GITHUB_OUTPUT", &output_path)
            .env("GITHUB_STEP_SUMMARY", &summary_path);
        command
    };
    success(
        cli()
            .args(["environment", "--workflow", "ci", "--output"])
            .arg(&environment_path)
            .env("ImageVersion", "20260927.320.1"),
    );
    let original: Environment =
        serde_json::from_slice(&std::fs::read(&environment_path).unwrap()).unwrap();
    success(
        cli()
            .args(["environment", "--workflow", "ci", "--output"])
            .arg(&environment_path),
    );
    let current: Environment =
        serde_json::from_slice(&std::fs::read(&environment_path).unwrap()).unwrap();
    assert_eq!(original, current);

    for changed in [
        "image-only",
        "rustc",
        "clang",
        "lld",
        "plan-download",
        "invalid-plan",
        "unavailable",
    ] {
        let mut expected = original.clone();
        match changed {
            "rustc" => expected.common.rustc = "different rustc".into(),
            "clang" | "lld" => {
                expected
                    .programs
                    .insert(changed.into(), "different version".into());
            }
            "unavailable" => executable(&bin.join("clang"), "#!/bin/sh\nexit 1\n"),
            _ => {}
        }
        let manifest = planning::manifest(&root, Workflow::Ci, expected).unwrap();
        let plan = coverage::select(manifest.clone(), 42, &[], Mode::Observe, 1100, None).unwrap();
        oer_durable::atomic_json(&plan_path, &plan).unwrap();
        std::fs::write(&output_path, "").unwrap();
        std::fs::write(&summary_path, "").unwrap();
        if changed == "invalid-plan" {
            let mut invalid = serde_json::to_value(&plan).unwrap();
            invalid["run_id"] = 99.into();
            oer_durable::atomic_json(&plan_path, &invalid).unwrap();
        }
        success(
            cli()
                .args([
                    "check-environment",
                    "--workflow",
                    "ci",
                    "--job",
                    "isa-conformance",
                ])
                .env(
                    "CI_TEST_FAIL_DOWNLOAD",
                    (changed == "plan-download").to_string(),
                ),
        );
        let reusable = changed == "image-only";
        assert_eq!(
            std::fs::read_to_string(&output_path).unwrap(),
            format!("reusable={reusable}\n")
        );
        let mut outcomes: BTreeMap<String, serde_json::Value> = manifest.jobs.keys().map(|id| {
            let qualified = id != "isa-conformance" || reusable;
            (id.clone(), serde_json::json!({"result": "success", "outputs": {"reusable": qualified.to_string()}}))
        }).collect();
        outcomes.insert(
            "prepare".into(),
            serde_json::json!({"result": "success", "outputs": {}}),
        );
        // The job may proceed without a plan, but the gate still requires one.
        if matches!(changed, "plan-download" | "invalid-plan") {
            let blocked_gate = cli()
                .args(["verify", "--workflow", "ci", "--output"])
                .arg(&proof_path)
                .env("CI_NEEDS", serde_json::to_string(&outcomes).unwrap())
                .env(
                    "CI_TEST_FAIL_DOWNLOAD",
                    (changed == "plan-download").to_string(),
                )
                .output()
                .unwrap();
            assert!(!blocked_gate.status.success());
            assert!(!proof_path.exists());
        }
        // A recovered download still cannot qualify the job retrospectively.
        oer_durable::atomic_json(&plan_path, &plan).unwrap();
        success(
            cli()
                .args(["verify", "--workflow", "ci", "--output"])
                .arg(&proof_path)
                .env("CI_NEEDS", serde_json::to_string(&outcomes).unwrap()),
        );
        let proof: Proof = serde_json::from_slice(&std::fs::read(&proof_path).unwrap()).unwrap();
        assert_eq!(proof.attempt, 2);
        assert_eq!(proof.jobs.contains_key("isa-conformance"), reusable);
        let jobs = Workflow::Ci.spec().jobs.len();
        assert_eq!(proof.jobs.len(), if reusable { jobs } else { jobs - 1 });
        let at = proof.jobs["host"].verified_at + 1;
        let next = coverage::select(manifest, 43, &[proof], Mode::Reuse, at, None).unwrap();
        for (id, action) in next.actions {
            assert_eq!(action.run, id == "isa-conformance" && !reusable);
        }
        // Disqualification never hides an actual failed check.
        outcomes.get_mut("isa-conformance").unwrap()["result"] = "failure".into();
        std::fs::remove_file(&proof_path).unwrap();
        let failed = cli()
            .args(["verify", "--workflow", "ci", "--output"])
            .arg(&proof_path)
            .env("CI_NEEDS", serde_json::to_string(&outcomes).unwrap())
            .output()
            .unwrap();
        assert!(!failed.status.success());
        assert!(!proof_path.exists());
    }
}
