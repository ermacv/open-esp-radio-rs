use super::*;
use std::fs;

#[test]
fn ignored_production_workspace_member_remains_in_compiled_audit_inventory() {
    let repository = tempfile::tempdir().unwrap();
    fs::write(
        repository.path().join("Cargo.toml"),
        "[workspace]\nresolver = '3'\nmembers = ['crates/visible', 'crates/ignored']\n",
    )
    .unwrap();
    fs::write(repository.path().join(".gitignore"), "/crates/ignored/\n").unwrap();
    for name in ["visible", "ignored"] {
        let directory = repository.path().join("crates").join(name);
        fs::create_dir_all(directory.join("src")).unwrap();
        fs::write(
            directory.join("Cargo.toml"),
            format!("[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n[package.metadata.open-radio]\nscope = 'production'\nlayer = 'contract'\nplatform = 'portable'\n"),
        )
        .unwrap();
        fs::write(directory.join("src/lib.rs"), "").unwrap();
    }
    let context = Context::new(repository.path()).unwrap();
    crate::process::run(context.command("git").args(["init", "--quiet"])).unwrap();
    assert!(
        !paths::source_manifests(&context)
            .unwrap()
            .iter()
            .any(|path| path.ends_with("crates/ignored/Cargo.toml"))
    );
    // Metadata only; this test never builds a nested Cargo workspace.
    let packages = production_packages(&context).unwrap();
    assert_eq!(
        packages
            .iter()
            .map(|package| package.package.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["ignored", "visible"])
    );
    assert!(packages.iter().all(|package| package.workspace_member));
}

fn architecture_repository(dependency_section: &str, target_layer: &str) -> tempfile::TempDir {
    let repository = tempfile::tempdir().unwrap();
    fs::write(
        repository.path().join("Cargo.toml"),
        "[workspace]\nresolver = '3'\nmembers = ['libraries/policy', 'crates/target']\n",
    )
    .unwrap();
    for (path, name, scope, layer) in [
        ("libraries/policy", "policy", "production", "service"),
        (
            "crates/target",
            "target-library",
            if target_layer == "experiment" {
                "experimental"
            } else {
                "production"
            },
            target_layer,
        ),
    ] {
        let directory = repository.path().join(path);
        fs::create_dir_all(directory.join("src")).unwrap();
        fs::write(directory.join("src/lib.rs"), "").unwrap();
        let edge = if name == "policy" {
            format!(
                "[{dependency_section}]\ntarget-library = {{ path = '../../crates/target', optional = {} }}\n",
                dependency_section != "dev-dependencies"
            )
        } else {
            String::new()
        };
        fs::write(directory.join("Cargo.toml"), format!(
            "[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n[package.metadata.open-radio]\nscope = '{scope}'\nlayer = '{layer}'\nplatform = 'portable'\n{edge}"
        )).unwrap();
    }
    let context = Context::new(repository.path()).unwrap();
    crate::process::run(context.command("git").args(["init", "--quiet"])).unwrap();
    repository
}

#[test]
fn classification_discovers_production_outside_crates_and_excludes_experiments_inside() {
    let repository = architecture_repository("dev-dependencies", "experiment");
    let context = Context::new(repository.path()).unwrap();
    let packages = production_packages(&context).unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].package.name.as_str(), "policy");
    validate_production_edges(&packages).unwrap();
}

#[test]
fn optional_and_build_dependencies_cannot_hide_production_to_research_edges() {
    for section in ["dependencies", "build-dependencies"] {
        let repository = architecture_repository(section, "experiment");
        let context = Context::new(repository.path()).unwrap();
        let packages = production_packages(&context).unwrap();
        let error = validate_production_edges(&packages)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("non-production package target-library"),
            "{error}"
        );
    }
}

#[test]
fn internal_packages_cannot_depend_on_the_public_facade() {
    let repository = architecture_repository("dependencies", "facade");
    let context = Context::new(repository.path()).unwrap();
    let packages = production_packages(&context).unwrap();
    let error = validate_production_edges(&packages)
        .unwrap_err()
        .to_string();
    assert!(error.contains("depends on public facade"), "{error}");
}

#[test]
fn unclassified_package_cannot_disappear_from_architecture_checks() {
    let repository = architecture_repository("dev-dependencies", "experiment");
    let manifest = repository.path().join("crates/target/Cargo.toml");
    let content = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        content
            .split("[package.metadata.open-radio]")
            .next()
            .unwrap(),
    )
    .unwrap();
    let context = Context::new(repository.path()).unwrap();
    let error = production_packages(&context)
        .err()
        .expect("missing classification must fail")
        .to_string();
    assert!(error.contains("lacks open-radio.scope"), "{error}");
}

fn set_classification(
    repository: &Path,
    relative: &str,
    layer: &str,
    platform: &str,
    chip: Option<&str>,
) {
    let manifest = repository.join(relative).join("Cargo.toml");
    let mut doc: toml::Value = toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    let metadata = doc["package"]["metadata"]["open-radio"]
        .as_table_mut()
        .unwrap();
    metadata.insert("layer".into(), layer.into());
    metadata.insert("platform".into(), platform.into());
    metadata.remove("chip");
    if let Some(chip) = chip {
        metadata.insert("chip".into(), chip.into());
    }
    fs::write(manifest, toml::to_string(&doc).unwrap()).unwrap();
}

fn edge_result(repository: &Path) -> Result<()> {
    let context = Context::new(repository)?;
    validate_production_edges(&production_packages(&context)?)
}

#[test]
fn portable_to_chip_rejection_does_not_depend_on_the_chip_name() {
    for chip in ["esp32s31", "esp32c5"] {
        let repository = architecture_repository("dependencies", "hardware");
        set_classification(
            repository.path(),
            "libraries/policy",
            "adapter",
            "portable",
            None,
        );
        set_classification(
            repository.path(),
            "crates/target",
            "hardware",
            "chip",
            Some(chip),
        );
        let error = edge_result(repository.path()).unwrap_err().to_string();
        assert!(error.contains("incompatible platform edge"), "{error}");
        set_classification(
            repository.path(),
            "libraries/policy",
            "facade",
            "portable",
            None,
        );
        edge_result(repository.path()).unwrap();
    }
}

#[test]
fn hardware_dependencies_cannot_cross_chip_identity() {
    let repository = architecture_repository("dependencies", "hardware");
    set_classification(
        repository.path(),
        "libraries/policy",
        "hardware",
        "chip",
        Some("esp32s31"),
    );
    set_classification(
        repository.path(),
        "crates/target",
        "hardware",
        "chip",
        Some("esp32c5"),
    );
    assert!(
        edge_result(repository.path())
            .unwrap_err()
            .to_string()
            .contains("incompatible platform edge")
    );
    set_classification(
        repository.path(),
        "crates/target",
        "hardware",
        "chip",
        Some("esp32s31"),
    );
    edge_result(repository.path()).unwrap();
}

#[test]
fn hardware_cannot_depend_on_execution_or_composition_even_optionally() {
    for section in ["dependencies", "build-dependencies"] {
        for layer in ["adapter", "runtime", "service", "composition"] {
            let repository = architecture_repository(section, layer);
            set_classification(
                repository.path(),
                "libraries/policy",
                "hardware",
                "chip",
                Some("esp32c5"),
            );
            set_classification(
                repository.path(),
                "crates/target",
                layer,
                "chip",
                Some("esp32c5"),
            );
            let error = edge_result(repository.path()).unwrap_err().to_string();
            assert!(error.contains("forbidden architecture edge"), "{error}");
        }
    }
}

#[test]
fn platform_category_and_chip_identity_must_agree() {
    for (platform, chip) in [
        ("chip", None),
        ("chip", Some("")),
        ("chip", Some("ESP32-S31")),
        ("chip", Some("esp32/s31")),
        ("portable", Some("esp32s31")),
        ("host", Some("esp32s31")),
        ("esp32s31", None),
    ] {
        let repository = architecture_repository("dependencies", "contract");
        set_classification(
            repository.path(),
            "libraries/policy",
            "service",
            platform,
            chip,
        );
        assert!(
            edge_result(repository.path()).is_err(),
            "{platform} {chip:?}"
        );
    }
}

#[test]
fn declared_alternatives_preserve_minimum_and_default_compilation() {
    let repository = architecture_repository("dependencies", "contract");
    set_classification(
        repository.path(),
        "libraries/policy",
        "facade",
        "portable",
        None,
    );
    let manifest = repository.path().join("libraries/policy/Cargo.toml");
    let mut doc: toml::Value = toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    doc["package"]["metadata"]["open-radio"]
        .as_table_mut()
        .unwrap()
        .insert(
            "supported-feature-profiles".into(),
            toml::Value::Array(vec!["left".into(), "right".into()]),
        );
    doc.as_table_mut().unwrap().insert(
        "features".into(),
        toml::toml! { left = [] right = [] }.into(),
    );
    fs::write(&manifest, toml::to_string(&doc).unwrap()).unwrap();
    let context = Context::new(repository.path()).unwrap();
    let packages = production_packages(&context).unwrap();
    let package = &packages
        .iter()
        .find(|p| p.package.name == "policy")
        .unwrap()
        .package;
    let profiles = compilation_profiles(package).unwrap();
    assert!(profiles.contains(&vec![]));
    assert!(profiles.contains(&vec!["--no-default-features".into()]));
    assert!(
        !profiles
            .iter()
            .flatten()
            .any(|flag| flag == "--all-features")
    );
    assert!(
        profiles
            .iter()
            .any(|flags| flags.last().is_some_and(|flag| flag == "left"))
    );
    assert!(
        profiles
            .iter()
            .any(|flags| flags.last().is_some_and(|flag| flag == "right"))
    );

    // A lower composition can require a choice even though the facade must
    // remain usable with no features. Its default is still a supported build.
    let mut composition = package.clone();
    composition.metadata["open-radio"]["layer"] = "composition".into();
    let profiles = compilation_profiles(&composition).unwrap();
    assert!(profiles.contains(&vec![]));
    assert!(!profiles.contains(&vec!["--no-default-features".into()]));
    assert_eq!(profiles.len(), 3);
}
