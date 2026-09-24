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
            "/repo/crates/network/dependencies/xarxa-patched.toml"
        ]
    );
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
        for other in ["upstream-network", "owned-network", "embassy-network"] {
            if other != integration.feature() {
                assert!(Integration::for_example(Some(integration), &[other.into()]).is_err());
            }
        }
    }
    assert_eq!(
        Integration::for_example(None, &["embassy-network".into()]).unwrap(),
        Integration::UpstreamSmoltcp
    );
    assert_eq!(
        Integration::for_example(None, &["owned-network".into()]).unwrap(),
        Integration::OwnedXarxa
    );
    assert!(
        Integration::for_example(None, &["owned-network".into(), "embassy-network".into()])
            .is_err()
    );
}

fn workspace_fixture() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let workspace = root.join("application");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(root.join(CONFIG).parent().unwrap()).unwrap();
    fs::write(
        root.join(CONFIG),
        include_str!("../../../../crates/network/dependencies/xarxa-patched.toml"),
    )
    .unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = 'application'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(workspace.join("src/lib.rs"), "").unwrap();
    fs::write(workspace.join("Cargo.lock"), "version = 4\n").unwrap();
    (directory, workspace)
}

#[test]
fn build_lock_copies_the_committed_catalog_and_owns_its_directory() {
    let (directory, workspace) = workspace_fixture();
    let output = directory.path().join("output");
    let lock = BuildLock::prepare(&workspace, &output).unwrap();
    assert_eq!(lock.path(), output.join("Cargo.lock"));
    assert_eq!(
        fs::read(lock.path()).unwrap(),
        fs::read(workspace.join("Cargo.lock")).unwrap()
    );
    let error = BuildLock::prepare(&workspace, &output)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("another build owns"), "{error}");
    // Another output directory builds the same workspace concurrently.
    let _other = BuildLock::prepare(&workspace, &directory.path().join("other")).unwrap();
    // dup shares the open file description as inheritance across fork does;
    // releasing the owner must not depend on that descriptor closing.
    let inherited = lock._lease.0.try_clone().unwrap();
    drop(lock);
    BuildLock::prepare(&workspace, &output).unwrap();
    drop(inherited);
}

#[test]
fn cargo_resolves_into_the_private_copy_and_leaves_the_catalog_untouched() {
    let (directory, workspace) = workspace_fixture();
    let committed = fs::read(workspace.join("Cargo.lock")).unwrap();
    let lock = BuildLock::prepare(&workspace, &directory.path().join("output")).unwrap();
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .args([
            "metadata",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(workspace.join("Cargo.toml"));
    lock.configure(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(workspace.join("Cargo.lock")).unwrap(), committed);
    let resolved = fs::read_to_string(lock.path()).unwrap();
    assert!(resolved.contains("name = \"application\""), "{resolved}");
}

#[test]
fn validation_compares_the_copy_with_the_committed_catalog() {
    let (directory, workspace) = workspace_fixture();
    let root = directory.path();
    let catalog = |xarxa: &str, other: &str| {
        format!(
            "version = 4\n[[package]]\nname = 'xarxa'\nversion = '0.1.0'\nsource = '{xarxa}'\n\
             [[package]]\nname = 'other'\nversion = '{other}'\n"
        )
    };
    fs::write(workspace.join("Cargo.lock"), catalog(UPSTREAM, "1.0.0")).unwrap();
    let lock = BuildLock::prepare(&workspace, &root.join("output")).unwrap();
    lock.validate(root, Integration::UpstreamXarxa).unwrap();
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(root.join(CONFIG)).unwrap()).unwrap();
    let spec = &config["patch"]["https://github.com/embassy-rs/xarxa"]["xarxa"];
    let (git, rev) = (spec["git"].as_str().unwrap(), spec["rev"].as_str().unwrap());
    let patched = format!("git+{git}?rev={rev}#{rev}");
    fs::write(lock.path(), catalog(&patched, "1.0.0")).unwrap();
    lock.validate(root, Integration::PatchedXarxa).unwrap();
    assert!(lock.validate(root, Integration::UpstreamXarxa).is_err());
    fs::write(lock.path(), catalog(&patched, "2.0.0")).unwrap();
    assert!(lock.validate(root, Integration::PatchedXarxa).is_err());
}
