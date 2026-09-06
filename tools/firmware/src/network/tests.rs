use super::*;

fn entry(name: &str, source: &str) -> Identity {
    (name.to_owned(), "0.1.0".to_owned(), Some(source.to_owned()))
}
#[test]
fn selection_changes_only_xarxa_and_preserves_the_driver_identity() {
    let original = BTreeSet::from([
        entry("xarxa", UPSTREAM),
        entry("xarxa-driver", UPSTREAM),
        entry("embassy-net", "original-embassy"),
    ]);
    let mut actual = original.clone();
    actual.remove(&entry("xarxa", UPSTREAM));
    actual.insert(entry("xarxa", "patch"));
    assert!(
        validate_identities(
            original.clone(),
            actual.clone(),
            Integration::UdpBackpressure,
            "patch"
        )
        .is_ok()
    );
    assert!(
        validate_identities(
            original.clone(),
            actual.clone(),
            Integration::Upstream,
            "patch"
        )
        .is_err()
    );
    actual.remove(&entry("xarxa-driver", UPSTREAM));
    actual.insert(entry("xarxa-driver", "patch"));
    assert!(validate_identities(original, actual, Integration::UdpBackpressure, "patch").is_err());
}
#[test]
fn patch_must_apply_and_must_not_update_other_packages() {
    let original = BTreeSet::from([entry("xarxa", UPSTREAM)]);
    assert!(
        validate_identities(
            original.clone(),
            original.clone(),
            Integration::UdpBackpressure,
            "patch"
        )
        .is_err()
    );
    let mut actual = BTreeSet::from([entry("xarxa", "patch")]);
    actual.insert(entry("unexpected", "registry"));
    assert!(validate_identities(original, actual, Integration::UdpBackpressure, "patch").is_err());
}
#[test]
fn command_applies_no_patch_to_the_control() {
    let mut command = Command::new("cargo");
    Integration::Upstream.configure(&mut command, Path::new("/repo"));
    assert_eq!(command.get_args().count(), 0);
    Integration::UdpBackpressure.configure(&mut command, Path::new("/repo"));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--config",
            "/repo/driver/network/adapters/xarxa/backpressure/cargo-config.toml"
        ]
    );
}

#[test]
fn failed_build_restores_catalog_and_releases_workspace_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let workspace = root.join("application");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(root.join(CONFIG).parent().unwrap()).unwrap();
    fs::write(
        root.join(CONFIG),
        include_str!("../../../../driver/network/adapters/xarxa/backpressure/cargo-config.toml"),
    )
    .unwrap();
    let lock = workspace.join("Cargo.lock");
    let original = b"version = 4\n[[package]]\nname = 'local'\nversion = '0.1.0'\n";
    fs::write(&lock, original).unwrap();
    {
        let _selection =
            Selection::acquire(root, &workspace, Integration::UdpBackpressure).unwrap();
        assert!(Selection::acquire(root, &workspace, Integration::Upstream).is_err());
        fs::write(&lock, "incomplete build output").unwrap();
    }
    assert_eq!(fs::read(&lock).unwrap(), original);
    assert!(Selection::acquire(root, &workspace, Integration::Upstream).is_ok());
}
