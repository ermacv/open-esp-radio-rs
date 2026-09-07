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
