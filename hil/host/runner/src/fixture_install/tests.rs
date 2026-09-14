use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest as _, Sha256};

use super::*;
use crate::fixture_install::{
    model::validate_adapters,
    transaction::test_support::{Effects, Layout, Stage, apply, policy_bytes},
};

const OPERATOR: &str = "fixture_test";

struct TestEffects {
    fail: Option<Stage>,
    panic: Option<Stage>,
    reject_candidate: bool,
    make_receipt_unwritable: Option<PathBuf>,
    calls: Vec<&'static str>,
}

impl TestEffects {
    fn passing() -> Self {
        Self {
            fail: None,
            panic: None,
            reject_candidate: false,
            make_receipt_unwritable: None,
            calls: Vec::new(),
        }
    }
}

impl Effects for TestEffects {
    fn checkpoint(&mut self, stage: Stage) -> crate::Result<()> {
        if self.panic == Some(stage) {
            panic!("simulated abrupt termination at {stage:?}");
        }
        if self.fail == Some(stage) {
            return Err(format!("injected failure at {stage:?}").into());
        }
        if stage == Stage::Receipt
            && let Some(directory) = &self.make_receipt_unwritable
        {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o555))?;
        }
        Ok(())
    }

    fn validate_candidate(&mut self, _layout: &Layout, candidate: &Path) -> crate::Result<()> {
        self.calls.push("candidate-policy");
        let policy = fs::read_to_string(candidate)?;
        if self.reject_candidate || !policy.contains("NOPASSWD") {
            return Err("candidate policy rejected".into());
        }
        Ok(())
    }

    fn validate_effective(&mut self, _layout: &Layout) -> crate::Result<()> {
        self.calls.push("effective-policy");
        Ok(())
    }

    fn verify_capabilities(&mut self, helper: &Path, expected: &str) -> crate::Result<()> {
        self.calls.push("capabilities");
        let imported = fs::read_to_string(helper)?;
        if !imported.contains(expected) {
            return Err("capabilities mismatch".into());
        }
        Ok(())
    }

    fn require_provider_idle(&mut self, _provider: Provider) -> crate::Result<()> {
        self.calls.push("provider-idle");
        Ok(())
    }
}

fn artifact_declarations(provider: Provider, suffix: &str) -> Vec<(Artifact, Vec<u8>)> {
    let network = format!(
        "#!/bin/sh\ntest \"$1\" = capabilities && printf '%s\\n' '{}'\n# {suffix}\n",
        Provider::LinuxNet.runtime_contract()
    );
    let bluetooth = format!(
        "#!/bin/sh\ntest \"$1\" = capabilities && printf '%s\\n' '{}'\n# {suffix}\n",
        Provider::LinuxBluetooth.runtime_contract()
    );
    let entries: Vec<(ArtifactRole, &str, &str, u32, Vec<u8>)> = match provider {
        Provider::LinuxNet => vec![
            (
                ArtifactRole::NetworkHelper,
                "open-radio-net",
                "/usr/local/sbin/open-radio-net",
                0o555,
                network.into_bytes(),
            ),
            (
                ArtifactRole::ProbeHelper,
                "open-radio-probe",
                "/usr/local/libexec/open-radio-probe",
                0o555,
                format!("probe-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::Hostapd,
                "open-radio-hostapd",
                "/usr/local/libexec/open-radio-hostapd",
                0o555,
                format!("hostapd-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::HostapdProvenance,
                "open-radio-hostapd.json",
                "/usr/local/libexec/open-radio-hostapd.json",
                0o444,
                format!("{{\"generation\":\"{suffix}\"}}\n").into_bytes(),
            ),
        ],
        Provider::LinuxBluetooth => vec![(
            ArtifactRole::BluetoothHelper,
            "open-radio-bluetooth",
            "/usr/local/libexec/open-radio-bluetooth",
            0o555,
            bluetooth.into_bytes(),
        )],
    };
    entries
        .into_iter()
        .map(|(role, name, target, mode, bytes)| {
            (
                Artifact {
                    role,
                    file_name: name.to_owned(),
                    target: target.into(),
                    size_bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    mode,
                },
                bytes,
            )
        })
        .collect()
}

fn make_bundle(directory: &Path, provider: Provider, suffix: &str) -> (PathBuf, Bundle) {
    let bundle_directory = directory.join(format!("bundle-{}-{suffix}", provider.as_str()));
    let artifact_directory = bundle_directory.join("artifacts");
    fs::create_dir_all(&artifact_directory).unwrap();
    fs::set_permissions(&bundle_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let declarations = artifact_declarations(provider, suffix);
    for (artifact, bytes) in &declarations {
        let path = artifact_directory.join(&artifact.file_name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(artifact.mode)).unwrap();
    }
    let mut bundle = Bundle {
        schema: BUNDLE_SCHEMA,
        provider,
        generation: String::new(),
        operator: OPERATOR.to_owned(),
        source: SourceIdentity {
            commit: "1".repeat(40),
            dirty: true,
            workspace_state_sha256: format!("{:x}", Sha256::digest(suffix.as_bytes())),
        },
        runtime_contract: provider.runtime_contract().to_owned(),
        allowed_bluetooth_adapters: provider.default_adapters(),
        artifacts: declarations
            .into_iter()
            .map(|(artifact, _)| artifact)
            .collect(),
    };
    bundle.generation = bundle_hash(&bundle);
    fs::write(
        bundle_directory.join("bundle.json"),
        serde_json::to_vec_pretty(&bundle).unwrap(),
    )
    .unwrap();
    (bundle_directory, bundle)
}

fn bundle_hash(bundle: &Bundle) -> String {
    let mut identity = bundle.clone();
    identity.generation.clear();
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity).unwrap())
    )
}

fn test_apply(
    root: &Path,
    provider: Provider,
    bundle: &Path,
    effects: &mut TestEffects,
) -> InstallResult {
    let layout = Layout::test(root, provider).unwrap();
    apply(
        &layout,
        provider,
        bundle,
        OPERATOR,
        unsafe { libc::geteuid() },
        effects,
    )
    .unwrap()
}

#[test]
fn plans_are_provider_specific_deterministic_and_offline() {
    let root = tempfile::tempdir().unwrap();
    let net = build_plan(root.path(), Provider::LinuxNet, &[]).unwrap();
    assert_eq!(
        net,
        build_plan(root.path(), Provider::LinuxNet, &[]).unwrap()
    );
    assert_eq!(net.provider, Provider::LinuxNet);
    assert!(net.build_steps.iter().any(|step| step.contains("hostapd")));
    assert!(
        net.artifacts
            .iter()
            .all(|item| item.state == ArtifactState::Missing)
    );
    assert!(!net.automatic_hardware_checks);

    let bluetooth = build_plan(root.path(), Provider::LinuxBluetooth, &[]).unwrap();
    assert_eq!(bluetooth.allowed_bluetooth_adapters, ["hci0"]);
    assert!(
        bluetooth
            .build_steps
            .iter()
            .all(|step| !step.contains("hostapd"))
    );
}

#[test]
fn bluetooth_adapter_policy_is_explicit_and_canonical() {
    assert_eq!(
        validate_adapters(
            Provider::LinuxBluetooth,
            &["hci2".into(), "hci0".into(), "hci2".into()]
        )
        .unwrap(),
        ["hci0", "hci2"]
    );
    for invalid in ["hci01", "hci65535", "../hci0", "hci*", "wlan0"] {
        assert!(
            validate_adapters(Provider::LinuxBluetooth, &[invalid.into()]).is_err(),
            "accepted {invalid}"
        );
    }
    assert!(validate_adapters(Provider::LinuxNet, &["hci0".into()]).is_err());
}

#[test]
fn fresh_repeat_and_upgrade_use_complete_generations() {
    let root = tempfile::tempdir().unwrap();
    let (first_path, first) = make_bundle(root.path(), Provider::LinuxBluetooth, "first");
    let result = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &first_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(
        result.state,
        InstallState::SoftwareVerified,
        "{:?}",
        result.primary_error
    );
    assert!(result.changed);
    let stable = root.path().join("usr/local/libexec/open-radio-bluetooth");
    assert_eq!(
        fs::read(&stable).unwrap(),
        artifact_declarations(Provider::LinuxBluetooth, "first")[0].1
    );

    let repeat = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &first_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(repeat.state, InstallState::SoftwareVerified);
    assert!(!repeat.changed);

    let (second_path, second) = make_bundle(root.path(), Provider::LinuxBluetooth, "second");
    let upgrade = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &second_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(
        upgrade.previous_generation.as_deref(),
        Some(first.generation.as_str())
    );
    assert_eq!(upgrade.generation, second.generation);
    assert_eq!(
        fs::read(&stable).unwrap(),
        artifact_declarations(Provider::LinuxBluetooth, "second")[0].1
    );
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/generations")
            .join(first.generation)
            .exists()
    );
}

#[test]
fn post_install_dispatch_contains_only_policy_bytes_and_capabilities_checks() {
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "software-only");
    let mut effects = TestEffects::passing();
    let result = test_apply(root.path(), Provider::LinuxBluetooth, &bundle, &mut effects);
    assert_eq!(result.state, InstallState::SoftwareVerified);
    assert_eq!(
        effects.calls,
        [
            "provider-idle",
            "candidate-policy",
            "effective-policy",
            "effective-policy",
            "capabilities"
        ]
    );
}

#[test]
fn invalid_candidate_rolls_back_without_replacing_old_bytes_or_policy() {
    let root = tempfile::tempdir().unwrap();
    let (first, _) = make_bundle(root.path(), Provider::LinuxNet, "old");
    let installed = test_apply(
        root.path(),
        Provider::LinuxNet,
        &first,
        &mut TestEffects::passing(),
    );
    let stable = root.path().join("usr/local/sbin/open-radio-net");
    let policy = root.path().join("etc/sudoers.d/open-radio-net");
    let old_bytes = fs::read(&stable).unwrap();
    let old_policy = fs::read(&policy).unwrap();
    let (second, _) = make_bundle(root.path(), Provider::LinuxNet, "new");
    let mut effects = TestEffects::passing();
    effects.reject_candidate = true;
    let rejected = test_apply(root.path(), Provider::LinuxNet, &second, &mut effects);
    assert_eq!(rejected.state, InstallState::RolledBack);
    assert!(rejected.primary_error.unwrap().contains("candidate policy"));
    assert_eq!(fs::read(stable).unwrap(), old_bytes);
    assert_eq!(fs::read(policy).unwrap(), old_policy);
    assert_eq!(
        fs::read_link(
            root.path()
                .join("var/lib/open-radio/fixture/linux-net/current")
        )
        .unwrap(),
        PathBuf::from("generations").join(installed.generation)
    );
}

#[test]
fn effective_policy_failure_rolls_back_and_never_runs_capabilities() {
    struct EffectiveReject {
        effective_calls: usize,
    }
    impl Effects for EffectiveReject {
        fn validate_candidate(&mut self, _: &Layout, _: &Path) -> crate::Result<()> {
            Ok(())
        }
        fn validate_effective(&mut self, _: &Layout) -> crate::Result<()> {
            self.effective_calls += 1;
            if self.effective_calls == 1 {
                Err("effective policy conflict".into())
            } else {
                Ok(())
            }
        }
        fn verify_capabilities(&mut self, _: &Path, _: &str) -> crate::Result<()> {
            panic!("capabilities must not run after policy rejection")
        }
    }

    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "policy-conflict");
    let mut effects = EffectiveReject { effective_calls: 0 };
    let result = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut effects,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RolledBack);
    assert!(result.primary_error.unwrap().contains("effective policy"));
    assert_eq!(effects.effective_calls, 2);
    assert!(
        !root
            .path()
            .join("etc/sudoers.d/open-radio-bluetooth")
            .exists()
    );
}

#[test]
fn every_control_stage_fails_closed_and_rollback_failure_is_reported() {
    for stage in [
        Stage::Imported,
        Stage::LinksPrepared,
        Stage::CandidateValidated,
        Stage::PolicyPublished,
        Stage::Activated,
        Stage::SoftwareVerified,
    ] {
        let root = tempfile::tempdir().unwrap();
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "failure");
        let mut effects = TestEffects::passing();
        effects.fail = Some(stage);
        let result = test_apply(root.path(), Provider::LinuxBluetooth, &bundle, &mut effects);
        assert_eq!(result.state, InstallState::RolledBack, "stage {stage:?}");
        assert!(
            !root
                .path()
                .join("usr/local/libexec/open-radio-bluetooth")
                .exists(),
            "stage {stage:?} left a stable helper"
        );
    }

    struct RollbackFailure;
    impl Effects for RollbackFailure {
        fn checkpoint(&mut self, stage: Stage) -> crate::Result<()> {
            if stage == Stage::CandidateValidated || stage == Stage::Rollback {
                return Err(format!("failure at {stage:?}").into());
            }
            Ok(())
        }
        fn validate_candidate(&mut self, _: &Layout, _: &Path) -> crate::Result<()> {
            Ok(())
        }
        fn validate_effective(&mut self, _: &Layout) -> crate::Result<()> {
            Ok(())
        }
        fn verify_capabilities(&mut self, _: &Path, _: &str) -> crate::Result<()> {
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "rollback");
    let result = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut RollbackFailure,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RecoveryRequired);
    assert!(result.primary_error.is_some());
    assert!(result.recovery_error.is_some());
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
            .exists()
    );
}

#[test]
fn abrupt_process_loss_is_recovered_from_persisted_journal() {
    for stage in [Stage::PolicyPublished, Stage::Activated] {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
        let (bundle, expected) = make_bundle(root.path(), Provider::LinuxBluetooth, "crash");
        let mut crashing = TestEffects::passing();
        crashing.panic = Some(stage);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = apply(
                &layout,
                Provider::LinuxBluetooth,
                &bundle,
                OPERATOR,
                unsafe { libc::geteuid() },
                &mut crashing,
            );
        }));
        assert!(crashed.is_err());
        assert!(
            root.path()
                .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
                .exists()
        );

        let recovered = apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .unwrap();
        assert_eq!(recovered.state, InstallState::SoftwareVerified);
        assert_eq!(
            fs::read_link(
                root.path()
                    .join("var/lib/open-radio/fixture/linux-bluetooth/current")
            )
            .unwrap(),
            PathBuf::from("generations").join(expected.generation)
        );
        assert!(
            !root
                .path()
                .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
                .exists()
        );
    }
}

#[test]
fn receipt_failure_after_commit_preserves_truth_and_next_apply_finishes_recovery() {
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let (bundle, expected) = make_bundle(root.path(), Provider::LinuxBluetooth, "receipt");
    let mut effects = TestEffects::passing();
    let receipts = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/receipts");
    effects.make_receipt_unwritable = Some(receipts.clone());
    let result = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut effects,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RecoveryRequired);
    assert_eq!(
        fs::read_link(
            root.path()
                .join("var/lib/open-radio/fixture/linux-bluetooth/current")
        )
        .unwrap(),
        PathBuf::from("generations").join(&expected.generation)
    );
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
            .exists()
    );

    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o755)).unwrap();
    let recovered = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut TestEffects::passing(),
    )
    .unwrap();
    assert_eq!(recovered.state, InstallState::SoftwareVerified);
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/receipt.json")
            .exists()
    );
}

#[test]
fn source_replacement_symlink_and_partial_installation_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "source");
    let replaced = bundle.join("artifacts/open-radio-bluetooth");
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(replaced, b"replaced").unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "symlink");
    let original = bundle.join("artifacts/open-radio-bluetooth");
    fs::remove_file(&original).unwrap();
    std::os::unix::fs::symlink("/bin/true", &original).unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "partial");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    fs::write(root.path().join("usr/local/sbin/open-radio-net"), b"legacy").unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxNet,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );
}

#[test]
fn manifest_traversal_operator_mismatch_and_writable_bundle_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (bundle_path, mut bundle) = make_bundle(root.path(), Provider::LinuxBluetooth, "manifest");
    bundle.artifacts[0].target = PathBuf::from("/usr/local/libexec/../escape");
    bundle.generation = bundle_hash(&bundle);
    fs::write(
        bundle_path.join("bundle.json"),
        serde_json::to_vec(&bundle).unwrap(),
    )
    .unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle_path,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle_path, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "operator");
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle_path,
            "another_operator",
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );
    fs::set_permissions(&bundle_path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle_path,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );
}

#[test]
fn legacy_installation_is_retained_as_previous_generation() {
    let root = tempfile::tempdir().unwrap();
    let _layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let declarations = artifact_declarations(Provider::LinuxBluetooth, "legacy");
    let stable = root.path().join("usr/local/libexec/open-radio-bluetooth");
    fs::write(&stable, &declarations[0].1).unwrap();
    fs::set_permissions(&stable, fs::Permissions::from_mode(0o555)).unwrap();
    let policy = root.path().join("etc/sudoers.d/open-radio-bluetooth");
    fs::write(&policy, b"legacy policy\n").unwrap();
    fs::set_permissions(&policy, fs::Permissions::from_mode(0o440)).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "new");
    let result = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &bundle,
        &mut TestEffects::passing(),
    );
    let previous = result.previous_generation.unwrap();
    assert!(previous.starts_with("legacy-"));
    let retained = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/generations")
        .join(previous);
    assert_eq!(
        fs::read(retained.join("open-radio-bluetooth")).unwrap(),
        declarations[0].1
    );
    assert_eq!(
        fs::read(retained.join("sudoers.policy")).unwrap(),
        b"legacy policy\n"
    );
}

#[test]
fn active_session_lock_refuses_upgrade_and_providers_are_independent() {
    let root = tempfile::tempdir().unwrap();
    let (net_bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "net");
    let (bt_bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "bt");
    test_apply(
        root.path(),
        Provider::LinuxNet,
        &net_bundle,
        &mut TestEffects::passing(),
    );
    test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &bt_bundle,
        &mut TestEffects::passing(),
    );
    assert!(root.path().join("usr/local/sbin/open-radio-net").exists());
    assert!(
        root.path()
            .join("usr/local/libexec/open-radio-bluetooth")
            .exists()
    );

    let lock_path = root
        .path()
        .join("run/open-radio-fixture/linux-net.session.lock");
    let lock = File::open(&lock_path).unwrap();
    lock.lock_shared().unwrap();
    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxNet, "upgrade");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let error = apply(
        &layout,
        Provider::LinuxNet,
        &upgrade,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut TestEffects::passing(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("active HIL session"));
    fs2::FileExt::unlock(&lock).unwrap();

    let install_lock_path = root.path().join("var/lib/open-radio/fixture/install.lock");
    let install_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(install_lock_path)
        .unwrap();
    fs2::FileExt::lock_exclusive(&install_lock).unwrap();
    let error = apply(
        &layout,
        Provider::LinuxNet,
        &upgrade,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut TestEffects::passing(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("another fixture installation"));
    let bluetooth_layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let error = apply(
        &bluetooth_layout,
        Provider::LinuxBluetooth,
        &bt_bundle,
        OPERATOR,
        unsafe { libc::geteuid() },
        &mut TestEffects::passing(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("another fixture installation"));
    fs2::FileExt::unlock(&install_lock).unwrap();
}

#[test]
fn writable_or_symlinked_destination_and_writable_active_generation_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "paths");
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let state = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();

    let destination = root.path().join("usr/local/libexec");
    fs::remove_dir(&destination).unwrap();
    std::os::unix::fs::symlink(root.path().join("etc"), &destination).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle, expected) = make_bundle(root.path(), Provider::LinuxBluetooth, "active-mode");
    let result = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &bundle,
        &mut TestEffects::passing(),
    );
    assert_eq!(result.state, InstallState::SoftwareVerified);
    let generation = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/generations")
        .join(expected.generation);
    fs::set_permissions(&generation, fs::Permissions::from_mode(0o777)).unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            unsafe { libc::geteuid() },
            &mut TestEffects::passing(),
        )
        .is_err()
    );
}

#[test]
fn policy_never_grants_installer_runner_or_wildcard_adapter() {
    let directory = tempfile::tempdir().unwrap();
    let (_, mut bundle) = make_bundle(directory.path(), Provider::LinuxBluetooth, "policy");
    bundle.allowed_bluetooth_adapters = vec!["hci0".into(), "hci2".into()];
    let policy = String::from_utf8(policy_bytes(&bundle).unwrap()).unwrap();
    assert!(policy.contains("--adapter hci0"));
    assert!(policy.contains("--adapter hci2"));
    assert!(!policy.contains("hci*"));
    assert!(!policy.contains("fixture-install"));
    assert!(!policy.contains("cargo"));
    assert!(!policy.contains("hil-runner"));
}

#[test]
fn genuine_visudo_parses_generated_policy_when_available() {
    let Some(visudo) = ["/usr/sbin/visudo", "/usr/bin/visudo"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
    else {
        eprintln!("visudo unavailable: genuine parser check skipped");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (_, mut bundle) = make_bundle(directory.path(), Provider::LinuxBluetooth, "visudo");
    bundle.operator = String::from_utf8(
        Command::new("/usr/bin/id")
            .arg("-un")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let main = directory.path().join("sudoers.main");
    fs::write(&main, b"Defaults env_reset\n").unwrap();
    let candidate = directory.path().join("sudoers.candidate");
    fs::write(&candidate, policy_bytes(&bundle).unwrap()).unwrap();
    let policy_path = directory.path().join("sudoers.composite");
    fs::write(
        &policy_path,
        format!(
            "@include {}\n@include {}\n",
            main.display(),
            candidate.display()
        ),
    )
    .unwrap();
    let output = Command::new(&visudo)
        .args(["-c", "-f"])
        .arg(&policy_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::write(&candidate, b"this is not sudoers syntax {\n").unwrap();
    let invalid = Command::new(&visudo)
        .args(["-c", "-f"])
        .arg(&policy_path)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

fn initialize_test_repository(root: &Path) {
    fs::write(root.join(".gitignore"), "/target\n").unwrap();
    fs::write(root.join("tracked.txt"), "clean\n").unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", ".gitignore", "tracked.txt"][..],
        &[
            "-c",
            "user.name=Fixture Test",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ][..],
    ] {
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
}

fn write_bluetooth_prepare_input(root: &Path, suffix: &str) -> PathBuf {
    let path = root.join("target/hil/fixture-build/debug/open-radio-bluetooth");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    executable(
        &path,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n# {suffix}\n",
            Provider::LinuxBluetooth.runtime_contract()
        ),
    );
    path
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn preparation_captures_locked_bluetooth_artifact_and_dirty_source_identity() {
    let root = tempfile::tempdir().unwrap();
    initialize_test_repository(root.path());
    write_bluetooth_prepare_input(root.path(), "prepared");
    fs::write(root.path().join("tracked.txt"), "dirty\n").unwrap();

    let directory = prepare(
        root.path(),
        Provider::LinuxBluetooth,
        OPERATOR,
        &["hci2".into()],
    )
    .unwrap();
    let bundle: Bundle =
        serde_json::from_slice(&fs::read(directory.join("bundle.json")).unwrap()).unwrap();
    assert!(bundle.source.dirty);
    assert_eq!(bundle.allowed_bluetooth_adapters, ["hci2"]);
    assert_eq!(bundle.artifacts.len(), 1);
    assert_eq!(bundle.artifacts[0].role, ArtifactRole::BluetoothHelper);
    assert!(!directory.join("artifacts/open-radio-hostapd").exists());
}

#[test]
fn preparation_rejects_missing_stale_or_modified_artifacts() {
    let root = tempfile::tempdir().unwrap();
    initialize_test_repository(root.path());
    assert!(prepare(root.path(), Provider::LinuxBluetooth, OPERATOR, &[]).is_err());

    let helper = write_bluetooth_prepare_input(root.path(), "stale");
    executable(&helper, "#!/bin/sh\necho schema=1\n");
    assert!(
        prepare(root.path(), Provider::LinuxBluetooth, OPERATOR, &[])
            .unwrap_err()
            .to_string()
            .contains("incompatible runtime contract")
    );

    write_bluetooth_prepare_input(root.path(), "valid");
    let bundle = prepare(root.path(), Provider::LinuxBluetooth, OPERATOR, &[]).unwrap();
    let installed = bundle.join("artifacts/open-radio-bluetooth");
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(&installed, b"corrupt cached bundle").unwrap();
    assert!(
        prepare(root.path(), Provider::LinuxBluetooth, OPERATOR, &[])
            .unwrap_err()
            .to_string()
            .contains("existing bundle artifact mismatch")
    );
}

#[test]
fn network_preparation_rejects_hostapd_provenance_mismatch() {
    let root = tempfile::tempdir().unwrap();
    initialize_test_repository(root.path());
    let network = root.path().join("hil/host/linux-net/open-radio-net");
    fs::create_dir_all(network.parent().unwrap()).unwrap();
    executable(
        &network,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n",
            Provider::LinuxNet.runtime_contract()
        ),
    );
    let probe = root
        .path()
        .join("target/hil/fixture-build/debug/open-radio-probe");
    fs::create_dir_all(probe.parent().unwrap()).unwrap();
    executable(&probe, "#!/bin/sh\nexit 0\n");
    let hostapd = root.path().join("target/hil/hostapd/hostapd");
    fs::create_dir_all(hostapd.parent().unwrap()).unwrap();
    executable(&hostapd, "hostapd bytes");
    fs::write(
        root.path().join("target/hil/hostapd/provenance.json"),
        serde_json::json!({"binary_sha256": "0".repeat(64)}).to_string(),
    )
    .unwrap();
    assert!(
        prepare(root.path(), Provider::LinuxNet, OPERATOR, &[])
            .unwrap_err()
            .to_string()
            .contains("provenance")
    );
    let hostapd_sha256 = format!("{:x}", Sha256::digest(fs::read(&hostapd).unwrap()));
    fs::write(
        root.path().join("target/hil/hostapd/provenance.json"),
        serde_json::json!({"binary_sha256": hostapd_sha256}).to_string(),
    )
    .unwrap();
    let directory = prepare(root.path(), Provider::LinuxNet, OPERATOR, &[]).unwrap();
    let bundle: Bundle =
        serde_json::from_slice(&fs::read(directory.join("bundle.json")).unwrap()).unwrap();
    assert_eq!(bundle.artifacts.len(), 4);
    assert!(
        bundle
            .artifacts
            .iter()
            .any(|artifact| artifact.role == ArtifactRole::HostapdProvenance)
    );
}
