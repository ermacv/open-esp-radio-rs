use super::*;

#[test]
fn resolves_composed_specs_relative_to_the_project() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-workbench-project-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &path,
        r#"
schema = 1
id = "fixture"
target-spec = "target.spec"
run-spec = "local.run"
memory-map = "memory.toml"
svd = ["registers/base.svd"]

[analysis.symbols]
output = "generated/symbols.json"

[analysis.navigation]
output = "generated/navigation.json"

[[analysis.ir]]
id = "vendor"
sources = ["rom", "archive"]
symbol-prefix = "phy_"
include-reachable = true
output = "generated/vendor.ir.json"
pseudo-rust = "generated/vendor.pseudo.rs"

[registers]
facts = "generated/mmio.json"
model = "registers/reviewed.toml"

[registers.review]
output = "generated/register-review.md"
linked-ir = ["generated/vendor.ir.json"]

[registers.svd]
output = "generated/device.svd"

[registers.pac]
output = "generated/pac/src/lib.rs"
target = "none"
edition = "2024"

[registers.bindings]
output = "generated/device.bindings"
crate-name = "fixture_pac"

[registers.api]
pack = "registers/api.toml"

[registers.lints]
pack = "registers/lints.toml"

[registers.evidence]
catalogs = ["registers/evidence.toml"]

[interfaces]
facts = "generated/interfaces.json"
pack = "interfaces/reviewed.toml"

[functions]
pack = "functions/reviewed.toml"
profiles = ["vendor"]

[functions.review]
output = "generated/function-review.md"
"#,
    )
    .unwrap();

    let project = ProjectSpec::load(&path).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert_eq!(project.id, "fixture");
    assert_eq!(project.target_spec, directory.join("target.spec"));
    assert_eq!(project.run_spec, Some(directory.join("local.run")));
    assert_eq!(project.memory_map, Some(directory.join("memory.toml")));
    assert!(project.svd_configured);
    assert_eq!(project.svd_paths, [directory.join("registers/base.svd")]);
    assert_eq!(
        project.symbol_inventory,
        Some(SymbolInventorySpec {
            output: directory.join("generated/symbols.json"),
        })
    );
    assert_eq!(
        project.navigation_index,
        Some(NavigationIndexSpec {
            output: directory.join("generated/navigation.json"),
        })
    );
    assert_eq!(
        project.ir_profiles,
        [ProjectIrProfile {
            id: "vendor".to_owned(),
            sources: vec!["rom".to_owned(), "archive".to_owned()],
            symbol_prefix: "phy_".to_owned(),
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: directory.join("generated/vendor.ir.json"),
            pseudo_rust: Some(directory.join("generated/vendor.pseudo.rs")),
        }]
    );
    assert_eq!(
        project.registers,
        Some(RegisterWorkspacePaths {
            facts: directory.join("generated/mmio.json"),
            model: directory.join("registers/reviewed.toml"),
            review_output: Some(directory.join("generated/register-review.md")),
            review_ir_reports: vec![directory.join("generated/vendor.ir.json")],
            svd_output: Some(directory.join("generated/device.svd")),
            pac: Some(PacOutputSpec {
                output: directory.join("generated/pac/src/lib.rs"),
                target: "none".to_owned(),
                edition: "2024".to_owned(),
            }),
            bindings: Some(PacBindingsOutputSpec {
                output: directory.join("generated/device.bindings"),
                crate_name: "fixture_pac".to_owned(),
            }),
            api_pack: Some(directory.join("registers/api.toml")),
            lint_pack: Some(directory.join("registers/lints.toml")),
            evidence_catalogs: vec![directory.join("registers/evidence.toml")],
        })
    );
    assert_eq!(
        project.interfaces,
        Some(InterfaceWorkspacePaths {
            facts: directory.join("generated/interfaces.json"),
            pack: Some(directory.join("interfaces/reviewed.toml")),
            semantic_catalogs: vec![],
        })
    );
    assert_eq!(
        project.functions,
        Some(FunctionWorkspacePaths {
            pack: directory.join("functions/reviewed.toml"),
            profiles: vec!["vendor".to_owned()],
            review_output: Some(directory.join("generated/function-review.md")),
        })
    );
    assert_eq!(
        project.function_ir_reports().unwrap(),
        [(
            "vendor".to_owned(),
            directory.join("generated/vendor.ir.json")
        )]
    );
}

#[test]
fn discovers_the_nearest_project_manifest_from_a_child_directory() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-workbench-project-discovery-{}",
        std::process::id()
    ));
    let child = directory.join("generated/findings");
    std::fs::create_dir_all(&child).unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(&manifest, "schema = 1\n").unwrap();

    assert_eq!(ProjectSpec::discover_from(&child).unwrap(), Some(manifest));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_removed_workspace_configuration_keys() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-workbench-project-removed-keys-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();

    let registers = directory.join("registers.toml");
    std::fs::write(
        &registers,
        "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.spec\"\n[registers]\nfacts = \"facts.json\"\noverlay = \"reviewed.toml\"\n",
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&registers)
            .unwrap_err()
            .to_string()
            .contains("unknown project registers key \"overlay\"")
    );

    let interfaces = directory.join("interfaces.toml");
    std::fs::write(
        &interfaces,
        "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.spec\"\n[interfaces]\nfacts = \"facts.json\"\nsemantic-catalogs = []\n",
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&interfaces)
            .unwrap_err()
            .to_string()
            .contains("semantic catalogs belong to the platform pack")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_generic_analysis_output_collisions() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-workbench-project-analysis-collision-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &manifest,
        r#"
schema = 1
id = "fixture"
target-spec = "target.spec"

[analysis.symbols]
output = "generated/facts.json"

[registers]
facts = "generated/facts.json"
model = "registers/device.toml"
"#,
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&manifest)
            .unwrap_err()
            .to_string()
            .contains("reuses another analysis facts path")
    );
    std::fs::write(
        &manifest,
        r#"
schema = 1
id = "fixture"
target-spec = "target.spec"

[analysis.symbols]
output = "generated/symbols.json"

[analysis.navigation]
output = "generated/facts.json"

[interfaces]
facts = "generated/facts.json"
"#,
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&manifest)
            .unwrap_err()
            .to_string()
            .contains("navigation index reuses another analysis facts path")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn distinguishes_an_explicit_empty_svd_catalog_from_an_omitted_one() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-workbench-project-empty-svd-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let explicit = directory.join("explicit.toml");
    let omitted = directory.join("omitted.toml");
    std::fs::write(
        &explicit,
        "schema = 1\nid = \"explicit\"\ntarget-spec = \"target.spec\"\nsvd = []\n",
    )
    .unwrap();
    std::fs::write(
        &omitted,
        "schema = 1\nid = \"omitted\"\ntarget-spec = \"target.spec\"\n",
    )
    .unwrap();

    let explicit = ProjectSpec::load(&explicit).unwrap();
    let omitted = ProjectSpec::load(&omitted).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(explicit.svd_configured);
    assert!(explicit.svd_paths.is_empty());
    assert!(!omitted.svd_configured);
    assert!(omitted.svd_paths.is_empty());
}
