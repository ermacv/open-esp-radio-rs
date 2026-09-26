//! Owned software policy and hardware obligations stay separately visible.
use super::*;

#[test]
fn software_policy_does_not_require_vendor_equivalence_or_hide_hardware_obligations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let program = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/wifi-sta.toml"),
        &root,
    )
    .unwrap()
    .document;
    let capability = |id: &str| program.capabilities.iter().find(|c| c.id == id).unwrap();
    for id in ["authentication-association", "wpa2"] {
        let owned = capability(id);
        assert!(owned.vendor_not_applicable.is_some());
        assert!(owned.vendor_roots.is_empty());
        assert!(owned.vendor_anchors.is_empty());
        assert!(!owned.hil_requirements.is_empty());
        assert!(!owned.source_contracts.is_empty());
        assert!(!owned.development.host_tests.is_empty());
    }
    assert!(
        capability("wpa2")
            .depends_on
            .iter()
            .any(|d| d == "station-hardware-crypto")
    );
    for id in [
        "station-hardware-crypto",
        "interrupt-recovery",
        "async-deadlines",
        "radio-cold-restart",
        "timeout-error-recovery",
    ] {
        let hardware = capability(id);
        assert!(hardware.vendor_not_applicable.is_none());
        assert!(!hardware.vendor_roots.is_empty() || !hardware.vendor_anchors.is_empty());
        assert!(
            hardware
                .gaps
                .iter()
                .any(|g| g.axis == crate::model::Axis::Vendor)
        );
    }
    let bluetooth = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/bluetooth-peripheral-acl.toml"),
        &root,
    )
    .unwrap()
    .document;
    for id in ["peripheral-acl", "peripheral-link-security"] {
        let hardware = bluetooth.catalog.capabilities.get(id).unwrap();
        assert!(hardware.vendor_not_applicable.is_none());
        assert!(
            hardware
                .gaps
                .iter()
                .any(|g| g.axis == crate::model::Axis::Vendor)
        );
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
        crate::model::ImplementationProof::Complete
    );
    assert!(recovery.gaps.is_empty());
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
