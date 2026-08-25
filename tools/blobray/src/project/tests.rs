use super::*;

fn invalid_project_span(input: &str, name: &str) -> (usize, usize, String) {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-diagnostic-{}-{name}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("vendor-project.toml");
    std::fs::write(&path, input).unwrap();
    let error = ProjectSpec::load(&path).unwrap_err();
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
    let message = error.to_string();
    let crate::Error::Project(ProjectError::Invalid { span, .. }) = error else {
        panic!("expected a source-aware project error, got {message}")
    };
    (span.offset(), span.len(), message)
}

#[test]
fn register_workspace_requires_an_explicit_nonempty_publication_scope() {
    let (_, _, missing) = invalid_project_span(
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[registers]\nfacts = \"facts.json\"\nmodel = \"registers.toml\"\n",
        "missing-owned-ranges.toml",
    );
    assert!(missing.contains("requires \"owned-ranges\""));

    let (_, _, duplicate) = invalid_project_span(
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[registers]\nfacts = \"facts.json\"\nmodel = \"registers.toml\"\nowned-ranges = [\"radio\", \"radio\"]\n",
        "duplicate-owned-ranges.toml",
    );
    assert!(duplicate.contains("duplicate project registers.owned-ranges"));
}

#[test]
fn review_scopes_require_explicit_canonical_protocols() {
    let (_, _, missing) = invalid_project_span(
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"vendor\"\nroots = \"all\"\noutput = \"vendor.ir\"\n[review]\noutput = \"review.json\"\npublication-scopes = [\"station-state\"]\n[[review.scopes]]\nid = \"station-state\"\nprofiles = [\"vendor\"]\nroots = [\"vendor:root\"]\n",
        "missing-review-protocols.toml",
    );
    assert!(missing.contains("requires \"protocols\""), "{missing}");

    for (name, protocol) in [
        ("bt-alias.toml", "bt"),
        ("dot-802154-alias.toml", "802.15.4"),
        ("unknown-radio.toml", "radio"),
    ] {
        let input = format!(
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"vendor\"\nroots = \"all\"\noutput = \"vendor.ir\"\n[review]\noutput = \"review.json\"\npublication-scopes = [\"feature\"]\n[[review.scopes]]\nid = \"feature\"\nprotocols = [\"{protocol}\"]\nprofiles = [\"vendor\"]\nroots = [\"vendor:root\"]\n"
        );
        let (_, _, error) = invalid_project_span(&input, name);
        assert!(
            error.contains("unsupported protocol"),
            "case {name}: {error}"
        );
    }
}

#[test]
fn reviewed_knowledge_requires_one_explicit_configured_default_pack() {
    let cases = [
        (
            "missing-default-pack.toml",
            "[reviewed-knowledge]\npacks = [\"reviewed/project.toml\"]\n",
            "requires default-pack when packs are configured",
        ),
        (
            "unconfigured-default-pack.toml",
            "[reviewed-knowledge]\npacks = [\"reviewed/one.toml\"]\ndefault-pack = \"reviewed/two.toml\"\n",
            "must exactly match one configured packs entry",
        ),
        (
            "default-without-packs.toml",
            "[reviewed-knowledge]\ndefault-pack = \"reviewed/project.toml\"\n",
            "is forbidden without non-empty packs",
        ),
        (
            "default-with-empty-packs.toml",
            "[reviewed-knowledge]\npacks = []\ndefault-pack = \"reviewed/project.toml\"\n",
            "is forbidden without non-empty packs",
        ),
    ];
    for (name, section, expected) in cases {
        let input =
            format!("schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n{section}");
        let (_, _, error) = invalid_project_span(&input, name);
        assert!(error.contains(expected), "case {name}: {error}");
    }
}

#[test]
fn project_manifest_rejects_misspelled_nested_configuration_tables() {
    let (_, _, error) = invalid_project_span(
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[registers]\nfacts = \"facts.json\"\nmodel = \"registers.toml\"\nowned-ranges = [\"radio\"]\n[registers.toml]\ncatalogs = [\"evidence.toml\"]\n",
        "unknown-register-table.toml",
    );
    assert!(error.contains("unknown project registers key \"toml\""));
}

#[test]
fn register_workspace_inherits_the_reusable_chip_model() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-chip-model-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"fixture-chip\"\nregister-model = \"registers/device.toml\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join(DEFAULT_PROJECT_MANIFEST),
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n\n[registers]\nfacts = \"generated/mmio.json\"\nowned-ranges = [\"radio\"]\n",
    )
    .unwrap();

    let project = ProjectSpec::load(&directory.join(DEFAULT_PROJECT_MANIFEST)).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();

    assert_eq!(
        project.registers.unwrap().model,
        directory.join("registers/device.toml")
    );
}

#[test]
fn project_composition_selects_one_reviewed_chip_revision_fail_closed() {
    const ARTIFACT_SHA256: &str = concat!(
        "0123456789abcdef0123456789abcdef",
        "0123456789abcdef0123456789abcdef"
    );
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-review-context-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join("reviewed")).unwrap();
    std::fs::write(
        directory.join("ecosystem.toml"),
        "schema = 3\nid = \"fixture-ecosystem\"\nknowledge-packs = []\ncapability-packs = []\ninterface-template-packs = []\n[applicability]\necosystems = [\"esp-idf\"]\n",
    )
    .unwrap();
    let write_chip = |revisions: &str| {
        std::fs::write(
            directory.join("chip.toml"),
            format!(
                "schema = 3\nid = \"fixture-chip\"\nregister-model = \"registers/device.toml\"\n[applicability]\nchips = [\"esp32s31\"]\nchip-revisions = {revisions}\n"
            ),
        )
        .unwrap();
    };
    write_chip("[\"rev0\"]");
    let make_pack = |id: &str, revision: &str, name: &str| {
        std::fs::write(
            directory.join(format!("reviewed/{id}.toml")),
            format!(
                r#"schema = 1
id = "{id}"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[applies-to]
ecosystems = ["esp-idf"]
chips = ["esp32s31"]
chip-revisions = ["{revision}"]
artifact-lineages = ["fixture-radio"]
artifacts = [{{ source = "fixture-blob", sha256 = "{ARTIFACT_SHA256}" }}]
[[assertions]]
id = "{id}.identity"
subject = "mmio:cpu:0x1000/32"
kind = "register-identity"
value = "{name}"
[[assertions.evidence]]
source = "FIXTURE"
locator = "manual"
"#
            ),
        )
        .unwrap();
    };
    make_pack("rev0", "rev0", "RADIO.CONTROL_REV0");
    make_pack("rev1", "rev1", "RADIO.CONTROL_REV1");
    std::fs::write(
        directory.join(DEFAULT_PROJECT_MANIFEST),
        format!(
            r#"schema = 3
id = "fixture"
target-spec = "target.toml"
ecosystem-packs = ["ecosystem.toml"]
chip-pack = "chip.toml"
[applicability]
artifact-lineages = ["fixture-radio"]
artifacts = [{{ source = "fixture-blob", sha256 = "{ARTIFACT_SHA256}" }}]
[reviewed-knowledge]
packs = ["reviewed/rev0.toml", "reviewed/rev1.toml"]
default-pack = "reviewed/rev0.toml"
[registers]
facts = "generated/mmio.json"
owned-ranges = ["radio"]
"#
        ),
    )
    .unwrap();

    let project = ProjectSpec::load(&directory.join(DEFAULT_PROJECT_MANIFEST)).unwrap();
    let context = &project.registers.unwrap().review_context;
    assert_eq!(context.ecosystems, ["esp-idf"]);
    assert_eq!(context.chips, ["esp32s31"]);
    assert_eq!(context.chip_revisions, ["rev0"]);
    assert_eq!(context.artifact_lineages, ["fixture-radio"]);
    assert_eq!(context.artifacts.len(), 1);
    assert_eq!(context.artifacts[0].source, "fixture-blob");
    assert_eq!(context.artifacts[0].sha256, ARTIFACT_SHA256);

    write_chip("[\"rev0\", \"rev1\"]");
    let ambiguous = ProjectSpec::load(&directory.join(DEFAULT_PROJECT_MANIFEST)).unwrap_err();
    assert!(ambiguous.to_string().contains("context is ambiguous"));

    write_chip("[]");
    let missing = ProjectSpec::load(&directory.join(DEFAULT_PROJECT_MANIFEST)).unwrap_err();
    assert!(missing.to_string().contains("chip-revisions"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn resolves_composed_specs_relative_to_the_project() {
    let directory =
        std::env::temp_dir().join(format!("open-radio-blobray-project-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &path,
        r#"
schema = 3
id = "fixture"
target-spec = "target.toml"
verification-addon = "verification.toml"
run-spec = "local.toml"
chip-pack = "chip.toml"

[reviewed-knowledge]
packs = ["reviewed/radio.toml"]
default-pack = "reviewed/radio.toml"

[analysis.symbols]
output = "generated/symbols.json"

[analysis.navigation]
output = "generated/navigation.json"

[code]
pack = "code/boundaries.toml"

[code.review]
output = "generated/code-boundaries.md"

[[analysis.ir]]
id = "vendor"
sources = ["rom", "archive"]
roots = "symbol-prefix"
symbol-prefix = "phy_"
include-reachable = true
output = "generated/vendor.ir"

[registers]
facts = "generated/mmio.json"
model = "registers/reviewed.toml"
owned-ranges = ["radio"]

[registers.review]
output = "generated/register-review.md"
linked-ir = ["generated/vendor.ir"]
non-operational-functions = ["archive:register_dump"]

[registers.svd]
output = "generated/device.svd"

[registers.pac-raw]
output = "generated/pac-raw/src/lib.rs"
target = "none"
edition = "2024"

[registers.bindings]
output = "generated/device.bindings.toml"
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

[review]
output = "generated/review-scopes.json"
publication-scopes = ["radio-init"]

[[review.scopes]]
id = "radio-init"
protocols = ["shared"]
profiles = ["vendor"]
roots = ["archive:phy_init"]
include-reachable = true

"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        r#"schema = 3
id = "fixture-chip"
memory-map = "memory.toml"
svd = ["registers/base.svd"]
knowledge-provider = "none"
knowledge-packs = []
"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("verification.toml"),
        r#"schema = 3
id = "fixture-verification"
report = "generated/verification.json"
evidence-index = "generated/vendor-evidence.json"

[[suites]]
id = "radio"
auxiliary-sources = ["linked-replay"]
rust-artifact-role = "rust-artifact:radio"
rust-companion-role = "rust-companion:radio"
rust-prefix = "open_trace_"
profiles = ["profiles/compiled.toml", "profiles/interrupts.toml"]
dispositions = ["dispositions/radio.toml"]
baselines = ["baselines/radio.toml"]
gate = "regression"
match-floor = 2
[[suites.vendor]]
source = "rom"
prefix = "phy_"
[[suites.vendor]]
source = "archive"
all = true
"#,
    )
    .unwrap();
    std::fs::create_dir_all(directory.join("reviewed")).unwrap();
    std::fs::write(
        directory.join("reviewed/radio.toml"),
        r#"schema = 1
id = "fixture-radio"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[[assertions]]
id = "radio.control.identity"
subject = "mmio:cpu:0x1000/32"
kind = "register-identity"
value = "RADIO.CONTROL"
[[assertions.evidence]]
source = "FIXTURE"
locator = "review"
"#,
    )
    .unwrap();

    let project = ProjectSpec::load(&path).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert_eq!(project.id, "fixture");
    assert_eq!(project.target_spec, directory.join("target.toml"));
    assert_eq!(project.run_spec, Some(directory.join("local.toml")));
    assert_eq!(project.memory_map, Some(directory.join("memory.toml")));
    assert_eq!(project.svd_paths, [directory.join("registers/base.svd")]);
    assert_eq!(
        project.reviewed_knowledge,
        [directory.join("reviewed/radio.toml")]
    );
    assert_eq!(
        project.reviewed_knowledge_default,
        Some(directory.join("reviewed/radio.toml"))
    );
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
        project.code,
        Some(CodeWorkspacePaths {
            pack: directory.join("code/boundaries.toml"),
            review_output: Some(directory.join("generated/code-boundaries.md")),
        })
    );
    assert_eq!(
        project.ir_profiles,
        [ProjectIrProfile {
            id: "vendor".to_owned(),
            sources: vec!["rom".to_owned(), "archive".to_owned()],
            roots: crate::project_ir::ProjectIrRoots::SymbolPrefix("phy_".to_owned()),
            include_reachable: true,
            entry_contract: "none".to_owned(),
            output: directory.join("generated/vendor.ir"),
        }]
    );
    assert_eq!(
        project.registers,
        Some(RegisterWorkspacePaths {
            facts: directory.join("generated/mmio.json"),
            model: directory.join("registers/reviewed.toml"),
            owned_ranges: vec!["radio".to_owned()],
            non_operational_functions: vec!["archive:register_dump".to_owned()],
            review_output: Some(directory.join("generated/register-review.md")),
            review_ir_reports: vec![directory.join("generated/vendor.ir")],
            svd_output: Some(directory.join("generated/device.svd")),
            pac_raw: Some(PacRawOutputSpec {
                output: directory.join("generated/pac-raw/src/lib.rs"),
                target: "none".to_owned(),
                edition: "2024".to_owned(),
            }),
            bindings: Some(PacBindingsOutputSpec {
                output: directory.join("generated/device.bindings.toml"),
                crate_name: "fixture_pac".to_owned(),
            }),
            api_pack: Some(directory.join("registers/api.toml")),
            api_output: None,
            lint_pack: Some(directory.join("registers/lints.toml")),
            evidence_catalogs: vec![directory.join("registers/evidence.toml")],
            reviewed_knowledge: vec![directory.join("reviewed/radio.toml")],
            review_context: open_radio_vendor_review::ApplicabilityContext::default(),
        })
    );
    assert_eq!(
        project.interfaces,
        Some(InterfaceWorkspacePaths {
            facts: directory.join("generated/interfaces.json"),
            pack: Some(directory.join("interfaces/reviewed.toml")),
            semantic_catalogs: vec![],
            capability_packs: vec![],
            interface_template_packs: vec![],
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
        [("vendor".to_owned(), directory.join("generated/vendor.ir"))]
    );
    assert_eq!(
        project.review,
        Some(ReviewWorkspaceSpec {
            output: directory.join("generated/review-scopes.json"),
            publication_scopes: vec!["radio-init".to_owned()],
            scopes: vec![ReviewScopeSpec {
                id: "radio-init".to_owned(),
                protocols: vec!["shared".to_owned()],
                profiles: vec!["vendor".to_owned()],
                roots: vec!["archive:phy_init".to_owned()],
                include_reachable: true,
            }],
        })
    );
    assert_eq!(
        project.verification,
        Some(VerificationWorkspacePaths {
            report: directory.join("generated/verification.json"),
            evidence_index: directory.join("generated/vendor-evidence.json"),
            policy: None,
            suites: vec![VerificationSuiteSpec {
                id: "radio".to_owned(),
                vendor: vec![
                    VerificationVendorSpec {
                        source: "rom".parse().unwrap(),
                        selection: VerificationVendorSelection::Prefix("phy_".to_owned()),
                    },
                    VerificationVendorSpec {
                        source: "archive".parse().unwrap(),
                        selection: VerificationVendorSelection::All,
                    },
                ],
                auxiliary_sources: vec!["linked-replay".parse().unwrap()],
                rust_artifact_role: InputRole::parse("rust-artifact:radio").unwrap(),
                rust_companion_role: Some(InputRole::parse("rust-companion:radio").unwrap()),
                rust_prefix: "open_trace_".to_owned(),
                profiles: vec![
                    directory.join("profiles/compiled.toml"),
                    directory.join("profiles/interrupts.toml"),
                ],
                dispositions: vec![directory.join("dispositions/radio.toml")],
                evidence_baselines: vec![directory.join("baselines/radio.toml")],
                gate: ProjectVerificationGate::Regression { match_floor: 2 },
            }],
        })
    );
}

#[test]
fn nested_project_errors_retain_the_exact_manifest_value_span() {
    let cases = [
        (
            "wrong-run-spec-type.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nrun-spec = 7\n",
            "7",
            "run-spec",
        ),
        (
            "wrong-analysis-shape.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nanalysis = \"wrong\"\n",
            "\"wrong\"",
            "analysis must be a table",
        ),
        (
            "wrong-source-id.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"fixture\"\nsources = [\"bad.source\"]\noutput = \"generated/fixture.json\"\n",
            "\"bad.source\"",
            "invalid source id",
        ),
        (
            "wrong-pac-edition.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[registers]\nfacts = \"facts.json\"\nmodel = \"registers.toml\"\nowned-ranges = [\"radio\"]\n[registers.pac-raw]\noutput = \"pac.rs\"\nedition = \"2018\"\n",
            "\"2018\"",
            "edition must be",
        ),
        (
            "removed-pac-key.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[registers]\nfacts = \"facts.json\"\nmodel = \"registers.toml\"\nowned-ranges = [\"radio\"]\n[registers.pac]\noutput = \"pac.rs\"\n",
            "[registers.pac]",
            "[registers.pac-raw]",
        ),
        (
            "removed-interface-key.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[interfaces]\nfacts = \"interfaces.json\"\nsemantic-catalogs = []\n",
            "[]",
            "knowledge packs belong to ecosystem or chip packs",
        ),
        (
            "unknown-function-profile.toml",
            "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[[analysis.ir]]\nid = \"known\"\nroots = \"all\"\noutput = \"known.json\"\n[functions]\npack = \"functions.toml\"\nprofiles = [\"missing\"]\n",
            "[\"missing\"]",
            "unknown IR profile",
        ),
    ];

    for (name, input, needle, expected_message) in cases {
        let (offset, length, message) = invalid_project_span(input, name);
        assert_eq!(
            offset,
            input.find(needle).unwrap(),
            "case {name}: {message}"
        );
        assert_eq!(length, needle.len(), "case {name}: {message}");
        assert!(message.contains(expected_message), "case {name}: {message}");
    }
}

#[test]
fn discovers_the_nearest_project_manifest_from_a_child_directory() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-discovery-{}",
        std::process::id()
    ));
    let child = directory.join("generated/findings");
    std::fs::create_dir_all(&child).unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(&manifest, "schema = 3\n").unwrap();

    assert_eq!(ProjectSpec::discover_from(&child).unwrap(), Some(manifest));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_removed_workspace_configuration_keys() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-removed-keys-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();

    let registers = directory.join("registers.toml");
    std::fs::write(
        &registers,
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[registers]\nfacts = \"facts.json\"\noverlay = \"reviewed.toml\"\n",
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
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[interfaces]\nfacts = \"facts.json\"\nsemantic-catalogs = []\n",
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&interfaces)
            .unwrap_err()
            .to_string()
            .contains("knowledge packs belong to ecosystem or chip packs")
    );

    let legacy_verification = directory.join("legacy-verification.toml");
    std::fs::write(
        &legacy_verification,
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\n[verification]\nprofiles = [\"profiles.toml\"]\n",
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&legacy_verification)
            .unwrap_err()
            .to_string()
            .contains("unknown project manifest key \"verification\"")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn completion_suite_may_start_without_an_evidence_baseline() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-completion-baseline-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &manifest,
        r#"
schema = 3
id = "fixture"
target-spec = "target.toml"
verification-addon = "verification.toml"
"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("verification.toml"),
        r#"schema = 3
id = "fixture-verification"
report = "generated/verification.json"
evidence-index = "generated/vendor-evidence.json"

[[suites]]
id = "uncovered-leaf"
rust-artifact-role = "rust-artifact"
rust-prefix = "open_trace_"
profiles = ["profile.toml"]
dispositions = ["dispositions.toml"]
baselines = []
gate = "completion"

[[suites.vendor]]
source = "vendor"
symbols = ["uncovered_leaf"]
"#,
    )
    .unwrap();

    let project = ProjectSpec::load(&manifest).unwrap();
    let suite = &project.verification.unwrap().suites[0];
    assert!(suite.evidence_baselines.is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_generic_analysis_output_collisions() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-analysis-collision-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &manifest,
        r#"
schema = 3
id = "fixture"
target-spec = "target.toml"

[analysis.symbols]
output = "generated/facts.json"

[registers]
facts = "generated/facts.json"
model = "registers/device.toml"
owned-ranges = ["radio"]
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
schema = 3
id = "fixture"
target-spec = "target.toml"

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
fn reviewed_code_pack_and_review_own_distinct_paths() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-code-path-collision-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &manifest,
        r#"
schema = 3
id = "fixture"
target-spec = "target.toml"

[analysis.symbols]
output = "generated/symbols.json"

[code]
pack = "generated/symbols.json"
"#,
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&manifest)
            .unwrap_err()
            .to_string()
            .contains("reuses symbol inventory output path")
    );
    std::fs::write(
        &manifest,
        r#"
schema = 3
id = "fixture"
target-spec = "target.toml"

[analysis.symbols]
output = "generated/symbols.json"

[code]
pack = "code/boundaries.toml"

[code.review]
output = "code/boundaries.toml"
"#,
    )
    .unwrap();
    assert!(
        ProjectSpec::load(&manifest)
            .unwrap_err()
            .to_string()
            .contains("review reuses the reviewed pack path")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn accepts_an_explicit_or_omitted_empty_chip_svd_catalog() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-empty-svd-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let explicit = directory.join("explicit.toml");
    let omitted = directory.join("omitted.toml");
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"fixture-chip\"\nsvd = []\nknowledge-provider = \"none\"\nknowledge-packs = []\n",
    )
    .unwrap();
    std::fs::write(
        &explicit,
        "schema = 3\nid = \"explicit\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n",
    )
    .unwrap();
    std::fs::write(
        &omitted,
        "schema = 3\nid = \"omitted\"\ntarget-spec = \"target.toml\"\n",
    )
    .unwrap();

    let explicit = ProjectSpec::load(&explicit).unwrap();
    let omitted = ProjectSpec::load(&omitted).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(explicit.svd_paths.is_empty());
    assert!(omitted.svd_paths.is_empty());
}

#[test]
fn one_chip_pack_is_reused_by_multiple_project_compositions() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-shared-chip-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"shared-chip\"\nmemory-map = \"memory.toml\"\nsvd = [\"chip.svd\"]\n",
    )
    .unwrap();
    for id in ["first", "second"] {
        std::fs::write(
            directory.join(format!("{id}.toml")),
            format!(
                "schema = 3\nid = \"{id}\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\n"
            ),
        )
        .unwrap();
    }

    let first = ProjectSpec::load(&directory.join("first.toml")).unwrap();
    let second = ProjectSpec::load(&directory.join("second.toml")).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();

    assert_eq!(first.memory_map, second.memory_map);
    assert_eq!(first.svd_paths, second.svd_paths);
    assert_eq!(first.chip_pack.unwrap().id, "shared-chip");
    assert_eq!(second.chip_pack.unwrap().id, "shared-chip");
}

#[test]
fn project_local_analysis_provider_is_explicit_and_conflicts_fail_closed() {
    let directory = std::env::temp_dir().join(format!(
        "open-radio-blobray-project-provider-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"shared-chip\"\n",
    )
    .unwrap();
    let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
    std::fs::write(
        &manifest,
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nchip-pack = \"chip.toml\"\nanalysis-provider = \"investigation-v1\"\n",
    )
    .unwrap();
    let project = ProjectSpec::load(&manifest).unwrap();
    let mut target = crate::TargetSpec {
        id: "fixture-target".to_owned(),
        knowledge_provider: None,
        architecture: crate::target::Architecture::Riscv32,
        calling_convention: crate::target::CallingConvention::RiscvIlp32,
        endianness: crate::target::Endianness::Little,
        pointer_width: 32,
        rust_target: "riscv32imc-unknown-none-elf".to_owned(),
    };
    project.apply_to_target(&mut target).unwrap();
    assert_eq!(
        target.knowledge_provider.as_deref(),
        Some("investigation-v1")
    );

    std::fs::write(
        directory.join("chip.toml"),
        "schema = 3\nid = \"shared-chip\"\nknowledge-provider = \"chip-v1\"\n",
    )
    .unwrap();
    let conflicting = ProjectSpec::load(&manifest).unwrap();
    target.knowledge_provider = None;
    let error = conflicting.apply_to_target(&mut target).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot compose analysis-provider")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_analysis_provider_must_be_one_token() {
    let (_, _, error) = invalid_project_span(
        "schema = 3\nid = \"fixture\"\ntarget-spec = \"target.toml\"\nanalysis-provider = \"two tokens\"\n",
        "invalid-analysis-provider.toml",
    );
    assert!(error.contains("analysis-provider must be one non-empty token"));
}
