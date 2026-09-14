use std::{cell::RefCell, ffi::OsString, fs, path::PathBuf};

use super::*;

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn build_inputs(root: &Path) -> Artifacts {
    for relative in [
        "Cargo.lock",
        "hil/targets/esp32s31/Cargo.lock",
        "hil/targets/esp32s31/Cargo.toml",
        "hil/targets/esp32s31/stack.toml",
        "platform/esp32s31/Cargo.lock",
        "platform/esp32s31/partitions/applications.csv",
    ] {
        write(&root.join(relative), relative.as_bytes());
    }
    for (relative, bytes) in [
        ("build/application.bin", b"exact application".as_slice()),
        ("build/runtime.elf", b"runtime elf".as_slice()),
        ("build/runtime.bin", b"runtime bin".as_slice()),
        ("build/bootstrap.elf", b"bootstrap elf".as_slice()),
    ] {
        write(&root.join(relative), bytes);
    }
    Artifacts {
        network: Integration::UpstreamXarxa,
        output: root.join("build"),
        runtime_elf: root.join("build/runtime.elf"),
        runtime_bin: root.join("build/runtime.bin"),
        bootstrap_elf: root.join("build/bootstrap.elf"),
        effective_embedded_lock: root.join("hil/targets/esp32s31/Cargo.lock"),
        effective_bootstrap_lock: root.join("platform/esp32s31/Cargo.lock"),
        application_image: root.join("build/application.bin"),
    }
}

fn session(root: &Path) -> RunSession {
    RunSession::create(
        root,
        "esp32s31",
        "test-cell",
        "test-dut",
        Path::new("/test/no-device"),
        vec![OsString::from("test")],
    )
    .unwrap()
}

#[test]
fn built_firmware_is_archived_before_the_exact_run_local_path_is_flashed() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let build_path = artifacts.application_image.clone();
    let mut session = session(root.path());
    let run_directory = session.directory().to_owned();
    let flashed = RefCell::new(None::<PathBuf>);

    let failure = archive_and_flash_built(
        ImageClass::Correctness,
        artifacts,
        &mut session,
        |artifacts| {
            flashed.replace(Some(artifacts.application_image.clone()));
            assert_eq!(
                fs::read(&artifacts.application_image).unwrap(),
                b"exact application"
            );
            Ok(())
        },
    )
    .unwrap();

    assert!(failure.is_none());
    let flashed = flashed.into_inner().expect("flash call");
    assert_ne!(flashed, build_path);
    assert_eq!(
        flashed,
        run_directory.join("firmware/correctness/application.bin")
    );
}

#[test]
fn flash_failure_is_typed_after_archival() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let mut session = session(root.path());
    let failure = archive_and_flash_built(ImageClass::Correctness, artifacts, &mut session, |_| {
        Err("injected flash failure".into())
    })
    .unwrap()
    .expect("typed failure");
    assert_eq!(failure.kind, FailureKind::ImageFlash);
    assert_eq!(failure.message, "injected flash failure");
    assert!(
        session
            .directory()
            .join("firmware/correctness/application.bin")
            .is_file()
    );
}

#[test]
fn current_build_has_the_canonical_firmware_plan_identity() {
    for integration in [
        Integration::UpstreamXarxa,
        Integration::PatchedXarxa,
        Integration::UpstreamSmoltcp,
        Integration::OwnedXarxa,
    ] {
        assert!(matches!(
            RunFirmware::BuildCurrent(integration).plan(),
            PlannedFirmware::BuildCurrent
        ));
    }
}

#[test]
fn replay_import_is_archived_before_flashing_the_new_run_local_path() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let mut source = session(root.path());
    let source_id = source.id().to_owned();
    archive_and_flash_built(ImageClass::Correctness, artifacts, &mut source, |_| Ok(())).unwrap();
    source.finish(Vec::new()).unwrap();

    let archived = crate::evidence::verify::archived_firmware(
        root.path(),
        "esp32s31",
        &source_id,
        ImageClass::Correctness,
    )
    .unwrap();
    let mut replay = session(root.path());
    let replay_directory = replay.directory().to_owned();
    let flashed = RefCell::new(None::<PathBuf>);
    let failure = import_and_flash_replay(&archived, &mut replay, |application| {
        flashed.replace(Some(application.to_owned()));
        assert_eq!(fs::read(application).unwrap(), b"exact application");
        Ok(())
    })
    .unwrap();

    assert!(failure.is_none());
    assert_eq!(
        flashed.into_inner().expect("flash call"),
        replay_directory.join("firmware/correctness/application.bin")
    );
}

#[test]
fn corrupt_replay_is_rejected_before_flash_without_a_build_fallback() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let mut source = session(root.path());
    let source_id = source.id().to_owned();
    archive_and_flash_built(ImageClass::Correctness, artifacts, &mut source, |_| Ok(())).unwrap();
    source.finish(Vec::new()).unwrap();

    let archived = crate::evidence::verify::archived_firmware(
        root.path(),
        "esp32s31",
        &source_id,
        ImageClass::Correctness,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(
            &archived.application_path,
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(&archived.application_path)
            .unwrap()
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&archived.application_path, permissions).unwrap();
    }
    fs::write(&archived.application_path, b"corrupt application").unwrap();
    let mut replay = session(root.path());
    let flash_called = std::cell::Cell::new(false);
    let error = import_and_flash_replay(&archived, &mut replay, |_| {
        flash_called.set(true);
        Ok(())
    })
    .unwrap_err();

    assert!(!flash_called.get());
    assert!(
        error
            .to_string()
            .contains("changed after bundle verification"),
        "{error}"
    );
    assert_eq!(
        fs::read(
            replay
                .directory()
                .join("firmware/correctness/application.bin")
        )
        .unwrap(),
        b"corrupt application"
    );
}
