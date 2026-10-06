//! The checks re-exec is the same admitted CLI operation through Cargo.
#![cfg(not(feature = "checks"))]

use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

use oer_device_lock::{DeviceAccess, DeviceId};

const TEST: &str = "checks_reexec_preserves_delegated_device_access";
const DIRECTORY: &str = "OER_FW_CHECKS_TEST_DIRECTORY";
const MAC: &str = "00:11:22:33:44:66";

#[test]
fn checks_reexec_preserves_delegated_device_access() {
    if std::env::var_os("OER_FW_CHECKS_TEST_CHILD").is_some() {
        let directory = PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
        let access = DeviceAccess::try_acquire_in(&directory, &MAC.parse().unwrap(), "checked fw")
            .unwrap()
            .unwrap();
        assert!(access.is_delegated());
        let _operation = access.operation().unwrap();
        let context = oer_process::Context::current().unwrap();
        assert_eq!(context.get("stand.lease"), Some("enclosing-lease"));
        assert_eq!(context.get("stand.job"), Some("same-job"));
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let id: DeviceId = MAC.parse().unwrap();
    let access = DeviceAccess::try_acquire_in(directory.path(), &id, "lease owner")
        .unwrap()
        .unwrap();
    let mut context = oer_process::Context::default()
        .with("stand.lease", "enclosing-lease")
        .with("stand.job", "same-job");
    access.delegate(&mut context).unwrap();
    let cargo = directory.path().join("cargo");
    fs::write(
        &cargo,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"${DIRECTORY}/arguments\"\n\
         exec \"{}\" --exact {TEST}\n",
            std::env::current_exe().unwrap().display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
    for wait in [false, true] {
        let mut fw = oer_process::command(env!("CARGO_BIN_EXE_oer-fw"));
        fw.args([
            "flash",
            "fixture-image",
            "--check",
            "stack",
            "--device",
            MAC,
        ])
        .env("CARGO", &cargo)
        .env(DIRECTORY, directory.path())
        .env("OER_FW_CHECKS_TEST_CHILD", "1");
        if wait {
            fw.arg("--wait");
        }
        context.apply(&mut fw).unwrap();
        oer_process::run_with_timeout(&mut fw, std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(
            fs::read_to_string(directory.path().join("arguments")).unwrap(),
            format!(
                "run\n--quiet\n-p\noer-fw\n--features\nchecks\n--\nflash\nfixture-image\n--check\nstack\n--device\n{MAC}\n{}",
                if wait { "--wait\n" } else { "" }
            )
        );
    }
}
