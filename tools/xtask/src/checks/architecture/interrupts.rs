//! Every build of esp-hal for a chip whose profile sets `[gate]
//! static-interrupts` leaves interrupt routes to the image's table: a
//! package that enables esp-hal's feature of that chip also enables its
//! `static-interrupts`, under which no esp-hal API binds a handler or routes
//! a source at run time
//! ([interrupt table](../../../../../crates/runtime/interrupt-table/README.md)).

use oer_repo::{Model, Package};

use crate::Result;

/// Fails naming every package that builds esp-hal for such a chip without
/// `static-interrupts`.
pub fn check(model: &Model, chips: &oer_repo::chips::Chips) -> Result<()> {
    let mut problems = Vec::new();
    for chip in chips
        .profiles()
        .iter()
        .filter(|profile| profile.gate.static_interrupts)
    {
        let chip = chip.id.as_str();
        problems.extend(
            model
                .packages()
                .iter()
                .filter(|package| enables(package, chip) && !enables(package, "static-interrupts"))
                .map(|package| {
                    format!(
                        "{}: esp-hal is built for {chip} without `static-interrupts`; enable it \
                         where `{chip}` is enabled",
                        package.manifest
                    )
                }),
        );
    }
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

    /// A repository with one package `manifest` and two chips, `chip-a`
    /// with static interrupts and `chip-b` without.
    fn model(manifest: &str) -> (tempfile::TempDir, Model, oer_repo::chips::Chips) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/Cargo.toml"), manifest).unwrap();
        for (chip, gate) in [
            ("chip-a", "[gate]\nstatic-interrupts = true\n"),
            ("chip-b", ""),
        ] {
            let platform = dir.path().join("platform").join(chip);
            std::fs::create_dir_all(&platform).unwrap();
            std::fs::write(
                platform.join("chip.toml"),
                format!(
                    "schema = 1\nid = \"{chip}\"\nfamily = \"f\"\nrust-target = \"t\"\n\
                     boot = \"staged\"\nespflash-chip = \"{chip}\"\nrevisions = []\n\
                     [properties]\nwifi-bands = []\nbluetooth = []\nieee802154 = false\n\
                     cores = 1\n{gate}"
                ),
            )
            .unwrap();
        }
        let repo = oer_repo::Repo::from_dir(dir.path()).unwrap();
        let model = Model::load(&repo).unwrap();
        let chips = oer_repo::chips::Chips::at(dir.path()).unwrap();
        (dir, model, chips)
    }

    #[test]
    fn a_static_interrupt_chip_s_esp_hal_build_needs_static_interrupts() {
        for (manifest, passes) in [
            (
                "esp-hal = { version = \"1\", features = [\"chip-a\"] }",
                false,
            ),
            (
                "esp-hal = { version = \"1\", optional = true }\n[features]\nx = [\"esp-hal?/chip-a\"]",
                false,
            ),
            (
                "esp-hal = { version = \"1\", features = [\"chip-a\", \"static-interrupts\"] }",
                true,
            ),
            (
                "esp-hal = { version = \"1\", features = [\"chip-a\"] }\n[features]\ns = [\"esp-hal/static-interrupts\"]",
                true,
            ),
            (
                "esp-hal = { version = \"1\", features = [\"chip-b\"] }",
                true,
            ),
        ] {
            let (_dir, model, chips) = model(&format!(
                "[package]\nname = \"a\"\n[dependencies]\n{manifest}\n"
            ));
            assert_eq!(check(&model, &chips).is_ok(), passes, "{manifest}");
        }
    }
}
