//! Every ESP32-S31 build of esp-hal leaves interrupt routes to the image's
//! table: a package that enables esp-hal's `esp32s31` feature also enables
//! its `static-interrupts`, under which no esp-hal API binds a handler or
//! routes a source at run time
//! ([interrupt table](../../../../../crates/runtime/interrupt-table/README.md)).

use oer_repo::{Model, Package};

use crate::Result;

/// Fails naming every package that builds esp-hal for the ESP32-S31
/// without `static-interrupts`.
pub fn check(model: &Model) -> Result<()> {
    let problems: Vec<String> = model
        .packages()
        .iter()
        .filter(|package| enables(package, "esp32s31") && !enables(package, "static-interrupts"))
        .map(|package| {
            format!(
                "{}: esp-hal is built for esp32s31 without `static-interrupts`; enable it where \
                 `esp32s31` is enabled",
                package.manifest
            )
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n").into())
    }
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

    fn model(manifest: &str) -> (tempfile::TempDir, Model) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/Cargo.toml"), manifest).unwrap();
        let model = Model::load(&oer_repo::Repo::from_dir(dir.path()).unwrap()).unwrap();
        (dir, model)
    }

    #[test]
    fn an_s31_esp_hal_build_needs_static_interrupts() {
        for (manifest, passes) in [
            (
                "esp-hal = { version = \"1\", features = [\"esp32s31\"] }",
                false,
            ),
            (
                "esp-hal = { version = \"1\", optional = true }\n[features]\nx = [\"esp-hal?/esp32s31\"]",
                false,
            ),
            (
                "esp-hal = { version = \"1\", features = [\"esp32s31\", \"static-interrupts\"] }",
                true,
            ),
            (
                "esp-hal = { version = \"1\", features = [\"esp32s31\"] }\n[features]\ns = [\"esp-hal/static-interrupts\"]",
                true,
            ),
            (
                "esp-hal = { version = \"1\", features = [\"esp32c5\"] }",
                true,
            ),
        ] {
            let (_dir, model) = model(&format!(
                "[package]\nname = \"a\"\n[dependencies]\n{manifest}\n"
            ));
            assert_eq!(check(&model).is_ok(), passes, "{manifest}");
        }
    }
}
