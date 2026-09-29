use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_hil_evidence::run::{
    collect_integrity_files,
    test_support::{session, session_for, temporary_directory, write_test_build_materials},
};
use oer_hil_image_class::ImageClass;

use super::{Recipe, firmware};
use crate::{Artifacts, BootArtifacts, Integration, TARGET};

/// Owned-Xarxa correctness artifacts over the given files, with one lock
/// standing in for both effective locks and a host-independent environment.
fn test_artifacts(
    application: &Path,
    runtime_elf: &Path,
    runtime_bin: &Path,
    bootstrap_elf: &Path,
    lock: &Path,
) -> Artifacts {
    Artifacts {
        chip: String::from("esp32s31"),
        rust_target: String::from(TARGET),
        layout_seed: None,
        features: oer_hil_image_class::FeatureDelta::default(),
        network: Integration::OwnedXarxa,
        output: application.parent().unwrap().to_path_buf(),
        runtime_elf: runtime_elf.to_path_buf(),
        effective_embedded_lock: lock.to_path_buf(),
        application_image: application.to_path_buf(),
        boot: BootArtifacts::Staged {
            runtime_bin: runtime_bin.to_path_buf(),
            bootstrap_elf: bootstrap_elf.to_path_buf(),
            effective_bootstrap_lock: lock.to_path_buf(),
        },
        source_inputs: None,
        environment: oer_hil_evidence::build::BuildEnvironment::synthetic(),
    }
}

#[test]
fn firmware_record_archives_the_exact_application() {
    let root = temporary_directory("firmware");
    write_test_build_materials(&root);
    let run_directory = root.join("run");
    fs::create_dir(&run_directory).unwrap();
    let application = root.join("application.bin");
    let runtime_elf = root.join("runtime.elf");
    let runtime_bin = root.join("runtime.bin");
    let bootstrap_elf = root.join("bootstrap.elf");
    let effective_embedded_lock = root.join("hil/targets/esp32s31/Cargo.lock");
    fs::write(&application, b"application bytes").unwrap();
    fs::write(&runtime_elf, b"runtime elf").unwrap();
    fs::write(&runtime_bin, b"runtime bin").unwrap();
    fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

    let mut first_session = session(&run_directory);
    let (_snapshot_root, snapshot) = oer_hil_source_snapshot::test_snapshot(&root);
    let slots = root.join("slots/source-build");
    first_session
        .bind_source_snapshot(snapshot.directory(), &slots)
        .unwrap();
    firmware::record(
        &mut first_session,
        ImageClass::Correctness,
        &test_artifacts(
            &application,
            &runtime_elf,
            &runtime_bin,
            &bootstrap_elf,
            &effective_embedded_lock,
        ),
    )
    .unwrap();
    let artifact = &first_session.manifest().firmware[0];
    assert_eq!(
        artifact.application_path,
        PathBuf::from("firmware/correctness/application.bin")
    );
    assert_eq!(artifact.application_size_bytes, 17);
    assert_eq!(
        fs::read(run_directory.join(&artifact.application_path)).unwrap(),
        b"application bytes"
    );
    assert_eq!(artifact.application_sha256.len(), 64);
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .runtime_elf_path
                    .as_ref()
                    .expect("runtime ELF path")
            )
        )
        .unwrap(),
        b"runtime elf"
    );
    assert_eq!(artifact.runtime_elf_size_bytes, Some(11));
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .runtime_bin_path
                    .as_ref()
                    .expect("runtime bin path")
            )
        )
        .unwrap(),
        b"runtime bin"
    );
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .bootstrap_elf_path
                    .as_ref()
                    .expect("bootstrap ELF path")
            )
        )
        .unwrap(),
        b"bootstrap elf"
    );
    let provenance_path = artifact
        .build_provenance_path
        .as_ref()
        .expect("build provenance path");
    let provenance: oer_hil_evidence::build::BuildProvenance =
        serde_json::from_slice(&fs::read(run_directory.join(provenance_path)).unwrap()).unwrap();
    assert_eq!(provenance.build_id, artifact.build_id.clone().unwrap());
    assert_eq!(provenance.subjects.len(), 4);
    assert_eq!(
        provenance.parameters.network.as_deref(),
        Some("owned-xarxa")
    );
    assert_eq!(
        provenance.parameters.runtime_features,
        ImageClass::Correctness.build_features(Integration::OwnedXarxa.feature())
    );
    for name in ["embedded-lock", "bootstrap-lock"] {
        assert!(provenance.files.iter().any(|file| file.name == name));
    }
    assert!(provenance.source_reconstructable);
    let object_root = root.join("objects/sha256");
    let first_objects = collect_integrity_files(&object_root).unwrap();
    assert_eq!(first_objects.len(), 8);
    assert!(
        firmware::record(
            &mut first_session,
            ImageClass::Correctness,
            &test_artifacts(
                &application,
                &runtime_elf,
                &runtime_bin,
                &bootstrap_elf,
                &effective_embedded_lock,
            ),
        )
        .is_err()
    );
    assert_eq!(
        collect_integrity_files(&object_root).unwrap(),
        first_objects
    );
    drop(first_session);

    let second_run_directory = root.join("run-2");
    fs::create_dir(&second_run_directory).unwrap();
    let mut second = session(&second_run_directory);
    second
        .bind_source_snapshot(snapshot.directory(), &slots)
        .unwrap();
    firmware::record(
        &mut second,
        ImageClass::Correctness,
        &test_artifacts(
            &application,
            &runtime_elf,
            &runtime_bin,
            &bootstrap_elf,
            &effective_embedded_lock,
        ),
    )
    .unwrap();
    assert_eq!(
        collect_integrity_files(&object_root).unwrap(),
        first_objects
    );
    drop(second);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replayed_firmware_bundle_is_self_contained_after_origin_removal() {
    let root = temporary_directory("firmware-replay");
    let target_directory = root.join("target/hil/esp32s31");
    let runs_directory = target_directory.join("runs");
    let repository_root = target_directory.clone();
    fs::create_dir_all(&runs_directory).unwrap();
    write_test_build_materials(&repository_root);

    let application = repository_root.join("application.bin");
    let runtime_elf = repository_root.join("runtime.elf");
    let runtime_bin = repository_root.join("runtime.bin");
    let bootstrap_elf = repository_root.join("bootstrap.elf");
    let effective_embedded_lock = repository_root.join("hil/targets/esp32s31/Cargo.lock");
    fs::write(&application, b"application bytes").unwrap();
    fs::write(&runtime_elf, b"runtime elf").unwrap();
    fs::write(&runtime_bin, b"runtime bin").unwrap();
    fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

    let source_directory = runs_directory.join("source-run");
    fs::create_dir(&source_directory).unwrap();
    let mut source = session_for(&source_directory, &target_directory, &repository_root);
    let (_snapshot_root, snapshot) = oer_hil_source_snapshot::test_snapshot(&repository_root);
    let slots = root.join("slots/source-build");
    source
        .bind_source_snapshot(snapshot.directory(), &slots)
        .unwrap();
    firmware::record(
        &mut source,
        ImageClass::Correctness,
        &test_artifacts(
            &application,
            &runtime_elf,
            &runtime_bin,
            &bootstrap_elf,
            &effective_embedded_lock,
        ),
    )
    .unwrap();
    source.finish(Vec::new()).unwrap();

    let archived = oer_hil_evidence::verify::archived_firmware(
        &root,
        "esp32s31",
        "source-run",
        ImageClass::Correctness,
        &Recipe,
    )
    .unwrap();
    let replay_directory = runs_directory.join("replay-run");
    fs::create_dir(&replay_directory).unwrap();
    let mut replay = session_for(&replay_directory, &target_directory, &repository_root);
    let replayed_application = replay.record_replayed_firmware(&archived).unwrap();
    assert_eq!(
        fs::read(&replayed_application).unwrap(),
        b"application bytes"
    );
    assert_eq!(replay.manifest().firmware.len(), 1);
    assert_eq!(
        replay.manifest().firmware[0]
            .replayed_from
            .as_ref()
            .expect("replay origin")
            .source_run_id,
        "source-run"
    );
    replay.finish(Vec::new()).unwrap();

    fs::remove_dir_all(source_directory).unwrap();
    let verified =
        oer_hil_evidence::verify::verify(&root, "esp32s31", Some("replay-run"), &Recipe).unwrap();
    assert_eq!(verified.verified_run_ids, ["replay-run"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provenance_records_the_network_implementation_and_its_feature() {
    let root = temporary_directory("build-selection");
    write_test_build_materials(&root);
    let network = Integration::OwnedXarxa;
    let provenance = firmware::create_provenance(
        &root,
        (
            ImageClass::Correctness,
            network,
            None,
            &oer_hil_image_class::FeatureDelta::default(),
            ("esp32s31", TARGET, oer_chip_profile::Boot::Staged),
        ),
        "00".repeat(32),
        vec![],
        vec![],
        vec![],
        oer_hil_evidence::build::BuildEnvironment::synthetic(),
    )
    .unwrap();
    assert_eq!(provenance.parameters.network.as_deref(), Some(network.id()));
    assert!(
        provenance
            .parameters
            .runtime_features
            .split(',')
            .any(|feature| feature == network.feature())
    );
    fs::remove_dir_all(root).unwrap();
}
