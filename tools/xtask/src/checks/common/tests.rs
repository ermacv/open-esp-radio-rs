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
    validate_production_edges(&packages, &Families::new()).unwrap();
}

#[test]
fn optional_and_build_dependencies_cannot_hide_production_to_research_edges() {
    for section in ["dependencies", "build-dependencies"] {
        let repository = architecture_repository(section, "experiment");
        let context = Context::new(repository.path()).unwrap();
        let packages = production_packages(&context).unwrap();
        let error = validate_production_edges(&packages, &Families::new())
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
    let error = validate_production_edges(&packages, &Families::new())
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
    metadata.remove("family");
    // A family package names its family where a chip package names its chip.
    let key = if platform == "family" {
        "family"
    } else {
        "chip"
    };
    if let Some(identity) = chip {
        metadata.insert(key.into(), identity.into());
    }
    fs::write(manifest, toml::to_string(&doc).unwrap()).unwrap();
}

/// Two chips of the `espressif` family and one chip of another.
fn test_families() -> Families {
    [
        ("esp32s31", "espressif"),
        ("esp32c5", "espressif"),
        ("esp32x9", "other"),
    ]
    .into_iter()
    .map(|(chip, family)| (chip.to_owned(), family.to_owned()))
    .collect()
}

fn edge_result(repository: &Path) -> Result<()> {
    let context = Context::new(repository)?;
    validate_production_edges(&production_packages(&context)?, &test_families())
}

#[test]
fn only_adapters_and_compositions_may_depend_on_an_executor() {
    for (layer, allowed) in [
        ("runtime", false),
        ("role", false),
        ("hardware", false),
        ("service", false),
        ("protocol", false),
        ("adapter", true),
        ("composition", true),
    ] {
        let repository = architecture_repository("dev-dependencies", "contract");
        set_classification(
            repository.path(),
            "libraries/policy",
            layer,
            "portable",
            None,
        );
        let manifest = repository.path().join("libraries/policy/Cargo.toml");
        let mut doc: toml::Value = toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
        doc.as_table_mut()
            .unwrap()
            .entry("dependencies")
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .unwrap()
            .insert("embassy-executor".into(), "0.9".into());
        fs::write(manifest, toml::to_string(&doc).unwrap()).unwrap();
        let result = edge_result(repository.path());
        assert_eq!(result.is_ok(), allowed, "{layer}: {result:?}");
        if let Err(error) = result {
            assert!(error.to_string().contains("depends on executor"), "{error}");
        }
    }
}

#[test]
fn only_execution_layers_may_depend_on_the_time_driver() {
    for (layer, allowed) in [
        ("contract", false),
        ("protocol", false),
        ("hardware", false),
        ("role", false),
        ("service", false),
        ("runtime", true),
        ("adapter", true),
        ("composition", true),
    ] {
        let repository = architecture_repository("dev-dependencies", "contract");
        set_classification(
            repository.path(),
            "libraries/policy",
            layer,
            "portable",
            None,
        );
        let manifest = repository.path().join("libraries/policy/Cargo.toml");
        let mut doc: toml::Value = toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
        doc.as_table_mut()
            .unwrap()
            .entry("dependencies")
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .unwrap()
            .insert("embassy-time".into(), "0.5".into());
        fs::write(manifest, toml::to_string(&doc).unwrap()).unwrap();
        let result = edge_result(repository.path());
        assert_eq!(result.is_ok(), allowed, "{layer}: {result:?}");
        if let Err(error) = result {
            assert!(error.to_string().contains("depends on time driver"), "{error}");
        }
    }
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
fn shared_code_reaches_a_chip_only_through_the_selected_pac() {
    let repository = architecture_repository("dependencies", "hardware");
    let edge = |source: (&str, Option<&str>), target: (&str, Option<&str>)| {
        set_classification(
            repository.path(),
            "libraries/policy",
            "hardware",
            source.0,
            source.1,
        );
        set_classification(
            repository.path(),
            "crates/target",
            "hardware",
            target.0,
            target.1,
        );
        edge_result(repository.path())
    };
    // Only `oer-pac` may reach a chip package from shared code.
    assert!(
        edge(("selected", None), ("chip", Some("esp32c5")))
            .unwrap_err()
            .to_string()
            .contains("incompatible platform edge")
    );
    edge(("selected", None), ("selected", None)).unwrap();
    edge(("chip", Some("esp32s31")), ("selected", None)).unwrap();
    assert!(edge(("portable", None), ("selected", None)).is_err());
}

#[test]
fn family_code_is_shared_only_within_its_family() {
    let repository = architecture_repository("dependencies", "hardware");
    let edge = |source: (&str, Option<&str>), target: (&str, Option<&str>)| {
        set_classification(
            repository.path(),
            "libraries/policy",
            "hardware",
            source.0,
            source.1,
        );
        set_classification(
            repository.path(),
            "crates/target",
            "hardware",
            target.0,
            target.1,
        );
        edge_result(repository.path())
    };
    let espressif = ("family", Some("espressif"));
    for chip in ["esp32s31", "esp32c5"] {
        edge(("chip", Some(chip)), espressif).unwrap();
        // Family code never reaches one chip of its family.
        assert!(edge(espressif, ("chip", Some(chip))).is_err());
    }
    edge(espressif, espressif).unwrap();
    edge(espressif, ("portable", None)).unwrap();
    for source in [
        ("chip", Some("esp32x9")),
        ("family", Some("other")),
        ("portable", None),
        ("host", None),
        ("selected", None),
    ] {
        let error = edge(source, espressif).unwrap_err().to_string();
        assert!(
            error.contains("incompatible platform edge"),
            "{source:?}: {error}"
        );
    }
    assert!(edge(espressif, ("selected", None)).is_err());
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
        for layer in ["role", "adapter", "runtime", "service", "composition"] {
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
fn services_declare_ports_that_adapters_bind_and_roles_stay_below_execution() {
    for (source, target, allowed) in [
        ("service", "adapter", false),
        ("service", "runtime", false),
        ("service", "role", false),
        ("service", "hardware", false),
        ("adapter", "service", true),
        ("runtime", "service", true),
        ("role", "hardware", true),
        ("role", "protocol", true),
        ("role", "adapter", false),
        ("role", "runtime", false),
        ("role", "service", false),
        ("adapter", "role", true),
        ("runtime", "role", true),
        ("composition", "role", true),
    ] {
        let repository = architecture_repository("dependencies", target);
        set_classification(
            repository.path(),
            "libraries/policy",
            source,
            "portable",
            None,
        );
        assert_eq!(
            edge_result(repository.path()).is_ok(),
            allowed,
            "{source} -> {target}"
        );
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
        ("family", None),
        ("family", Some("Espressif")),
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

    let mut duplicate = package.clone();
    duplicate.metadata["open-radio"]["supported-feature-profiles"] =
        serde_json::json!(["left", "left"]);
    let error = declared_profiles(&duplicate).unwrap_err().to_string();
    assert!(
        error.contains("repeats supported feature profile"),
        "{error}"
    );

    let mut sets = package.clone();
    sets.metadata["open-radio"]["test-feature-sets"] = serde_json::json!(["left", "left,right"]);
    assert_eq!(test_feature_sets(&sets).unwrap(), ["left", "left,right"]);
    sets.metadata["open-radio"]["test-feature-sets"] = serde_json::json!(["missing"]);
    let error = test_feature_sets(&sets).unwrap_err().to_string();
    assert!(error.contains("invalid test feature set"), "{error}");

    let mut unknown = package.clone();
    unknown.metadata["open-radio"]["supported-feature-profiles"] =
        serde_json::json!(["left,missing"]);
    let error = declared_profiles(&unknown).unwrap_err().to_string();
    assert!(
        error.contains("invalid supported feature profile"),
        "{error}"
    );
}

#[test]
fn source_package_discovery_covers_independent_and_ignored_workspace_members() {
    let repository = tempfile::tempdir().unwrap();
    fs::write(
        repository.path().join("Cargo.toml"),
        "[workspace]\nresolver='3'\nmembers=['new']\nexclude=['island']\n",
    )
    .unwrap();
    fs::write(repository.path().join(".gitignore"), "/new/\n").unwrap();
    for (directory, name) in [("new", "new-package"), ("island", "island-package")] {
        fs::create_dir_all(repository.path().join(directory).join("src")).unwrap();
        fs::write(
            repository.path().join(directory).join("Cargo.toml"),
            format!(
                "[package]\nname='{name}'\nversion='0.0.0'\nedition='2024'\n[package.metadata.open-radio]\nscope='production'\nlayer='contract'\nplatform='portable'\n{}",
                if directory == "island" { "[workspace]\n" } else { "" }
            ),
        )
        .unwrap();
        fs::write(repository.path().join(directory).join("src/lib.rs"), "").unwrap();
    }
    let context = Context::new(repository.path()).unwrap();
    crate::process::run(context.command("git").args(["init", "--quiet"])).unwrap();
    crate::process::run(
        context
            .command("git")
            .args(["add", "Cargo.toml", ".gitignore", "island"]),
    )
    .unwrap();
    for manifest in ["Cargo.toml", "island/Cargo.toml"] {
        crate::process::run(context.cargo().args([
            "generate-lockfile",
            "--offline",
            "--manifest-path",
            manifest,
        ]))
        .unwrap();
    }
    let packages = source_packages(&context).unwrap();
    assert_eq!(
        packages
            .iter()
            .map(|package| package.package.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["island-package", "new-package"])
    );
}

#[test]
fn package_names_share_one_prefix_and_only_the_facade_is_branded() {
    for (name, layer) in [
        ("oer-ieee80211-sta", "protocol"),
        ("oer-esp32s31-ieee80211-embassy-net-upstream", "adapter"),
        ("oer-hil-runner", "hil"),
        ("open-esp-radio", "facade"),
    ] {
        validate_package_name(name, layer).unwrap();
    }
    for (name, layer) in [
        ("open-esp-radio-hil-runner", "hil"),
        ("open-esp-radio-register-model", "tool"),
        ("oer-Wifi", "protocol"),
        ("oer--mac", "protocol"),
        ("oer-", "protocol"),
        ("oer-radio", "facade"),
    ] {
        assert!(validate_package_name(name, layer).is_err(), "{name}");
    }
}

#[test]
fn only_verification_packages_declare_an_evidence_role() {
    assert_eq!(
        evidence_role("v", "verification", Some("verdict")).unwrap(),
        Some(Evidence::Verdict)
    );
    assert_eq!(
        evidence_role("r", "verification", Some("report")).unwrap(),
        Some(Evidence::Report)
    );
    assert!(evidence_role("v", "verification", None).is_err());
    assert!(evidence_role("v", "verification", Some("other")).is_err());
    assert_eq!(evidence_role("t", "tool", None).unwrap(), None);
    assert!(evidence_role("t", "tool", Some("verdict")).is_err());
}

#[test]
fn a_verdict_never_depends_on_a_report() {
    let (verdict, report) = (Some(Evidence::Verdict), Some(Evidence::Report));
    assert!(!evidence_edge_allowed(verdict, report));
    assert!(evidence_edge_allowed(report, verdict));
    assert!(evidence_edge_allowed(verdict, verdict));
    assert!(evidence_edge_allowed(None, report));
}

#[test]
fn only_hil_packages_declare_a_hil_role() {
    assert_eq!(
        hil_role("o", "hil", Some("observation")).unwrap(),
        Some(Hil::Observation)
    );
    assert_eq!(
        hil_role("s", "hil", Some("operation")).unwrap(),
        Some(Hil::Operation)
    );
    assert!(hil_role("o", "hil", None).is_err());
    assert!(hil_role("o", "hil", Some("other")).is_err());
    assert_eq!(hil_role("t", "tool", None).unwrap(), None);
    assert!(hil_role("t", "tool", Some("observation")).is_err());
}

#[test]
fn observation_never_depends_on_stand_operation() {
    let (observation, operation) = (Some(Hil::Observation), Some(Hil::Operation));
    assert!(!hil_edge_allowed(observation, operation));
    assert!(hil_edge_allowed(operation, observation));
    assert!(hil_edge_allowed(observation, observation));
    assert!(hil_edge_allowed(None, operation));
}

#[test]
fn a_chip_package_compiles_for_its_own_chip_target() {
    let repository = architecture_repository("dependencies", "contract");
    set_classification(
        repository.path(),
        "libraries/policy",
        "service",
        "chip",
        Some("esp32x9"),
    );
    let platform = repository.path().join("platform/esp32x9");
    fs::create_dir_all(&platform).unwrap();
    fs::write(
        platform.join("chip.toml"),
        "schema = 1\nid = \"esp32x9\"\nfamily = \"vendor\"\n\
         rust-target = \"riscv32imac-unknown-none-elf\"\n\
         boot = \"esp-idf-bootloader\"\nespflash-chip = \"esp32x9\"\nrevisions = [\"rev0\"]\n\
         [properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = false\ncores = 1\n",
    )
    .unwrap();
    let context = Context::new(repository.path()).unwrap();
    let packages = production_packages(&context).unwrap();
    let configurations = architecture_configurations(
        repository.path(),
        &packages,
        "riscv32imafc-unknown-none-elf",
    )
    .unwrap();
    let target = |package: &str| {
        configurations
            .iter()
            .filter(|configuration| configuration.package == package)
            .map(|configuration| configuration.target.as_str())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        target("policy"),
        BTreeSet::from(["riscv32imac-unknown-none-elf"])
    );
    assert_eq!(
        target("target-library"),
        BTreeSet::from(["riscv32imafc-unknown-none-elf"])
    );
}

#[test]
fn a_family_package_compiles_for_every_target_of_its_family() {
    let repository = architecture_repository("dependencies", "contract");
    set_classification(
        repository.path(),
        "libraries/policy",
        "hardware",
        "family",
        Some("vendor"),
    );
    for (chip, family, target) in [
        ("esp32x7", "vendor", "riscv32imafc-unknown-none-elf"),
        ("esp32x8", "vendor", "riscv32imac-unknown-none-elf"),
        ("esp32x9", "other", "xtensa-esp32-none-elf"),
    ] {
        let platform = repository.path().join("platform").join(chip);
        fs::create_dir_all(&platform).unwrap();
        fs::write(
            platform.join("chip.toml"),
            format!(
                "schema = 1\nid = \"{chip}\"\nfamily = \"{family}\"\n\
                 rust-target = \"{target}\"\nboot = \"esp-idf-bootloader\"\n\
                 espflash-chip = \"{chip}\"\nrevisions = [\"rev0\"]\n\
                 [properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\n\
                 ieee802154 = false\ncores = 1\n"
            ),
        )
        .unwrap();
    }
    let context = Context::new(repository.path()).unwrap();
    let packages = production_packages(&context).unwrap();
    let configurations =
        architecture_configurations(repository.path(), &packages, "host-target").unwrap();
    let targets = configurations
        .iter()
        .filter(|configuration| configuration.package == "policy")
        .map(|configuration| configuration.target.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        targets,
        BTreeSet::from([
            "riscv32imac-unknown-none-elf",
            "riscv32imafc-unknown-none-elf"
        ])
    );
    set_classification(
        repository.path(),
        "libraries/policy",
        "hardware",
        "family",
        Some("absent"),
    );
    let packages = production_packages(&context).unwrap();
    let error = architecture_configurations(repository.path(), &packages, "host-target")
        .err()
        .expect("a family no chip declares must fail")
        .to_string();
    assert!(error.contains("which no chip declares"), "{error}");
}
