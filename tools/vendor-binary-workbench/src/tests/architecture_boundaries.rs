use super::*;

fn rust_sources(root: &Path) -> Vec<std::path::PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut sources = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    sources
}

fn longest_hex_run(text: &str) -> usize {
    text.bytes()
        .fold((0, 0), |(current, longest), byte| {
            let current = if byte.is_ascii_hexdigit() {
                current + 1
            } else {
                0
            };
            (current, longest.max(current))
        })
        .1
}

fn contains_bare_macro_call(source: &str, name: &str) -> bool {
    source.match_indices(name).any(|(index, _)| {
        index == 0
            || !source.as_bytes()[index - 1].is_ascii_alphanumeric()
                && source.as_bytes()[index - 1] != b'_'
    })
}

#[test]
fn production_modules_cannot_bypass_the_command_output_boundary() {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_roots = [
        manifest_root.join("src"),
        manifest_root.join("crates/harness-esp32s31-semantic/src"),
    ];
    for path in source_roots.iter().flat_map(|root| rust_sources(root)) {
        if path.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some("tests" | "oracle_tests")
            )
        }) {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for forbidden in ["println!(", "print!("] {
            assert!(
                !contains_bare_macro_call(&source, forbidden),
                "direct stdout macro {forbidden} bypasses cli/output.rs in {}",
                path.display()
            );
        }
    }
}

#[test]
fn cargo_alias_keeps_workbench_preparation_visible() {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = fs::read_to_string(manifest_root.join("../../.cargo/config.toml")).unwrap();
    let alias = config
        .lines()
        .find(|line| line.trim_start().starts_with("vendor-binary-workbench ="))
        .expect("workspace must provide the documented Workbench Cargo alias");
    assert!(alias.contains("run --profile workbench"));
    assert!(
        !alias.contains("run --quiet"),
        "Cargo preparation and build-lock messages must remain visible"
    );
}

#[test]
fn facade_and_register_model_errors_cannot_implicitly_absorb_strings() {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = [
        manifest_root.join("src/error.rs"),
        manifest_root.join("../register-model/src/lib.rs"),
    ];
    for forbidden in [
        "impl From<String> for WorkbenchError",
        "impl From<&str> for WorkbenchError",
        "impl From<String> for Error",
        "impl From<&str> for Error",
        "Message(#[from] String)",
    ] {
        for path in &sources {
            let source = fs::read_to_string(path).unwrap();
            assert!(
                !source.contains(forbidden),
                "implicit string error conversion {forbidden} survived in {}",
                path.display()
            );
        }
    }
}

#[test]
fn workbench_source_contains_no_private_paths_or_embedded_digests() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let private_path_marker = ["_oracles", "/"].concat();
    for path in rust_sources(source_root) {
        let source = fs::read_to_string(&path).unwrap();
        assert!(
            !source.contains(&private_path_marker),
            "private artifact path leaked into {}",
            path.display()
        );
        assert!(
            longest_hex_run(&source) < 40,
            "embedded digest-like hex literal leaked into {}",
            path.display()
        );
    }
}

#[test]
fn executable_contract_api_uses_verification_vocabulary() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let forbidden_names = [
        ["DriverAdapter", "Qualification"].concat(),
        ["Qualify", "Contract"].concat(),
        ["qualify_", "driver_adapter"].concat(),
        ["qualify_", "semantic_contract"].concat(),
        ["qualify_", "named_contract"].concat(),
        ["qualify_", "esp32s31_"].concat(),
        ["QUALIFICATION", "-CASE"].concat(),
        ["QUALIFICATION", "-SUMMARY"].concat(),
    ];
    for path in rust_sources(source_root) {
        let source = fs::read_to_string(&path).unwrap();
        for forbidden in &forbidden_names {
            assert!(
                !source.contains(forbidden),
                "executable qualification name {forbidden} survived in {}",
                path.display()
            );
        }
    }
}

#[test]
fn neutral_contracts_have_no_architecture_or_platform_identity() {
    let contracts = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/contracts/src");
    for path in rust_sources(&contracts) {
        let source = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        for forbidden in ["esp32", "riscv", "xtensa", "cortex", "thumb"] {
            assert!(
                !source.contains(forbidden),
                "{forbidden} identity leaked into neutral contracts source {}",
                path.display()
            );
        }
    }
    let manifest = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/contracts/Cargo.toml"),
    )
    .unwrap();
    assert!(
        !manifest.contains("[dependencies]"),
        "neutral contracts must remain dependency-free"
    );
}

#[test]
fn riscv_backend_contains_no_platform_identity() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/backend-riscv/src");
    for path in rust_sources(&backend) {
        let source = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        assert!(
            !source.contains("esp32s31") && !source.contains("esp32-s31"),
            "platform identity leaked into RISC-V backend source {}",
            path.display()
        );
    }

    let manifest = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/backend-riscv/Cargo.toml"),
    )
    .unwrap();
    for forbidden in ["harness-esp32s31", "open-esp-radio-esp32s31", "../.."] {
        assert!(
            !manifest.contains(forbidden),
            "RISC-V backend manifest contains forbidden dependency {forbidden}"
        );
    }
}

#[test]
fn shared_model_contains_no_calling_convention_identity() {
    let shared_model = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/analysis-model/src");
    for path in rust_sources(&shared_model) {
        let source = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        for forbidden in ["rv32", "riscv", "aapcs", "xtensa", "exit_a0"] {
            assert!(
                !source.contains(forbidden),
                "{forbidden} calling-convention identity leaked into shared model {}",
                path.display()
            );
        }
        assert!(
            !source.contains("[symbolicvalue; 8]"),
            "fixed register-argument width leaked into shared model {}",
            path.display()
        );
    }
}

#[test]
fn facade_does_not_compile_backend_implementation_dependencies_directly() {
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for forbidden in [
        "object =",
        "rv-asm =",
        "roxmltree =",
        "open-esp-radio-esp32s31-phy =",
    ] {
        assert!(
            !manifest
                .lines()
                .any(|line| line.trim_start().starts_with(forbidden)),
            "workbench facade directly owns backend/model dependency {forbidden}"
        );
    }
}

#[test]
fn semantic_interface_has_no_platform_or_backend_dependency() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/semantics");
    for path in rust_sources(&root.join("src")) {
        let source = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        for forbidden in ["esp32", "riscv", "xtensa", "cortex", "thumb"] {
            assert!(
                !source.contains(forbidden),
                "{forbidden} identity leaked into semantic interface {}",
                path.display()
            );
        }
    }
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    for forbidden in ["backend-riscv", "harness-", "open-esp-radio-"] {
        assert!(
            !manifest.contains(forbidden),
            "semantic interface manifest contains platform/backend dependency {forbidden}"
        );
    }
}
