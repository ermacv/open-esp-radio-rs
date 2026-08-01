use super::*;

#[test]
fn checked_in_manifest_is_strict_and_resolves_defaults() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("dispositions/esp32s31.disposition");
    let manifest = Manifest::load(&path).unwrap();
    assert_eq!(manifest.entries().count(), 9);

    let disable_agc = manifest.resolve("rom", "phy_disable_agc");
    assert_eq!(disable_agc.disposition, Disposition::Direct);
    let effect_contract = disable_agc.entry.unwrap().effect_contract.as_ref().unwrap();
    assert_eq!(effect_contract.comparison, EffectComparison::ExactEffectsV1);
    assert_eq!(effect_contract.rules().count(), 2);
    let binding = disable_agc.entry.unwrap().binding.as_ref().unwrap();
    assert_eq!(binding.version, BindingVersion::V1);
    assert_eq!(binding.revision, VendorRevision::Esp32s31Eco0Rom);
    assert_eq!(binding.rust_probe, "open_phy_trace_disable_agc");
    assert_eq!(binding.driver_adapter, None);

    let iq_enable = manifest.resolve("rom", "phy_iq_est_enable");
    assert_eq!(iq_enable.disposition, Disposition::StateTransition);
    let iq_enable = iq_enable.entry.unwrap();
    assert_eq!(
        iq_enable.binding.as_ref().unwrap().driver_adapter,
        Some(DriverAdapter::Esp32s31IqEstEnableV1)
    );
    assert_eq!(
        iq_enable.effect_contract.as_ref().unwrap().rules().count(),
        9
    );

    let root = manifest.resolve("archive", "register_chipv7_phy");
    assert_eq!(root.disposition, Disposition::ReplacedByComposition);
    assert_eq!(root.protocol, Protocol::Shared);
    let root = root.entry.unwrap();
    assert!(root.rust_component.is_some());
    assert_eq!(
        root.qualification_blockers,
        [("archive".to_owned(), "phy_bb_init".to_owned())]
    );

    let bb_init = manifest.resolve("archive", "phy_bb_init");
    assert_eq!(
        bb_init.entry.unwrap().qualification_blockers,
        [("archive".to_owned(), "phy_bt_tx_gain_init".to_owned())]
    );

    let channel = manifest.resolve("archive", "phy_chip_set_chan");
    assert_eq!(
        channel.entry.unwrap().semantic_contract,
        Some(SemanticContract::Esp32s31Channel)
    );

    let rf_init = manifest.resolve("archive", "phy_rf_init");
    assert_eq!(
        rf_init.entry.unwrap().semantic_contract,
        Some(SemanticContract::Esp32s31RfInit)
    );

    let bluetooth_txdc = manifest.resolve("archive", "phy_bt_txdc_cal_new");
    assert_eq!(
        bluetooth_txdc.entry.unwrap().semantic_contract,
        Some(SemanticContract::Esp32s31BluetoothTxDc)
    );

    let bluetooth_tx_power = manifest.resolve("archive", "phy_bt_tx_pwctrl_init");
    assert_eq!(
        bluetooth_tx_power.entry.unwrap().semantic_contract,
        Some(SemanticContract::Esp32s31BluetoothTxPower)
    );

    let bluetooth_txdc_pwdet = manifest.resolve("archive", "phy_txdc_cal_pwdet_init");
    assert_eq!(
        bluetooth_txdc_pwdet.entry.unwrap().semantic_contract,
        Some(SemanticContract::Esp32s31BluetoothTxDcPwdet)
    );

    let bluetooth = manifest.resolve("rom", "phy_bt_filter_reg");
    assert_eq!(bluetooth.disposition, Disposition::NotYetPorted);
    assert_eq!(bluetooth.protocol, Protocol::Bluetooth);

    let unknown = manifest.resolve("rom", "phy_unclassified_example");
    assert_eq!(unknown.disposition, Disposition::NotYetPorted);
    assert_eq!(unknown.protocol, Protocol::Unknown);
}
