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
        (
            "cli/commands/symbol_inventory.rs",
            "artifacts::SYMBOL_INVENTORY",
        ),
        (
            "cli/commands/discover_mmio_json.rs",
            "artifacts::MMIO_FACTS",
        ),
        (
            "cli/commands/interface_discovery_json.rs",
            "artifacts::INTERFACE_FACTS",
        ),
        (
            "cli/commands/export_ir/json_report.rs",
            "artifacts::LINKED_IR",
        ),
        ("registers/facts.rs", "artifacts::MMIO_FACTS"),
        ("interfaces/facts/parse.rs", "artifacts::INTERFACE_FACTS"),
        ("function_workspace/facts.rs", "artifacts::LINKED_IR"),
        ("registers/review_ir_parse.rs", "artifacts::LINKED_IR"),
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
    for removed in ["project_ir_report.rs", "symbol_inventory_report.rs"] {
        assert!(
            !root.join(removed).exists(),
            "legacy persistent-artifact reader still exists: {removed}"
        );
    }
}
