//! The functional maintenance scope is independently selectable without weakening calibration.
use super::*;

#[test]
fn maintenance_continuity_keeps_rf_and_performance_claims_separate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let focused = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/wifi-maintenance-continuity.toml"),
        &root,
    )
    .unwrap()
    .document;
    assert_eq!(
        focused.required_capabilities,
        ["station-phy-maintenance-continuity"]
    );
    let narrow = &focused.capabilities[0];
    assert_eq!(narrow.hil_requirements[0].minimum_repetitions, 3);
    assert_eq!(
        narrow.hil_requirements[0].checks,
        [
            "wifi.maintenance.transaction-valid",
            "wifi.maintenance.same-link",
            "wifi.maintenance.ip-exchange-resumed"
        ]
    );
    assert!(narrow.hil_not_applicable.is_none());
    assert!(narrow.source_contracts.len() >= 4);
    let scope = narrow.catalog_scope.as_ref().unwrap();
    assert_eq!(scope.phy, "ht20");
    assert!(scope.limitations.contains("ICMP"));
    assert!(scope.limitations.contains("RF-quality"));
    let broad = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/wifi-sta.toml"),
        &root,
    )
    .unwrap()
    .document;
    let calibration = broad
        .capabilities
        .iter()
        .find(|d| d.id == "runtime-phy-calibration")
        .unwrap();
    assert_eq!(calibration.hil_requirements.len(), 7);
    assert!(
        calibration
            .gaps
            .iter()
            .any(|g| g.id == "repeated-current-clean-rf-quality-cells-missing")
    );
    let high_load = calibration
        .hil_requirements
        .iter()
        .find(|r| r.scenario == "diagnostic-station-phy-combined-high-load-delivery-rx")
        .unwrap();
    assert!(high_load.checks.iter().any(|c| c == "udp.rx.target-rate"));
    assert!(
        high_load
            .checks
            .iter()
            .any(|c| c == "udp.rx.maximum-silence")
    );
    for id in ["interrupt-recovery", "async-deadlines"] {
        let owner = broad.capabilities.iter().find(|d| d.id == id).unwrap();
        assert!(!owner.source_contracts.is_empty());
        assert!(!owner.development.host_tests.is_empty());
    }
}

#[test]
fn ap_availability_qualifies_only_selected_functional_transitions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let focused = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/wifi-ap-availability.toml"),
        &root,
    )
    .unwrap()
    .document;
    assert_eq!(
        focused.required_capabilities,
        ["station-ap-loss-recovery", "station-initial-ap-absence"]
    );
    for capability in &focused.capabilities {
        assert!(capability.depends_on.is_empty());
        assert!(!capability.source_contracts.is_empty());
        assert_eq!(capability.hil_requirements[0].minimum_repetitions, 3);
        assert!(
            capability.hil_requirements[0]
                .checks
                .iter()
                .any(|c| c == "wifi.station.control-responsive")
        );
    }
    let recovery = focused
        .capabilities
        .iter()
        .find(|c| c.id == "station-ap-loss-recovery")
        .unwrap();
    assert_eq!(
        recovery.implementation,
        crate::model::ImplementationProof::Incomplete
    );
    assert!(
        recovery
            .gaps
            .iter()
            .any(|gap| { gap.id == "ordinary-tx-protection-required-unimplemented" })
    );
    assert!(
        recovery.hil_requirements[0]
            .checks
            .iter()
            .any(|c| c == "wifi.station.recovered-ip-exchange")
    );
    let initial = focused
        .capabilities
        .iter()
        .find(|c| c.id == "station-initial-ap-absence")
        .unwrap();
    assert_eq!(
        initial.hil_requirements[0].scenario,
        "station-ap-initial-absence"
    );
    let broad = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/wifi-sta.toml"),
        &root,
    )
    .unwrap()
    .document;
    for id in [
        "interrupt-recovery",
        "async-deadlines",
        "timeout-error-recovery",
    ] {
        let c = broad.capabilities.iter().find(|c| c.id == id).unwrap();
        assert!(
            !c.gaps.is_empty(),
            "functional success cannot discharge {id}'s wider requirements"
        );
    }
}
