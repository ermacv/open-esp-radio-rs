use super::*;

#[test]
fn public_names_and_aliases_resolve_to_one_canonical_identity() {
    for (canonical, alias, expected) in [
        ("upstream-xarxa", "upstream", Integration::UpstreamXarxa),
        (
            "patched-xarxa",
            "udp-backpressure",
            Integration::PatchedXarxa,
        ),
    ] {
        assert_eq!(canonical.parse::<Integration>().unwrap(), expected);
        assert_eq!(alias.parse::<Integration>().unwrap(), expected);
        assert_eq!(expected.id(), canonical);
    }
    assert_eq!(Integration::default(), Integration::UpstreamXarxa);
    for unsupported in ["", "patched", "native", "owned-network"] {
        assert!(unsupported.parse::<Integration>().is_err());
    }
}

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
            Integration::PatchedXarxa,
            "patch"
        )
        .is_ok()
    );
    assert!(
        validate_identities(
            original.clone(),
            actual.clone(),
            Integration::UpstreamXarxa,
            "patch"
        )
        .is_err()
    );
    actual.remove(&entry("xarxa-driver", UPSTREAM));
    actual.insert(entry("xarxa-driver", "patch"));
    assert!(validate_identities(original, actual, Integration::PatchedXarxa, "patch").is_err());
}
#[test]
fn patch_must_apply_and_must_not_update_other_packages() {
    let original = BTreeSet::from([entry("xarxa", UPSTREAM)]);
    assert!(
        validate_identities(
            original.clone(),
            original.clone(),
            Integration::PatchedXarxa,
            "patch"
        )
        .is_err()
    );
    let mut actual = BTreeSet::from([entry("xarxa", "patch")]);
    actual.insert(entry("unexpected", "registry"));
    assert!(validate_identities(original, actual, Integration::PatchedXarxa, "patch").is_err());
}
#[test]
fn command_applies_no_patch_to_the_control() {
    let mut command = Command::new("cargo");
    Integration::UpstreamXarxa.configure(&mut command, Path::new("/repo"));
    assert_eq!(command.get_args().count(), 0);
    Integration::PatchedXarxa.configure(&mut command, Path::new("/repo"));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--config",
            "/repo/driver/network/dependencies/xarxa-patched.toml"
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
        include_str!("../../../../driver/network/dependencies/xarxa-patched.toml"),
    )
    .unwrap();
    let lock = workspace.join("Cargo.lock");
    let original = b"version = 4\n[[package]]\nname = 'local'\nversion = '0.1.0'\n";
    fs::write(&lock, original).unwrap();
    {
        let _selection = Selection::acquire(root, &workspace, Integration::PatchedXarxa).unwrap();
        assert!(Selection::acquire(root, &workspace, Integration::UpstreamXarxa).is_err());
        fs::write(&lock, "incomplete build output").unwrap();
    }
    assert_eq!(fs::read(&lock).unwrap(), original);
    assert!(Selection::acquire(root, &workspace, Integration::UpstreamXarxa).is_ok());
}

#[test]
fn example_selection_is_explicit_and_rejects_conflicting_contracts() {
    assert_eq!(
        Integration::for_example(None, &[]).unwrap(),
        Integration::UpstreamXarxa
    );
    for integration in [
        Integration::UpstreamXarxa,
        Integration::PatchedXarxa,
        Integration::UpstreamSmoltcp,
        Integration::OwnedXarxa,
    ] {
        assert_eq!(
            integration.id().parse::<Integration>().unwrap(),
            integration
        );
        assert_eq!(
            Integration::for_example(
                Some(integration),
                &[integration.feature().into(), "diagnostics".into()]
            )
            .unwrap(),
            integration
        );
        for other in ["upstream-network", "owned-network", "compat-network"] {
            if other != integration.feature() {
                assert!(Integration::for_example(Some(integration), &[other.into()]).is_err());
            }
        }
    }
    assert_eq!(
        Integration::for_example(None, &["compat-network".into()]).unwrap(),
        Integration::UpstreamSmoltcp
    );
    assert_eq!(
        Integration::for_example(None, &["owned-network".into()]).unwrap(),
        Integration::OwnedXarxa
    );
    assert!(
        Integration::for_example(None, &["owned-network".into(), "compat-network".into()]).is_err()
    );
}
