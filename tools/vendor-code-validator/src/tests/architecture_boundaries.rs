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

#[test]
fn validator_source_contains_no_private_paths_or_embedded_digests() {
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
fn neutral_core_has_no_architecture_or_platform_identity() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/core/src");
    for path in rust_sources(&core) {
        let source = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        for forbidden in ["esp32", "riscv", "xtensa", "cortex", "thumb"] {
            assert!(
                !source.contains(forbidden),
                "{forbidden} identity leaked into neutral core source {}",
                path.display()
            );
        }
    }
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/core/Cargo.toml"))
            .unwrap();
    assert!(
        !manifest.contains("[dependencies]"),
        "neutral core must remain dependency-free"
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
    let shared_model = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/model/src");
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
            "validator facade directly owns backend/model dependency {forbidden}"
        );
    }
}

#[test]
fn semantic_interface_has_no_platform_or_backend_dependency() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/semantic");
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
