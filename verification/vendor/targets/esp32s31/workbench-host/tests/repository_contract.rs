use std::process::Command;
use std::{fs, path::Path};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("ESP32-S31 host remains under verification/vendor/targets")
}

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

fn named_files(root: &Path, name: &str, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            named_files(&path, name, output);
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            output.push(path);
        }
    }
}

#[test]
fn closed_chip_pac_is_the_only_driver_dependency_on_the_raw_pac() {
    let repository = repository_root();
    let driver = repository.join("driver");
    let closed_manifest = driver.join("chips/esp32s31/pac/Cargo.toml");
    let raw_manifest = driver.join("chips/esp32s31/pac-raw/Cargo.toml");
    let mut manifests = Vec::new();
    named_files(&driver, "Cargo.toml", &mut manifests);
    let violations = manifests
        .into_iter()
        .filter(|path| path != &closed_manifest && path != &raw_manifest)
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read driver manifest")
                .contains("open-esp-radio-esp32s31-pac-raw")
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "driver crates bypass the closed PAC: {violations:#?}"
    );

    let generated = fs::read_to_string(driver.join("chips/esp32s31/pac/src/generated.rs"))
        .expect("read generated closed-PAC domains");
    assert!(generated.contains("pub struct MacInterruptMask(u32);"));
    assert!(!generated.contains("from_bits"));
    assert!(!generated.contains("open_esp_radio_esp32s31_pac_raw"));

    let mut closed_files = Vec::new();
    rust_files(&driver.join("chips/esp32s31/pac/src"), &mut closed_files);
    let direct_raw_imports = closed_files
        .iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("lib.rs"))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read closed PAC module")
                .contains("open_esp_radio_esp32s31_pac_raw")
        })
        .collect::<Vec<_>>();
    assert!(
        direct_raw_imports.is_empty(),
        "closed PAC modules bypass the single private raw-PAC alias: {direct_raw_imports:#?}"
    );
    let bypasses = closed_files
        .into_iter()
        .filter(|path| {
            !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("generated.rs" | "validation.rs")
            )
        })
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read closed PAC module");
            source.contains("::full_register_write::")
                || source.contains("::register_image_write::")
                || source.contains("::masked_register_modify::")
        })
        .collect::<Vec<_>>();
    assert!(
        bypasses.is_empty(),
        "closed PAC modules bypass generated transactions: {bypasses:#?}"
    );

    let facade = fs::read_to_string(driver.join("chips/esp32s31/pac/src/lib.rs"))
        .expect("read closed PAC facade");
    for rejected in [
        "use open_esp_radio_esp32s31_pac::svd;",
        "use open_esp_radio_esp32s31_pac::Register32;",
        "MacInterruptMask(0xdead_beef)",
        "RadioRegisters::write_register",
    ] {
        assert!(
            facade.contains(rejected),
            "closed PAC must retain compile-fail coverage for `{rejected}`"
        );
    }
}

#[test]
fn reviewed_sta_ap_and_coex_boundaries_hide_register_mechanics() {
    let repository = repository_root();
    let boundaries = [
        repository.join("driver/chips/esp32s31/hal/src/wifi_mac.rs"),
        repository.join("driver/chips/esp32s31/hal/src/coex.rs"),
        repository.join("driver/chips/esp32s31/wifi/mac/src/sta_ap_registers.rs"),
        repository.join("driver/chips/esp32s31/wifi/mac/src/coex_runtime.rs"),
    ];
    for path in boundaries {
        let source = fs::read_to_string(&path).expect("read reviewed register boundary");
        for forbidden in [
            "open_esp_radio_esp32s31_pac_raw",
            "Register32",
            "0x2010_",
            "write_volatile",
            "read_volatile",
        ] {
            assert!(
                !source.contains(forbidden),
                "reviewed boundary {} exposes {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn cargo_alias_selects_the_product_host_and_keeps_build_output_visible() {
    let config = fs::read_to_string(repository_root().join(".cargo/config.toml")).unwrap();
    let alias = config
        .lines()
        .find(|line| line.trim_start().starts_with("vendor-binary-workbench ="))
        .expect("repository provides the documented Workbench alias");
    assert!(alias.contains("open-radio-vendor-workbench-esp32s31-host"));
    assert!(alias.contains("run --profile workbench"));
    assert!(!alias.contains("run --quiet"));
}

#[test]
fn analysis_input_builder_owns_every_generated_project_role() {
    let builder =
        repository_root().join("verification/vendor/targets/esp32s31/build-analysis-inputs");
    let output = Command::new(&builder)
        .arg("--list-roles")
        .output()
        .expect("query analysis-input producer contract");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [
        "source-artifact:archive",
        "source-artifact:libpp",
        "source-artifact:libpp-replay",
        "source-artifact:libnet80211",
        "source-artifact:wifi-sta-ap-receive",
        "source-artifact:wifi-sta-lifecycle",
        "source-artifact:wifi-key-role",
        "source-artifact:coex",
        "source-artifact:btbb",
        "rust-artifact",
        "rust-artifact:wifi-registers",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn platform_pack_owns_its_pinned_semantic_catalog() {
    let target = repository_root().join("verification/vendor/targets/esp32s31");
    let platform =
        fs::read_to_string(target.join("platform.toml")).expect("read target platform pack");
    assert!(platform.contains("semantic-catalogs = [\"semantics/embedded-platform.toml\"]"));
    assert!(!platform.contains("tools/vendor-binary-workbench"));
    assert!(target.join("semantics/embedded-platform.toml").is_file());
}
