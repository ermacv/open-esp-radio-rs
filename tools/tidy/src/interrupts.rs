//! Every ESP32-S31 build of esp-hal leaves interrupt routes to the image's
//! table: a package that enables esp-hal's `esp32s31` feature also enables
//! its `static-interrupts`, under which no esp-hal API binds a handler or
//! routes a source at run time
//! ([interrupt table](../../../crates/runtime/interrupt-table/README.md)).

use crate::{Context, manifest::Package};

/// Packages that build esp-hal for the ESP32-S31 without
/// `static-interrupts`.
pub fn check(context: &Context<'_>) -> Vec<String> {
    context
        .manifests
        .packages
        .iter()
        .filter(|package| enables(package, "esp32s31") && !enables(package, "static-interrupts"))
        .map(|package| {
            format!(
                "{}: esp-hal is built for esp32s31 without `static-interrupts`; enable it where \
                 `esp32s31` is enabled",
                package.manifest
            )
        })
        .collect()
}

/// Whether `package` enables esp-hal's `feature`, in a dependency spec or
/// through its own `[features]`.
fn enables(package: &Package, feature: &str) -> bool {
    let forwarded = |entry: &String| {
        entry
            .strip_prefix("esp-hal")
            .map(|rest| rest.trim_start_matches('?'))
            .and_then(|rest| rest.strip_prefix('/'))
            == Some(feature)
    };
    package
        .dependencies
        .iter()
        .filter(|dependency| dependency.key == "esp-hal")
        .any(|dependency| dependency.features.iter().any(|enabled| enabled == feature))
        || package.feature_entries.iter().any(forwarded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Dependency;

    fn package(features: &[&str], entries: &[&str]) -> Package {
        Package {
            manifest: "a/Cargo.toml".into(),
            directory: "a".into(),
            name: "a".into(),
            library: "a".into(),
            workspace: None,
            roots: vec![],
            missing_roots: vec![],
            dependencies: vec![Dependency {
                key: "esp-hal".into(),
                table: "dependencies".into(),
                path: None,
                features: features.iter().map(|&f| f.into()).collect(),
            }],
            feature_forwarded: vec![],
            feature_entries: entries.iter().map(|&f| f.into()).collect(),
            open_radio: None,
        }
    }

    #[test]
    fn an_s31_esp_hal_build_needs_static_interrupts() {
        assert!(enables(&package(&["esp32s31"], &[]), "esp32s31"));
        assert!(!enables(&package(&["esp32s31"], &[]), "static-interrupts"));
        assert!(enables(&package(&[], &["esp-hal?/esp32s31"]), "esp32s31"));
        assert!(enables(
            &package(&["static-interrupts"], &[]),
            "static-interrupts"
        ));
        assert!(enables(
            &package(&[], &["esp-hal/static-interrupts"]),
            "static-interrupts"
        ));
        assert!(!enables(&package(&["esp32c5"], &[]), "esp32s31"));
    }
}
