use super::*;

fn catalog() -> crate::scenario::Catalog {
    crate::scenario::Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"))
        .expect("load scenario catalog")
}

#[test]
fn boot_smoke_preflight_never_opens_a_serial_capture() {
    let lab = LabConfig::for_test();
    let output = tempfile::tempdir().unwrap();
    validate_flashed_image(&lab, catalog().get("boot-smoke").unwrap(), output.path()).unwrap();
    assert!(std::fs::read_dir(output.path()).unwrap().next().is_none());
}

#[test]
fn a_flashed_image_must_declare_the_role_its_scenario_drives() {
    let catalog = catalog();
    let dtm = ImageClass::BluetoothDtm.image_keys_on("esp32s31").unwrap();
    check_flashed_image_keys(
        "esp32s31",
        catalog.get("bluetooth-dtm-bidirectional").unwrap(),
        &dtm,
    )
    .unwrap();
    let scannable = catalog.get("bluetooth-scannable-advertising").unwrap();
    check_flashed_image_keys("esp32s31", scannable, &dtm).unwrap();
    // The advertising workloads drive the Controller over raw HCI.
    let without_hci = DeviceImageKeys::of_keys(dtm.keys().iter().copied().filter(|key| {
        *key != <oer_hil_protocol::bluetooth::Hci as oer_hil_protocol::Message>::KEY
    }));
    assert!(check_flashed_image_keys("esp32s31", scannable, &without_hci).is_err());
}

#[test]
fn an_esp32c5_system_image_is_classified_by_its_chip() {
    let catalog = catalog();
    let scenario = catalog.get("system-watchdog").unwrap();
    let esp32c5 = ImageClass::SystemWatchdog.image_keys_on("esp32c5").unwrap();
    check_flashed_image_keys("esp32c5", scenario, &esp32c5).unwrap();
    // The chip comes from the lab, the class from the keys: the esp32c5 builds
    // no Wi-Fi image, so a Wi-Fi image's keys name no esp32c5 class.
    let staged = ImageClass::Performance.image_keys_on("esp32s31").unwrap();
    assert!(check_flashed_image_keys("esp32c5", scenario, &staged).is_err());
}
