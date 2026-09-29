//! What each image class's runtime compiles and reports, derived from its
//! chip's runtime manifest.
//!
//! A class names the Cargo features its runtime is built with; the manifest
//! says which features each of those enables in turn. The closure of that
//! graph is exactly what `cfg(feature = ...)` sees in the runtime, and the
//! runtime reports its capability keys with the same function of it
//! ([`oer_hil_image_keys::image_keys`]), so the host's
//! expectation of a flashed image cannot drift from the firmware. A flashed
//! image is the one class of its chip whose keys it reports.

use std::collections::{BTreeMap, BTreeSet};

use crate::session::DeviceCapabilities;

use super::{ImageClass, Integration};

/// The runtime manifests of this tree, by chip.
const RUNTIME_MANIFESTS: [(&str, &str); 2] = [
    (
        "esp32s31",
        include_str!("../../../../targets/esp32s31/runtime/Cargo.toml"),
    ),
    (
        "esp32c5",
        include_str!("../../../../targets/esp32c5/runtime/Cargo.toml"),
    ),
];

fn runtime_manifest(chip: &str) -> Option<&'static str> {
    RUNTIME_MANIFESTS
        .iter()
        .find_map(|(name, manifest)| (*name == chip).then_some(*manifest))
}

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
    /// Every Cargo feature this class's runtime on `chip` is built with, or
    /// `None` where the chip's runtime has no such class.
    pub fn enabled_features_on(self, chip: &str) -> Option<BTreeSet<String>> {
        let graph = feature_graph(runtime_manifest(chip)?);
        let features = self.build_features(Integration::OwnedXarxa);
        let roots: Vec<&str> = features
            .split(',')
            .filter(|root| !root.is_empty())
            .collect();
        // A class whose own features the chip's runtime lacks is not built
        // there; the network integration is the esp32s31 runtime's alone.
        let own = self.runtime_features();
        if !own.split(',').all(|feature| graph.contains_key(feature)) {
            return None;
        }
        let roots: Vec<&str> = roots
            .into_iter()
            .filter(|root| graph.contains_key(*root))
            .collect();
        Some(closure(&graph, &roots))
    }

    /// The capability keys this class's runtime on `chip` reports, or `None`
    /// for the boot smoke image, which reports none, and for a class the
    /// chip does not build.
    pub fn capabilities_on(self, chip: &str) -> Option<DeviceCapabilities> {
        if self == Self::BootSmoke {
            return None;
        }
        let enabled = self.enabled_features_on(chip)?;
        Some(DeviceCapabilities::of_keys(oer_hil_image_keys::image_keys(
            &|feature| enabled.contains(feature),
        )))
    }
}

/// The class of `chip` whose keys a flashed image reports.
pub fn classify_flashed(chip: &str, capabilities: &DeviceCapabilities) -> Option<ImageClass> {
    ImageClass::ALL.into_iter().find(|class| {
        class
            .capabilities_on(chip)
            .is_some_and(|expected| expected.same_keys(capabilities))
    })
}

#[cfg(test)]
mod tests {
    use oer_hil_protocol::{Message, bluetooth, network, system, telemetry, wifi};

    use super::*;

    fn keys(class: ImageClass) -> DeviceCapabilities {
        class.capabilities_on("esp32s31").unwrap()
    }

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
    fn every_class_of_a_chip_reports_its_own_keys_and_classifies_as_itself() {
        for (chip, _) in RUNTIME_MANIFESTS {
            let mut seen = BTreeMap::new();
            for class in ImageClass::ALL {
                let Some(capabilities) = class.capabilities_on(chip) else {
                    continue;
                };
                if let Some(other) = seen.insert(capabilities.keys().clone(), class) {
                    panic!(
                        "{chip}: {} and {} report the same keys",
                        other.id(),
                        class.id()
                    );
                }
                assert_eq!(
                    classify_flashed(chip, &capabilities),
                    Some(class),
                    "{chip}: {}",
                    class.id()
                );
            }
            assert!(!seen.is_empty(), "{chip} builds no class");
        }
    }

    #[test]
    fn a_key_set_no_class_builds_is_no_class() {
        let performance = keys(ImageClass::Performance);
        let mut foreign = performance.keys().clone();
        foreign.insert(<bluetooth::Dtm as Message>::KEY);
        assert_eq!(
            classify_flashed("esp32s31", &DeviceCapabilities::of_keys(foreign)),
            None
        );
        let mut missing = performance.keys().clone();
        missing.remove(&<network::Udp as Message>::KEY);
        assert_eq!(
            classify_flashed("esp32s31", &DeviceCapabilities::of_keys(missing)),
            None
        );
    }

    #[test]
    fn every_feature_the_keys_read_is_the_runtime_s() {
        let graph = feature_graph(runtime_manifest("esp32s31").unwrap());
        for feature in oer_hil_image_keys::READ_FEATURES {
            assert!(graph.contains_key(*feature), "{feature}");
        }
    }

    #[test]
    fn the_classes_report_what_their_features_promise() {
        let performance = keys(ImageClass::Performance);
        assert!(performance.has::<network::Udp>());
        assert!(!performance.has::<wifi::DriverObservation>());
        assert!(keys(ImageClass::Correctness).has::<wifi::DriverObservation>());
        let coex = keys(ImageClass::WifiBleCoex);
        assert!(coex.has::<bluetooth::Gatt>() && coex.has::<network::Udp>());
        let dtm = keys(ImageClass::BluetoothDtm);
        assert!(dtm.has::<bluetooth::Dtm>() && dtm.has::<bluetooth::Hci>());
        assert!(!dtm.has::<network::Udp>());
        let watchdog = keys(ImageClass::SystemWatchdog);
        assert!(watchdog.has::<system::WatchdogTest>());
        assert!(!watchdog.has::<network::Udp>());
        assert!(keys(ImageClass::DiagnosticMemoryBenchmark).has::<system::MemoryBenchmark>());
        assert!(keys(ImageClass::DiagnosticTaskResidence).has::<telemetry::PcProfile>());
        assert!(!performance.has::<telemetry::PcProfile>());
    }

    #[test]
    fn the_esp32c5_builds_its_system_watchdog_image() {
        let watchdog = ImageClass::SystemWatchdog
            .capabilities_on("esp32c5")
            .unwrap();
        assert!(watchdog.has::<system::WatchdogTest>());
        assert!(ImageClass::Performance.capabilities_on("esp32c5").is_none());
    }
}
