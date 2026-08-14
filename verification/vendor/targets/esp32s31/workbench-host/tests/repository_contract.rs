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
    assert!(generated.contains("pub enum MacRxBeaconClearRequest"));
    assert!(!generated.contains("from_bits"));
    assert!(!generated.contains("open_esp_radio_esp32s31_pac_raw"));
    let raw = fs::read_to_string(driver.join("chips/esp32s31/pac-raw/src/lib.rs"))
        .expect("read generated raw PAC");
    assert!(!raw.contains("input & 0x00000000"));

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
                || source.contains("::indexed_bit_set_modify::")
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
fn hal_is_the_only_driver_dependency_on_the_closed_pac() {
    let repository = repository_root();
    let driver = repository.join("driver");
    let hal_manifest = driver.join("chips/esp32s31/hal/Cargo.toml");
    let pac_manifest = driver.join("chips/esp32s31/pac/Cargo.toml");
    let raw_manifest = driver.join("chips/esp32s31/pac-raw/Cargo.toml");
    let mut manifests = Vec::new();
    named_files(&driver, "Cargo.toml", &mut manifests);
    let violations = manifests
        .into_iter()
        .filter(|path| path != &hal_manifest && path != &pac_manifest && path != &raw_manifest)
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read driver manifest")
                .contains("open-esp-radio-esp32s31-pac")
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "driver crates bypass the HAL, including through dev-dependencies: {violations:#?}"
    );

    let pac_manifest = fs::read_to_string(pac_manifest).expect("read closed PAC manifest");
    assert!(
        !pac_manifest.contains("test-register-catalog"),
        "closed PAC restored its removed external test register catalog"
    );
}

#[test]
fn powered_phy_capability_cannot_recover_the_pac_owner() {
    let repository = repository_root();
    let chip = repository.join("driver/chips/esp32s31");
    let facade = fs::read_to_string(chip.join("hal/src/lib.rs")).expect("read HAL facade");

    for removed in [
        "PhyRegisterAccess",
        "impl Deref for PhyHal",
        "impl DerefMut for PhyHal",
        "phy_parts_mut",
        "registers_mut",
    ] {
        assert!(
            !facade.contains(removed),
            "HAL restored removed generic PHY escape `{removed}`"
        );
    }

    let mut phy_files = Vec::new();
    rust_files(&chip.join("phy/src"), &mut phy_files);
    let violations = phy_files
        .into_iter()
        .filter(|path| {
            let source = fs::read_to_string(path).expect("read PHY source");
            source.contains("open_esp_radio_esp32s31_pac")
                || source.contains("RadioRegisters")
                || source.contains("ColdRadioRegisters")
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "PHY source recovered a PAC owner outside HAL: {violations:#?}"
    );
}

#[test]
fn hal_public_functions_do_not_accept_a_pac_owner() {
    let hal = repository_root().join("driver/chips/esp32s31/hal/src");
    let mut files = Vec::new();
    rust_files(&hal, &mut files);
    let violations = files
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read HAL source")
                .lines()
                .any(|line| {
                    line.contains("pub fn ")
                        && line.contains('(')
                        && (line.contains("RadioRegisters") || line.contains("ColdRadioRegisters"))
                })
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "HAL public signatures expose PAC owner parameters: {violations:#?}"
    );
}

#[test]
fn completed_trust_reset_inventory_cannot_return() {
    let repository = repository_root();
    assert!(
        !repository.join("TRUST_RESET_INVENTORY.md").exists(),
        "the retired narrative inventory must not return"
    );
    assert!(
        !repository
            .join("verification/vendor/targets/esp32s31/audits/verification-bindings.toml")
            .exists(),
        "the cutover inventory must be deleted after every decision is encoded in current manifests"
    );
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
fn ecosystem_pack_composes_the_shared_espressif_family_knowledge() {
    let vendor = repository_root().join("verification/vendor");
    let target = vendor.join("targets/esp32s31");
    let ecosystem = fs::read_to_string(target.join("ecosystem.toml")).expect("read ecosystem pack");
    assert!(ecosystem.contains("knowledge-packs"));
    assert!(ecosystem.contains("neutral-embedded.toml"));
    assert!(ecosystem.contains("../../knowledge/espressif/esp-idf.toml"));
    assert!(vendor.join("knowledge/espressif/esp-idf.toml").is_file());
    assert!(!target.join("semantics/embedded-platform.toml").exists());
}

#[test]
fn chip_knowledge_cannot_acquire_a_production_or_qualification_dependency() {
    let provider =
        repository_root().join("verification/vendor/targets/esp32s31/workbench-provider");
    let knowledge = fs::read_to_string(provider.join("knowledge/Cargo.toml")).unwrap();
    for forbidden in [
        "driver/chips",
        "open-esp-radio-esp32s31",
        "qualification-check",
        "qualification/",
    ] {
        assert!(
            !knowledge.contains(forbidden),
            "chip knowledge provider acquired forbidden dependency {forbidden}"
        );
    }

    assert!(
        !provider.join("verification").exists(),
        "compiled target code must not own comparison verdicts"
    );
}

#[test]
fn removed_provider_and_provenance_paths_do_not_return() {
    let repository = repository_root();
    let provider = repository.join("verification/vendor/targets/esp32s31/workbench-provider");
    assert!(!provider.join("semantic").exists());
    assert!(!provider.join("verification/src/qualification").exists());
    assert!(!provider.join("verification").exists());
    assert!(!provider.join("verification/src/semantic_replay").exists());

    let evidence = repository.join("verification/vendor/targets/esp32s31/registers/evidence");
    assert!(!evidence.join("migration.toml").exists());
    assert!(evidence.join("promoted-driver-provenance.toml").is_file());

    let mut production = Vec::new();
    rust_files(&repository.join("driver"), &mut production);
    let stale = production
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read production source")
                .contains("migration/")
        })
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "production source cites removed migration paths: {stale:#?}"
    );
}

#[test]
fn channel_lifecycle_reaches_the_pac_only_through_the_hal() {
    let repository = repository_root();
    let driver = repository.join("driver/chips/esp32s31");
    for manifest in [
        driver.join("phy/Cargo.toml"),
        driver.join("wifi/Cargo.toml"),
    ] {
        let input = fs::read_to_string(&manifest).expect("read role-neutral manifest");
        let dependencies = input
            .split("[dev-dependencies]")
            .next()
            .expect("manifest prefix");
        assert!(
            !dependencies.contains("open-esp-radio-esp32s31-pac"),
            "{} bypasses the HAL in production dependencies",
            manifest.display()
        );
    }

    let target_port =
        fs::read_to_string(driver.join("phy/src/target_port.rs")).expect("read PHY target port");
    assert!(target_port.contains("channel::RadioChannelHal"));
    assert!(!target_port.contains("request_mac_channel_stop_without_power_save"));

    let cold_start = fs::read_to_string(driver.join("wifi/src/cold_start.rs"))
        .expect("read production cold-start channel path");
    assert!(cold_start.contains("channel_hal()"));
    assert!(cold_start.contains("select_phy_channel_with_hal"));
    assert!(!cold_start.contains("powered.parts_mut()"));

    let production_trace = fs::read_to_string(
        repository
            .join("verification/vendor/targets/esp32s31/probes/library/src/production_trace.rs"),
    )
    .expect("read exact production trace wrapper");
    assert!(production_trace.contains("Radio::claim_for_validation(platform)"));
    assert!(production_trace.contains("channel_hal()"));
    assert!(!production_trace.contains("pac::validation"));
    assert!(!production_trace.contains("RadioRegisters"));

    let capability =
        fs::read_to_string(driver.join("hal/src/channel.rs")).expect("read channel HAL capability");
    assert!(!capability.contains("pub fn registers_mut"));
    assert!(!capability.contains("pub fn into_registers"));
}

#[test]
fn compiled_probe_operations_do_not_call_pac_validation_directly() {
    let repository = repository_root();
    let probe = fs::read_to_string(
        repository.join("verification/vendor/targets/esp32s31/probes/library/src/lib.rs"),
    )
    .expect("read compiled probe library");
    assert!(
        !probe.contains("open_esp_radio_esp32s31_pac::validation"),
        "compiled operations must acquire validation ownership through HAL"
    );
    for retired_prefix in [
        "open_libpp_coex_trace_",
        "open_libpp_power_trace_",
        "open_libpp_power_tsf_trace_",
        "open_btbb_coex_trace_",
        "open_coex_scheduler_trace_",
        "open_libpp_trace_wdev_process_fiq_mac_slice",
        "open_phy_trace_iq_est_enable",
        "open_libpp_tx_trace_hal_mac_txq_publish_owned",
        "open_libnet80211_trace_sta_join_state",
        "open_wifi_key_role_trace_wdev_insert_key_entry",
    ] {
        assert!(
            !probe.contains(retired_prefix),
            "quarantined probe entry returned: {retired_prefix}"
        );
    }

    let pac_validation =
        fs::read_to_string(repository.join("driver/chips/esp32s31/pac/src/validation.rs"))
            .expect("read PAC validation capability factory");
    let exported = pac_validation
        .lines()
        .filter(|line| line.trim_start().starts_with("pub fn "))
        .collect::<Vec<_>>();
    assert_eq!(
        exported.len(),
        3,
        "PAC validation regained semantic operations"
    );
    assert!(pac_validation.contains("pub fn radio_registers()"));
    assert!(pac_validation.contains("pub fn mac_interrupt_registers()"));
    assert!(pac_validation.contains("pub fn mac_power_interrupt_registers()"));

    let hal = repository.join("driver/chips/esp32s31/hal/src");
    let mut hal_files = Vec::new();
    rust_files(&hal, &mut hal_files);
    let direct_acquisition = hal_files
        .into_iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("lib.rs"))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read HAL source")
                .contains("open_esp_radio_esp32s31_pac::validation")
        })
        .collect::<Vec<_>>();
    assert!(
        direct_acquisition.is_empty(),
        "HAL modules bypass the single validation owner factory: {direct_acquisition:#?}"
    );
}

#[test]
fn polling_and_multi_register_channel_sequences_are_owned_by_the_hal() {
    let repository = repository_root();
    let pac = repository.join("driver/chips/esp32s31/pac/src");
    let hal = repository.join("driver/chips/esp32s31/hal/src");

    let cold_pac = fs::read_to_string(pac.join("mac_cold_start.rs")).unwrap();
    assert!(!cold_pac.contains("loop {"));
    assert!(!cold_pac.contains("sample_limit"));
    let mac_hal = fs::read_to_string(hal.join("wifi_mac.rs")).unwrap();
    assert!(mac_hal.contains("begin_cold_mac_handshake"));
    assert!(mac_hal.contains("sample_limit"));
    assert!(mac_hal.contains("resume_mac_channel_without_power_save"));
    assert!(mac_hal.contains("select_wifi_no_power_save_regdma_link"));
}

#[test]
fn radio_arena_escape_surface_is_frozen() {
    let repository = repository_root();
    assert!(
        !repository
            .join("driver/chips/esp32s31/wifi/src/register_arena.rs")
            .exists(),
        "runtime MMIO serialization belongs to the HAL"
    );
    let arena = fs::read_to_string(repository.join("driver/chips/esp32s31/hal/src/radio_arena.rs"))
        .unwrap();
    for forbidden in [
        "pub fn with_mut",
        "pub fn with_ref",
        "pub fn try_with_ref",
        "unsafe impl Sync",
        "unsafe impl Send",
    ] {
        assert!(
            !arena.contains(forbidden),
            "register arena widened its generic escape surface with {forbidden}"
        );
    }
    assert!(arena.contains("explicit serialization owner"));
    assert!(arena.contains("try_station_receive_policy_snapshot"));
    assert!(arena.contains("try_receive_statistics_snapshot"));
    assert!(arena.contains("try_receive_dma_snapshot"));
    assert!(arena.contains("try_configure_station_receive_policy"));
    assert!(arena.contains("try_noise_floor_dbm"));
    assert!(arena.contains("try_install_station_ccmp_entry"));
    assert!(arena.contains("try_clear_ccmp_entry"));
    assert!(arena.contains("try_ccmp_entry_is_valid"));
    assert!(arena.contains("try_channel_hal"));
    assert!(arena.contains("try_wifi_mac_hal"));

    // Neither the unique root lease nor its copyable access handle may expose
    // the published PAC owner. Runtime code receives only named operations or
    // narrow guards such as `WifiMacHal` and `RadioChannelHal`.
    assert_eq!(arena.matches("pub fn borrow").count(), 0);

    for path in [
        "driver/adapters/embassy/esp32s31-wifi/src/scan_target.rs",
        "driver/adapters/embassy/esp32s31-wifi/src/sta_attempt_target/channel.rs",
    ] {
        let source = fs::read_to_string(repository.join(path)).unwrap();
        assert!(source.contains("switch_published_channel"));
        assert!(!source.contains("borrow_mut()"));
    }
    let connected = fs::read_to_string(
        repository.join("driver/integration/esp32s31/embassy-wifi/src/connected.rs"),
    )
    .unwrap();
    assert!(connected.contains("try_prepare_connected_sta_without_power_save"));
    assert!(!connected.contains("access.borrow_mut()"));
    assert!(!connected.contains(".borrow()\n            .rx_statistics_snapshot()"));
}

#[test]
fn esp32c5_fixture_composes_only_neutral_and_family_knowledge() {
    let target = repository_root().join("verification/vendor/targets/esp32c5");
    let ecosystem = fs::read_to_string(target.join("ecosystem.toml")).unwrap();
    assert!(ecosystem.contains("../../knowledge/espressif/esp-idf.toml"));
    assert!(!ecosystem.contains("knowledge-provider"));
    assert!(!ecosystem.contains("esp32s31"));

    let target_spec = fs::read_to_string(target.join("target.toml")).unwrap();
    assert!(!target_spec.contains("memory-map"));
    assert!(!target_spec.contains("0x"));

    let output = Command::new(env!("CARGO_BIN_EXE_vendor-binary-workbench"))
        .args([
            "project",
            "configure",
            "--project",
            target.join("vendor-project.toml").to_str().unwrap(),
            "--check",
            "--format",
            "json",
            "--color",
            "never",
        ])
        .output()
        .expect("validate C5 portability fixture");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
