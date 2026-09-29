//! Build images from a frozen source snapshot.

use std::path::{Path, PathBuf};

use oer_hil_durable::atomic_json;
use oer_hil_image_class::{FeatureDelta, ImageClass};
use oer_hil_source_snapshot::FrozenSources;

use crate::{
    Result,
    image::{
        Artifacts, BuildPlacement, Integration, LayoutSeed, LocalOverrides, build_resolved,
        chip_build_root, chip_profile, esp_idf, seed_suffix,
    },
};

/// The base of the host's build slots for `chip` images (`<base>-<n>`).
pub fn build_slots(chip: &str) -> Result<PathBuf> {
    Ok(chip_build_root(chip)?.join("source-build"))
}

/// The compile cache of `class` for a build from `frozen`.
pub fn compile_cache(frozen: &FrozenSources, class: ImageClass, network: Integration) -> PathBuf {
    frozen.cache_base().join(format!(
        "{}-{}-{}",
        class.runtime_profile(),
        class.id(),
        network.id()
    ))
}

/// Build `class` for `chip` from `frozen` through the pipeline of its boot
/// flow.
pub fn build_for_chip(
    frozen: &FrozenSources,
    chip: &str,
    class: ImageClass,
    network: Integration,
    layout_seed: LayoutSeed,
    features: &FeatureDelta,
) -> Result<Artifacts> {
    let profile = chip_profile(chip)?;
    if profile.boot == oer_chip_profile::Boot::Staged {
        return build(frozen, class, network, layout_seed, features);
    }
    if layout_seed.is_some() {
        return Err(format!("{chip} images have no layout seeds").into());
    }
    frozen.verify_unchanged()?;
    let output = chip_build_root(chip)?
        .join("snapshot-builds")
        .join(frozen.snapshot().id())
        .join(format!("{}{}", class.id(), features.suffix()));
    let artifacts = esp_idf::build(
        &frozen.repository(),
        &profile,
        class,
        network,
        features,
        &output,
        &compile_cache(frozen, class, network),
    )?;
    finish(frozen, &output)?;
    Ok(artifacts)
}

/// Build the staged esp32s31 image `class` from `frozen`.
pub fn build(
    frozen: &FrozenSources,
    class: ImageClass,
    network: Integration,
    layout_seed: LayoutSeed,
    features: &FeatureDelta,
) -> Result<Artifacts> {
    frozen.verify_unchanged()?;
    let esp_hal = frozen.source_root("esp-hal");
    let embassy = frozen.source_root("embassy");
    let xarxa = frozen.source_root("xarxa");
    let output = chip_build_root("esp32s31")?
        .join("snapshot-builds")
        .join(frozen.snapshot().id())
        .join(format!(
            "{}-{}{}{}",
            class.id(),
            network.id(),
            seed_suffix(layout_seed),
            features.suffix()
        ));
    let artifacts = build_resolved(
        &frozen.repository(),
        class,
        network,
        LocalOverrides {
            esp_hal: esp_hal.as_deref(),
            embassy: embassy.as_deref(),
            xarxa: xarxa.as_deref(),
        },
        BuildPlacement {
            output: Some(&output),
            cache: &compile_cache(frozen, class, network),
            layout_seed,
            features,
        },
    )?;
    finish(frozen, &output)?;
    Ok(artifacts)
}

/// Confirm the checkout did not change during the build and record which
/// snapshot the output came from.
fn finish(frozen: &FrozenSources, output: &Path) -> Result<()> {
    frozen.verify_unchanged()?;
    atomic_json(&output.join("source-snapshot.json"), frozen.snapshot())?;
    Ok(())
}
