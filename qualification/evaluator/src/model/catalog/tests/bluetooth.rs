//! The selected secure product must consume its scenario without inheriting
//! stronger guarantees than the application or that scenario supplies.

use super::*;
use crate::model::{AsyncProof, Axis, HostProof, ImplementationProof};

#[test]
fn secure_gatt_requires_repeated_numeric_comparison_hil_but_retains_retirement_gaps() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let program = ManifestDocument::load_and_validate(
        &root.join("qualification/targets/esp32s31/bluetooth-secure-gatt.toml"),
        &root,
    )
    .unwrap()
    .document;
    let product = program
        .capabilities
        .iter()
        .find(|capability| capability.id == "secure-peripheral-gatt")
        .unwrap();
    let scope = product.catalog_scope.as_ref().unwrap();
    assert_eq!(
        scope.security,
        [
            "le-secure-connections",
            "numeric-comparison-only",
            "authenticated-att",
            "ram-bond",
        ]
    );
    assert_eq!(product.depends_on, ["peripheral-link-security"]);
    assert!(product.hil_not_applicable.is_none());
    let requirement = product
        .hil_requirements
        .iter()
        .find(|requirement| requirement.scenario == "bluetooth-trouble-secure-gatt")
        .expect("secure GATT must consume its own composed HIL scenario");
    assert_eq!(requirement.minimum_repetitions, 3);
    ScenarioCatalog::load(&root, Path::new("hil/scenarios"))
        .unwrap()
        .validate_requirement(&crate::hil::HilRequirement {
            scenario: requirement.scenario.clone(),
            checks: requirement.checks.clone(),
            minimum_repetitions: requirement.minimum_repetitions,
        })
        .unwrap();

    // A normal cold restart does not prove fault/cancellation disposition or
    // turn engineering observations into qualified hardware evidence.
    assert_eq!(product.implementation, ImplementationProof::Incomplete);
    assert_eq!(product.host, HostProof::Incomplete);
    assert_eq!(product.async_proof, AsyncProof::Incomplete);
    for (axis, id) in [
        (
            Axis::Implementation,
            "secure-gatt-terminal-host-controller-fault-disposition-incomplete",
        ),
        (
            Axis::Hil,
            "secure-gatt-coordinated-host-controller-retirement-qualified-evidence-missing",
        ),
        (
            Axis::Async,
            "trouble-host-controller-coordinated-shutdown-and-cancellation-incomplete",
        ),
    ] {
        assert!(
            product
                .gaps
                .iter()
                .any(|gap| gap.axis == axis && gap.id == id)
        );
    }
}
