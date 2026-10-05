use super::*;

fn catalog() -> crate::scenario::Catalog {
    crate::scenario::Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"))
        .expect("load scenario catalog")
}

/// The chip whose agent builds the performance image (the full agent) and
/// one whose agent does not.
fn chips() -> (String, String) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let full = oer_hil_image::chips_building(&root, &[ImageClass::Performance]).unwrap();
    let partial = oer_chip_profile::supported(&root)
        .unwrap()
        .into_iter()
        .find(|chip| !full.contains(chip))
        .unwrap();
    (full[0].clone(), partial)
}

#[test]
fn boot_smoke_preflight_never_opens_a_serial_capture() {
    let lab = LabConfig::for_test();
    let output = tempfile::tempdir().unwrap();
    validate_flashed_image(
        &lab,
        catalog().get("boot-smoke").unwrap(),
        output.path(),
        None,
    )
    .unwrap();
    assert!(std::fs::read_dir(output.path()).unwrap().next().is_none());
}

#[test]
fn a_flashed_image_must_declare_the_role_its_scenario_drives() {
    let catalog = catalog();
    let (full, _) = chips();
    let dtm = oer_hil_image_class::image_keys_on(ImageClass::BluetoothDtm, &full).unwrap();
    check_flashed_image_keys(
        &full,
        catalog.get("bluetooth-dtm-bidirectional").unwrap(),
        &dtm,
    )
    .unwrap();
    let scannable = catalog.get("bluetooth-scannable-advertising").unwrap();
    check_flashed_image_keys(&full, scannable, &dtm).unwrap();
    // The advertising workloads drive the Controller over raw HCI.
    let without_hci = DeviceImageKeys::of_keys(dtm.keys().iter().copied().filter(|key| {
        *key != <oer_hil_protocol::bluetooth::Hci as oer_hil_protocol::Message>::KEY
    }));
    assert!(check_flashed_image_keys(&full, scannable, &without_hci).is_err());
}

#[test]
fn a_partial_agent_s_system_image_is_classified_by_its_chip() {
    let catalog = catalog();
    let (full, partial) = chips();
    let scenario = catalog.get("system-watchdog").unwrap();
    let keys = oer_hil_image_class::image_keys_on(ImageClass::SystemWatchdog, &partial).unwrap();
    check_flashed_image_keys(&partial, scenario, &keys).unwrap();
    // The chip comes from the lab, the class from the keys: the partial
    // agent builds no Wi-Fi image, so a Wi-Fi image's keys name none of its
    // classes.
    let wifi = oer_hil_image_class::image_keys_on(ImageClass::Performance, &full).unwrap();
    assert!(check_flashed_image_keys(&partial, scenario, &wifi).is_err());
}

#[test]
fn the_boot_smoke_recovery_image_is_recognized_by_its_pass_line() {
    // It reports no image keys, so no key check could ever accept it.
    assert!(oer_hil_image_class::image_keys_on(ImageClass::BootSmoke, &chips().0).is_none());
    assert_eq!(
        recognition(ImageClass::BootSmoke),
        Recognition::BootSmokeLine
    );
    assert_eq!(
        recognition(ImageClass::SystemWatchdog),
        Recognition::ImageKeys
    );
}
