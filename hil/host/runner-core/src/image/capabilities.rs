//! What each image class's runtime compiles and reports, derived from the
//! runtime manifest's feature graph.
//!
//! A class names the Cargo features its runtime is built with; the manifest
//! says which features each of those enables in turn. The closure of that
//! graph is exactly what `cfg(feature = ...)` sees in the runtime, and the
//! runtime reports its capabilities with the same function of it
//! ([`oer_hil_target_core::image_capabilities`]), so the host's expectation
//! of a flashed image cannot drift from the firmware.

use std::collections::{BTreeMap, BTreeSet};

use oer_hil_protocol::FeatureCapabilities;

use super::{ImageClass, Integration};

/// The runtime manifest of this tree.
const RUNTIME_MANIFEST: &str = include_str!("../../../../targets/esp32s31/runtime/Cargo.toml");

/// The runtime's features and the features each enables, leaving out
/// optional dependencies (`dep:x`) and other packages' features (`x/f`,
/// `x?/f`), which `cfg(feature)` of the runtime does not see.
fn feature_graph(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let manifest: toml::Table = toml::from_str(manifest).expect("the runtime manifest parses");
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("the runtime manifest declares features");
    features
        .iter()
        .map(|(name, enables)| {
            let own = enables
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .filter(|member| !member.starts_with("dep:") && !member.contains('/'))
                .map(str::to_owned)
                .collect();
            (name.clone(), own)
        })
        .collect()
}

/// The features `roots` enable through `graph`, themselves included.
fn closure(graph: &BTreeMap<String, Vec<String>>, roots: &[&str]) -> BTreeSet<String> {
    let mut enabled = BTreeSet::new();
    let mut pending = roots
        .iter()
        .map(|root| (*root).to_owned())
        .collect::<Vec<_>>();
    while let Some(feature) = pending.pop() {
        if enabled.insert(feature.clone())
            && let Some(members) = graph.get(&feature)
        {
            pending.extend(members.iter().cloned());
        }
    }
    enabled
}

impl ImageClass {
    /// Every Cargo feature this class's runtime is built with.
    pub fn enabled_features(self) -> BTreeSet<String> {
        let features = self.build_features(Integration::OwnedXarxa);
        closure(
            &feature_graph(RUNTIME_MANIFEST),
            &features.split(',').collect::<Vec<_>>(),
        )
    }

    /// The capabilities this class's runtime reports, or `None` for the boot
    /// smoke image, which reports none.
    pub fn capabilities(self) -> Option<FeatureCapabilities> {
        if self == Self::BootSmoke {
            return None;
        }
        let enabled = self.enabled_features();
        Some(
            oer_hil_target_core::image_capabilities::feature_capabilities(&|feature| {
                enabled.contains(feature)
            }),
        )
    }

    /// Whether the image compiles the program-counter sampler
    /// (`pc-profile`), so a scenario can request a profile of it.
    pub fn samples_program_counter(self) -> bool {
        self.enabled_features().contains("pc-profile")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feature_graph_keeps_only_the_runtime_s_own_features() {
        let graph = feature_graph(
            r#"
            [features]
            a = ["b", "dep:x", "y/z", "w?/v"]
            b = ["c"]
            c = []
            "#,
        );
        assert_eq!(graph["a"], ["b"]);
        assert_eq!(
            closure(&graph, &["a"]),
            ["a", "b", "c"].map(String::from).into()
        );
    }

    #[test]
    fn every_class_but_boot_smoke_reports_capabilities_no_other_class_does() {
        let mut seen = BTreeMap::new();
        for class in ImageClass::ALL {
            let Some(capabilities) = class.capabilities() else {
                assert_eq!(class, ImageClass::BootSmoke);
                continue;
            };
            if let Some(other) = seen.insert(format!("{capabilities:?}"), class) {
                panic!(
                    "{} and {} report the same capabilities",
                    other.id(),
                    class.id()
                );
            }
            assert_eq!(
                crate::image::classify_flashed_capabilities(&capabilities),
                Some(class),
                "{}",
                class.id()
            );
        }
    }

    #[test]
    fn a_capability_no_class_builds_is_no_class() {
        let performance = ImageClass::Performance.capabilities().unwrap();
        for foreign in [
            FeatureCapabilities {
                phy_fault_injection: true,
                ..performance
            },
            FeatureCapabilities {
                bluetooth_dtm: true,
                ..performance
            },
            FeatureCapabilities {
                udp: true,
                ..ImageClass::BluetoothHci.capabilities().unwrap()
            },
        ] {
            assert_eq!(crate::image::classify_flashed_capabilities(&foreign), None);
        }
    }

    #[test]
    fn every_feature_the_capabilities_read_is_the_runtime_s() {
        let graph = feature_graph(RUNTIME_MANIFEST);
        for feature in oer_hil_target_core::image_capabilities::READ_FEATURES {
            assert!(graph.contains_key(*feature), "{feature}");
        }
    }

    #[test]
    fn the_classes_report_what_their_features_promise() {
        let performance = ImageClass::Performance.capabilities().unwrap();
        assert!(performance.udp && !performance.driver_observation_evidence);
        let correctness = ImageClass::Correctness.capabilities().unwrap();
        assert!(correctness.driver_observation_evidence);
        let coex = ImageClass::WifiBleCoex.capabilities().unwrap();
        assert!(coex.bluetooth_gatt && coex.udp);
        let dtm = ImageClass::BluetoothHci.capabilities().unwrap();
        assert!(dtm.bluetooth_dtm && dtm.bluetooth_hci && !dtm.udp);
        assert!(
            ImageClass::SystemWatchdog
                .capabilities()
                .unwrap()
                .system_watchdog
        );
        assert!(
            ImageClass::DiagnosticMemoryBenchmark
                .capabilities()
                .unwrap()
                .memory_benchmark
        );
        assert!(ImageClass::DiagnosticTaskResidence.samples_program_counter());
        assert!(!ImageClass::Performance.samples_program_counter());
    }
}
