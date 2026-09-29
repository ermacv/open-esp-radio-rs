//! Compile a materialized snapshot using the existing image pipeline.

use super::*;

pub fn build(
    root: &Path,
    directory: &Path,
    class: crate::image::ImageClass,
    network: crate::image::Integration,
    layout_seed: crate::image::LayoutSeed,
    features: &crate::image::FeatureDelta,
) -> Result<crate::image::Artifacts> {
    FrozenSources::open_in_free_workspace(
        directory,
        &root.join("target/hil/esp32s31/source-build"),
    )?
    .build(root, class, network, layout_seed, features)
}

impl FrozenSources {
    /// Build `class` for `chip` through the pipeline of its boot flow.
    pub fn build_for_chip(
        &self,
        root: &Path,
        chip: &str,
        class: crate::image::ImageClass,
        network: crate::image::Integration,
        layout_seed: crate::image::LayoutSeed,
        features: &crate::image::FeatureDelta,
    ) -> Result<crate::image::Artifacts> {
        let profile = crate::image::chip_profile(chip)?;
        if profile.boot == oer_chip_profile::Boot::Staged {
            return self.build(root, class, network, layout_seed, features);
        }
        if layout_seed.is_some() {
            return Err(format!("{chip} images have no layout seeds").into());
        }
        self.verify_unchanged()?;
        let hil = root.join("target/hil").join(chip);
        let output = hil
            .join("snapshot-builds")
            .join(&self.snapshot.snapshot_id)
            .join(format!("{}{}", class.id(), features.suffix()));
        let artifacts = crate::image::esp_idf::build(
            &self.repository(),
            &profile,
            class,
            network,
            features,
            &output,
            &hil.join("build-cache").join(class.id()),
        )?;
        self.verify_unchanged()?;
        atomic_json(&output.join("source-snapshot.json"), &self.snapshot)?;
        Ok(artifacts)
    }

    pub fn build(
        &self,
        root: &Path,
        class: crate::image::ImageClass,
        network: crate::image::Integration,
        layout_seed: crate::image::LayoutSeed,
        features: &crate::image::FeatureDelta,
    ) -> Result<crate::image::Artifacts> {
        self.verify_unchanged()?;
        let source = self.repository();
        let override_path = |role: &str| {
            self.manifest
                .sources
                .iter()
                .any(|s| s.name == role)
                .then(|| self.checkout.path().join(role))
        };
        let esp_hal = override_path("esp-hal");
        let embassy = override_path("embassy");
        let xarxa = override_path("xarxa");
        let output = root
            .join("target/hil/esp32s31/snapshot-builds")
            .join(&self.snapshot.snapshot_id)
            .join(format!(
                "{}-{}{}{}",
                class.id(),
                network.id(),
                crate::image::seed_suffix(layout_seed),
                features.suffix()
            ));
        let artifacts = crate::image::build_resolved(
            &source,
            class,
            network,
            crate::image::LocalOverrides {
                esp_hal: esp_hal.as_deref(),
                embassy: embassy.as_deref(),
                xarxa: xarxa.as_deref(),
            },
            crate::image::BuildPlacement {
                output: Some(&output),
                cache: &crate::image::shared_compile_cache(root, class, network),
                layout_seed,
                features,
            },
        )?;
        self.verify_unchanged()?;
        atomic_json(&output.join("source-snapshot.json"), &self.snapshot)?;
        Ok(artifacts)
    }
}
