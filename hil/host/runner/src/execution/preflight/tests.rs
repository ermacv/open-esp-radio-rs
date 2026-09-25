use super::*;

fn catalog() -> hil_core::scenario::Catalog {
    hil_core::scenario::Catalog::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"),
    )
    .expect("load scenario catalog")
}

#[test]
fn scheduler_compatibility_remains_a_selection_contract() {
    let catalog = catalog();
    let mut access_point = catalog
        .get("diagnostic-ap-mixed-tx-work")
        .expect("AP scenario")
        .clone();
    assert!(
        configure_run_selection(
            &mut access_point,
            Some(WifiApScheduler::DeficitHtResponse24),
            false,
            Integration::OwnedXarxa,
        )
        .is_ok()
    );

    let mut wrong_network = catalog
        .get("diagnostic-ap-mixed-tx-work")
        .expect("AP scenario")
        .clone();
    let error = configure_run_selection(
        &mut wrong_network,
        Some(WifiApScheduler::RrHtResponse24),
        false,
        Integration::UpstreamXarxa,
    )
    .unwrap_err();
    assert!(error.to_string().contains("--network owned-xarxa"));

    let mut replay = catalog
        .get("diagnostic-ap-mixed-tx-work")
        .expect("AP scenario")
        .clone();
    assert!(
        configure_run_selection(
            &mut replay,
            Some(WifiApScheduler::RrHtResponse24),
            true,
            Integration::UpstreamXarxa,
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
    validate_flashed_image(&lab, ImageClass::BootSmoke, output.path()).unwrap();
    assert!(std::fs::read_dir(output.path()).unwrap().next().is_none());
}
