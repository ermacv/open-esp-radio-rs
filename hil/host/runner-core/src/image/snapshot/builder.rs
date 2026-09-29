//! Compile a materialized snapshot using the existing image pipeline.

use super::*;

pub fn build(
    directory: &Path,
    class: crate::image::ImageClass,
    network: crate::image::Integration,
    layout_seed: crate::image::LayoutSeed,
    features: &crate::image::FeatureDelta,
) -> Result<crate::image::Artifacts> {
    FrozenSources::open_in_free_workspace(directory, &build_slots("esp32s31")?)?.build(
        class,
        network,
        layout_seed,
        features,
    )
}

/// The base of the host's build slots for `chip` images (`<base>-<n>`).
pub fn build_slots(chip: &str) -> Result<PathBuf> {
    Ok(crate::image::chip_build_root(chip)?.join("source-build"))
}

impl FrozenSources {
    /// The compile cache of `class` for this build: beside the slot, so the
    /// units compiled from this slot's paths stay with it.
    pub fn compile_cache(
        &self,
        class: crate::image::ImageClass,
        network: crate::image::Integration,
    ) -> PathBuf {
        let base = match &self.checkout {
            Checkout::Workspace { path, .. } => path.with_extension("cache"),
            Checkout::Temporary(directory) => directory.path().join("cache"),
        };
        base.join(format!(
            "{}-{}-{}",
            class.runtime_profile(),
            class.id(),
            network.id()
        ))
    }

    /// Build `class` for `chip` through the pipeline of its boot flow.
    pub fn build_for_chip(
        &self,
        chip: &str,
        class: crate::image::ImageClass,
        network: crate::image::Integration,
        layout_seed: crate::image::LayoutSeed,
        features: &crate::image::FeatureDelta,
    ) -> Result<crate::image::Artifacts> {
        let profile = crate::image::chip_profile(chip)?;
        if profile.boot == oer_chip_profile::Boot::Staged {
            return self.build(class, network, layout_seed, features);
        }
        if layout_seed.is_some() {
            return Err(format!("{chip} images have no layout seeds").into());
        }
        self.verify_unchanged()?;
        let output = crate::image::chip_build_root(chip)?
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
            &self.compile_cache(class, network),
        )?;
        self.verify_unchanged()?;
        atomic_json(&output.join("source-snapshot.json"), &self.snapshot)?;
        Ok(artifacts)
    }

    pub fn build(
        &self,
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
        let output = crate::image::chip_build_root("esp32s31")?
            .join("snapshot-builds")
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
                cache: &self.compile_cache(class, network),
                layout_seed,
                features,
            },
        )?;
        self.verify_unchanged()?;
        atomic_json(&output.join("source-snapshot.json"), &self.snapshot)?;
        Ok(artifacts)
    }
}
