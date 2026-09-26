//! Compile a materialized snapshot using the existing image pipeline.

use super::*;

pub fn build(
    root: &Path,
    directory: &Path,
    class: crate::image::ImageClass,
    network: crate::image::Integration,
) -> Result<crate::image::Artifacts> {
    FrozenSources::open_in_workspace(directory, &root.join("target/hil/esp32s31/source-build"))?
        .build(root, class, network)
}

impl FrozenSources {
    pub fn build(
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
                .then(|| self.checkout.path().join(role))
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
                cache: crate::image::CompileCache::Shared(&crate::image::shared_compile_cache(
                    root, class, network,
                )),
            },
            false,
            false,
        )?;
        self.verify_unchanged()?;
        atomic_json(&output.join("source-snapshot.json"), &self.snapshot)?;
        Ok(artifacts)
    }
}
