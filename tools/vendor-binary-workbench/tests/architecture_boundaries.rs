//! Source-level guards for frontend/domain dependency direction.

use std::{fs, path::Path};

fn rust_files(root: &Path, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            rust_files(&path, output);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            output.push(path);
        }
    }
}

#[test]
fn domain_and_application_do_not_depend_on_cli_rendering() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let violations = files
        .into_iter()
        .filter(|path| !path.starts_with(root.join("cli")))
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read Rust source");
            (source.contains("outputln!")
                || source.contains("crate::cli")
                || source.contains("cli::commands"))
            .then_some(path)
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "non-CLI modules depend on CLI output or commands: {violations:#?}"
    );
}

#[test]
fn cli_command_adapters_do_not_invoke_each_other() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/commands");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let violations = files
        .into_iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("mod.rs"))
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("tests.rs"))
        .filter(|path| !path.components().any(|part| part.as_os_str() == "tests"))
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read CLI command adapter");
            source
                .lines()
                .any(|line| line.contains("super::") && line.contains("::run("))
                .then_some(path)
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "CLI command adapters invoke sibling commands: {violations:#?}"
    );
    let output =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/output.rs"))
            .expect("read CLI output boundary");
    assert!(
        !output.contains("fn suppress") && !output.contains("SUPPRESSION_DEPTH"),
        "CLI output suppression restored nested command composition"
    );
    let pipeline = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/commands/project_pipeline.rs"),
    )
    .expect("read project analysis adapter");
    assert!(
        !pipeline.contains("impl ProjectAnalysisOperations")
            && pipeline.contains("project_analysis::analyze_project"),
        "project analysis dependency wiring escaped the application service"
    );
}

#[test]
fn application_status_has_one_typed_model() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        !source_root.join("application/snapshot/status.rs").exists(),
        "snapshot status projection must not duplicate ProjectStatusReport"
    );
    let snapshot = fs::read_to_string(source_root.join("application/snapshot.rs"))
        .expect("read application snapshot");
    assert!(
        !snapshot.contains("serde_json::to_value"),
        "workspace snapshots must embed the typed status report directly"
    );
    let model = fs::read_to_string(source_root.join("application/model.rs"))
        .expect("read application model");
    assert!(!model.contains("ProjectStatusSnapshot"));
    assert!(!model.contains("WorkspaceReadiness"));
}

#[test]
fn generic_cli_has_no_esp_phy_prefix_defaults() {
    let arguments =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/arguments.rs"))
            .expect("read typed CLI arguments");
    for forbidden in [
        "default_value = \"phy_\"",
        "default_value = \"open_phy_trace_\"",
    ] {
        assert!(
            !arguments.contains(forbidden),
            "generic CLI contains target-specific default {forbidden}"
        );
    }
}

#[test]
fn generic_reference_generation_has_no_target_product_vocabulary() {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [
        manifest_root.join("crates/backend-riscv/src"),
        manifest_root.join("src/orchestration/generated_reference.rs"),
    ];
    let mut violations = Vec::new();
    for root in roots {
        let files = if root.is_dir() {
            let mut files = Vec::new();
            rust_files(&root, &mut files);
            files
        } else {
            vec![root]
        };
        for path in files {
            let source = fs::read_to_string(&path).expect("read generic reference source");
            for forbidden in ["open_phy", "esp32s31", "open_esp_radio"] {
                if source.contains(forbidden) {
                    violations.push((path.clone(), forbidden));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "generic reference generation contains target/product vocabulary: {violations:#?}"
    );
}

#[test]
fn execution_environment_contracts_do_not_live_in_the_riscv_backend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/backend-riscv/src/execution");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let source = files
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("read backend execution source"))
        .collect::<String>();
    for forbidden in [
        "pub enum DeviceModelSpec",
        "pub struct DeviceModelDescriptor",
        "pub struct TableInstance",
        "pub enum TableLifecycleEvent",
        "mod device;",
    ] {
        assert!(
            !source.contains(forbidden),
            "RISC-V backend still owns architecture-neutral contract {forbidden}"
        );
    }
}

#[test]
fn platform_harness_dependencies_are_optional_addons() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("read workbench manifest");
    assert!(
        manifest.contains("default = []"),
        "generic workbench must not compile a chip harness by default"
    );
    for dependency in [
        "open-radio-vendor-harness-esp32s31",
        "open-radio-vendor-harness-esp32s31-semantic",
    ] {
        let line = manifest
            .lines()
            .find(|line| line.starts_with(dependency))
            .unwrap_or_else(|| panic!("missing optional dependency {dependency}"));
        assert!(
            line.contains("optional = true"),
            "compiled addon dependency is unconditional: {line}"
        );
    }
}

#[test]
fn register_catalog_uses_the_typed_model_without_an_xml_round_trip() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/register_catalog.rs"))
            .expect("read register catalog adapter");
    let production = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in ["render_svd()", "MmioMap::parse("] {
        assert!(
            !production.contains(forbidden),
            "register catalog still contains model-to-SVD round trip {forbidden}"
        );
    }
}

#[test]
fn cli_resolution_has_one_typed_command_axis() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli");
    let arguments = fs::read_to_string(root.join("args.rs")).expect("read CLI grammar");
    let resolver = fs::read_to_string(root.join("resolver.rs")).expect("read CLI resolver");
    let dispatch = fs::read_to_string(root.join("dispatch.rs")).expect("read CLI dispatch");

    assert!(
        !arguments.contains("CommandArguments"),
        "CLI restored the parallel command/argument representation"
    );
    for obsolete_policy in [
        "requires_backend",
        "requires_harness",
        "requires_mmio_map",
        "uses_memory_map",
        "uses_register_catalog",
        "uses_run_spec",
    ] {
        assert!(
            !arguments.contains(obsolete_policy) && !resolver.contains(obsolete_policy),
            "CLI restored independent capability policy `{obsolete_policy}`"
        );
    }
    for impossible_branch in ["unreachable!", ".expect("] {
        assert!(
            !dispatch.contains(impossible_branch),
            "resolved dispatch contains an impossible command/argument branch: {impossible_branch}"
        );
    }
}

#[test]
fn persistent_artifact_identities_have_one_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let required_uses = [
        ("artifacts/symbol_inventory.rs", "SYMBOL_INVENTORY"),
        ("artifacts/mmio_facts.rs", "MMIO_FACTS"),
        ("artifacts/mmio_facts_read.rs", "MMIO_FACTS"),
        ("artifacts/interface_facts.rs", "INTERFACE_FACTS"),
        ("artifacts/interface_facts_read.rs", "INTERFACE_FACTS"),
        ("artifacts/linked_ir_document.rs", "artifacts::LINKED_IR"),
        ("artifacts/linked_ir_read.rs", "LINKED_IR"),
    ];
    for (relative, identity) in required_uses {
        let source =
            fs::read_to_string(root.join(relative)).expect("read artifact boundary source");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(
            production.contains(identity),
            "persistent artifact boundary {relative} does not use canonical {identity}"
        );
    }
    for relative in [
        "registers/facts.rs",
        "registers/review_ir_parse.rs",
        "interfaces/facts/parse.rs",
        "function_workspace/facts/parse.rs",
    ] {
        let source = fs::read_to_string(root.join(relative)).expect("read artifact consumer");
        assert!(
            !source.contains("serde_json::Value") && !source.contains("Map<String, Value>"),
            "artifact consumer {relative} restored a handwritten JSON tree reader"
        );
    }
    for removed in [
        "project_ir_report.rs",
        "symbol_inventory_report.rs",
        "navigation/reports.rs",
    ] {
        assert!(
            !root.join(removed).exists(),
            "legacy persistent-artifact reader still exists: {removed}"
        );
    }
}

#[test]
fn large_analysis_and_human_renderers_keep_functional_boundaries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for required in [
        "analysis/linked_ir/register_index/build.rs",
        "analysis/linked_ir/register_index/evidence.rs",
        "analysis/linked_ir/summary/effect.rs",
        "analysis/linked_ir/summary/event_dispatch.rs",
        "analysis/linked_ir/summary/projection.rs",
        "function_workspace/pack_validate/contexts.rs",
        "function_workspace/pack_validate/primitives.rs",
        "function_workspace/pack_validate/types.rs",
        "cli/commands/export_ir/human/functions/effects.rs",
        "cli/commands/export_ir/human/functions/local.rs",
    ] {
        assert!(
            root.join(required).is_file(),
            "missing module boundary {required}"
        );
    }

    let human_root = root.join("cli/commands/export_ir/human");
    let mut renderer_files = vec![
        root.join("cli/commands/export_ir/human.rs"),
        root.join("cli/render.rs"),
    ];
    rust_files(&human_root, &mut renderer_files);
    rust_files(&root.join("cli/render"), &mut renderer_files);
    let violations = renderer_files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read human renderer");
            source.contains("outputln!").then_some(path)
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "shared or linked-IR section renderers write through the global line macro: {violations:#?}"
    );
}
