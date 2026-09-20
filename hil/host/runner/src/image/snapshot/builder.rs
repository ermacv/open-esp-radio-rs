//! Compile a materialized snapshot using the existing image pipeline.

use super::*;

pub(crate) fn build(
    root: &Path,
    directory: &Path,
    class: crate::image::ImageClass,
    network: crate::image::Integration,
) -> Result<crate::image::Artifacts> {
    FrozenSources::open(directory)?.build(root, class, network)
}

impl FrozenSources {
    pub(crate) fn build(
        &self,
        root: &Path,
        class: crate::image::ImageClass,
        network: crate::image::Integration,
    ) -> Result<crate::image::Artifacts> {
        self.verify_unchanged()?;
        let source = self.repository();
        let override_path = |role: &str| {
            self.manifest
                .sources
                .iter()
                .any(|s| s.name == role)
                .then(|| self.isolated.path().join(role))
        };
        let esp_hal = override_path("esp-hal");
        let embassy = override_path("embassy");
        let xarxa = override_path("xarxa");
        if network != crate::image::Integration::UpstreamXarxa
            && (esp_hal.is_some() || embassy.is_some() || xarxa.is_some())
        {
            return Err("local dependency overrides are supported only with upstream-xarxa".into());
        }
        let output = root
            .join("target/hil/esp32s31/snapshot-builds")
            .join(&self.snapshot.snapshot_id)
            .join(format!("{}-{}", class.id(), network.id()));
        let mut selection = oer_firmware::network::Selection::acquire(
            &source,
            &source.join("hil/targets/esp32s31"),
            network,
        )?;
        let has_overrides = esp_hal.is_some() || embassy.is_some() || xarxa.is_some();
        let mut runtime_lock = has_overrides
            .then(|| {
                crate::image::TrackedFileSnapshot::capture(
                    source.join("hil/targets/esp32s31/Cargo.lock"),
                )
            })
            .transpose()?;
        let mut bootstrap_lock = has_overrides
            .then(|| {
                crate::image::TrackedFileSnapshot::capture(
                    source.join("platform/esp32s31/Cargo.lock"),
                )
            })
            .transpose()?;
        let artifacts = crate::image::build_resolved(
            &source,
            class,
            network,
            crate::image::LocalOverrides {
                esp_hal: esp_hal.as_deref(),
                embassy: embassy.as_deref(),
                xarxa: xarxa.as_deref(),
            },
            Some(&output),
            false,
            false,
        )?;
        if let Some(lock) = &mut runtime_lock {
            lock.restore()?;
        }
        if let Some(lock) = &mut bootstrap_lock {
            lock.restore()?;
        }
        selection.validate()?;
        selection.restore()?;
        self.verify_unchanged()?;
        atomic_json(&output.join("source-snapshot.json"), &self.snapshot)?;
        Ok(artifacts)
    }
}
