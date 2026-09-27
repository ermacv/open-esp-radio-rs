use super::*;

fn catalog() -> crate::scenario::Catalog {
    crate::scenario::Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"))
        .expect("load scenario catalog")
}

#[test]
fn scheduler_selection_applies_only_to_standalone_access_points() {
    let catalog = catalog();
    let mut access_point = catalog
        .get("diagnostic-ap-mixed-tx-work")
        .expect("AP scenario")
        .clone();
    assert!(
        configure_run_selection(
            &mut access_point,
            Some(WifiApScheduler::DeficitHtResponse24)
        )
        .is_ok()
    );
}

#[test]
fn boot_smoke_preflight_never_opens_a_serial_capture() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local.example.toml");
    let private = tempfile::tempdir().unwrap();
    let lab_path = private.path().join("local.toml");
    std::fs::copy(source, &lab_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&lab_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let lab = LabConfig::load(&lab_path).expect("example lab config");
    let output = tempfile::tempdir().unwrap();
    validate_flashed_image(&lab, catalog().get("boot-smoke").unwrap(), output.path()).unwrap();
    assert!(std::fs::read_dir(output.path()).unwrap().next().is_none());
}

#[test]
fn a_flashed_image_must_declare_the_role_its_scenario_drives() {
    let catalog = catalog();
    let dtm = ImageClass::BluetoothDtm.console_capabilities().unwrap();
    check_flashed_capabilities(catalog.get("bluetooth-dtm-bidirectional").unwrap(), &dtm).unwrap();
    let connection = catalog
        .get("bluetooth-peripheral-local-disconnect")
        .unwrap();
    assert!(check_flashed_capabilities(connection, &dtm).is_err());
    let peripheral = FeatureCapabilities {
        bluetooth_peripheral: true,
        ..dtm
    };
    check_flashed_capabilities(connection, &peripheral).unwrap();
}
