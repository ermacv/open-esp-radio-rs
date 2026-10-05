//! Shared firmware publication for observations and build-only records.
use super::Recipe;
use crate::{Artifacts, LayoutSeed, Result};
use oer_durable::atomic_json;
use oer_hil_image_class::ImageClass;
use oer_hil_image_class::NETWORK;
use oer_hil_run_bundle::{
    build::{self, BuildSubject, BuildSubjectRole},
    run::{FirmwareArchive, FirmwareArtifact, RunSession},
    verify::FirmwareRecipe as _,
};
use std::path::{Path, PathBuf};

/// Archive `artifacts` as the `image` firmware of `session` and bind it to
/// the run. Returns the bundle to flash: the build's flash files with the
/// archived application, below the run's target directory, so the run
/// writes exactly the bytes it archived.
pub fn record(
    session: &mut RunSession,
    image: ImageClass,
    artifacts: &Artifacts,
) -> Result<oer_image::ImageBundle> {
    let context = session.firmware_archive(image)?;
    let flash = context
        .target_directory
        .join("flash")
        .join(session.id())
        .join(image.id());
    let (artifact, application) = archive(&context, image, artifacts)?;
    let bundle = artifacts.bundle.with_application(&application, &flash)?;
    session.bind_firmware(artifact)?;
    Ok(bundle)
}

pub(super) fn archive(
    context: &FirmwareArchive<'_>,
    image: ImageClass,
    artifacts: &Artifacts,
) -> Result<(FirmwareArtifact, PathBuf)> {
    let bundle = &artifacts.bundle;
    let selection = (
        image,
        bundle.layout_seed,
        &artifacts.features,
        (
            bundle.chip.as_str(),
            bundle.rust_target.as_str(),
            bundle.boot,
        ),
    );
    let firmware_directory = PathBuf::from("firmware").join(image.id());
    let archive = |source: &Path, file: &str| -> Result<(PathBuf, build::ArchivedFile)> {
        let path = firmware_directory.join(file);
        let archived = build::archive_content_addressed(
            source,
            &context.directory.join(&path),
            context.target_directory,
        )?;
        Ok((path, archived))
    };
    let (application_path, application) = archive(&bundle.application(), "application.bin")?;
    let archived_application = context.directory.join(&application_path);
    let (runtime_elf_path, runtime_elf) = archive(&bundle.runtime_elf(), "runtime.elf")?;
    let embedded_lock = std::path::Path::new("hil/targets")
        .join(&bundle.chip)
        .join("Cargo.lock");
    let effective_embedded_lock = bundle
        .lock(&embedded_lock)
        .ok_or("the bundle has no effective lock of its agent workspace")?;
    let subject = |role, (path, archived): &(PathBuf, build::ArchivedFile)| BuildSubject {
        role,
        path: path.clone(),
        size_bytes: archived.size_bytes,
        sha256: archived.sha256.clone(),
    };
    // The workspace whose lock the runtime resolved against, and the boot
    // flow's own files: build record subjects go application, boot files,
    // runtime ELF.
    let mut locks = Vec::new();
    let mut lock = |name: &str, original: String, filename: &str, source: &Path| -> Result<()> {
        let (path, archived) = archive(source, filename)?;
        locks.push(build::archived_file_material(
            name,
            Path::new(&original),
            path,
            &archived,
        ));
        Ok(())
    };
    let mut subjects = vec![subject(
        BuildSubjectRole::Application,
        &(application_path.clone(), application.clone()),
    )];
    let mut artifact = FirmwareArtifact {
        image,
        replayed_from: None,
        build_id: None,
        build_provenance_path: None,
        application_path,
        application_size_bytes: application.size_bytes,
        application_sha256: application.sha256.clone(),
        runtime_elf_path: Some(runtime_elf_path.clone()),
        runtime_elf_size_bytes: Some(runtime_elf.size_bytes),
        runtime_elf_sha256: runtime_elf.sha256.clone(),
        boot: oer_hil_run_bundle::run::Boot::Staged,
        runtime_bin_path: None,
        runtime_bin_size_bytes: None,
        runtime_bin_sha256: None,
        bootstrap_elf_path: None,
        bootstrap_elf_size_bytes: None,
        bootstrap_elf_sha256: None,
        bootloader_path: None,
        bootloader_size_bytes: None,
        bootloader_sha256: None,
        partition_table_path: None,
        partition_table_size_bytes: None,
        partition_table_sha256: None,
        layout_seed: bundle.layout_seed,
    };
    match bundle.boot {
        oer_chip_profile::Boot::Staged => {
            let runtime_bin = archive(
                &bundle
                    .runtime_bin()
                    .ok_or("a staged bundle has its runtime")?,
                "runtime.bin",
            )?;
            let bootstrap_elf = archive(
                &bundle
                    .bootstrap_elf()
                    .ok_or("a staged bundle has its bootstrap")?,
                "bootstrap.elf",
            )?;
            lock(
                "embedded-lock",
                embedded_lock.display().to_string(),
                "effective-Cargo.lock",
                &effective_embedded_lock,
            )?;
            let bootstrap_lock = std::path::Path::new("platform")
                .join(&bundle.chip)
                .join("Cargo.lock");
            lock(
                "bootstrap-lock",
                bootstrap_lock.display().to_string(),
                "bootstrap-Cargo.lock",
                &bundle
                    .lock(&bootstrap_lock)
                    .ok_or("the bundle has no effective lock of its bootstrap")?,
            )?;
            subjects.push(subject(BuildSubjectRole::BootstrapElf, &bootstrap_elf));
            subjects.push(subject(BuildSubjectRole::RuntimeBin, &runtime_bin));
            artifact.runtime_bin_path = Some(runtime_bin.0);
            artifact.runtime_bin_size_bytes = Some(runtime_bin.1.size_bytes);
            artifact.runtime_bin_sha256 = Some(runtime_bin.1.sha256);
            artifact.bootstrap_elf_path = Some(bootstrap_elf.0);
            artifact.bootstrap_elf_size_bytes = Some(bootstrap_elf.1.size_bytes);
            artifact.bootstrap_elf_sha256 = Some(bootstrap_elf.1.sha256);
        }
        oer_chip_profile::Boot::EspIdfBootloader => {
            let bootloader = archive(&bundle.bootloader(), "bootloader.bin")?;
            let partition_table = archive(&bundle.partitions(), "partition-table.bin")?;
            lock(
                "embedded-lock",
                embedded_lock.display().to_string(),
                "effective-Cargo.lock",
                &effective_embedded_lock,
            )?;
            subjects.push(subject(BuildSubjectRole::Bootloader, &bootloader));
            subjects.push(subject(BuildSubjectRole::PartitionTable, &partition_table));
            artifact.boot = oer_hil_run_bundle::run::Boot::EspIdfBootloader;
            artifact.bootloader_path = Some(bootloader.0);
            artifact.bootloader_size_bytes = Some(bootloader.1.size_bytes);
            artifact.bootloader_sha256 = Some(bootloader.1.sha256);
            artifact.partition_table_path = Some(partition_table.0);
            artifact.partition_table_size_bytes = Some(partition_table.1.size_bytes);
            artifact.partition_table_sha256 = Some(partition_table.1.sha256);
        }
    }
    subjects.push(subject(
        BuildSubjectRole::RuntimeElf,
        &(runtime_elf_path, runtime_elf),
    ));
    archive(&bundle.source_inputs(), "source-inputs.json")?;
    let build_id = build::build_id(&subjects);
    let build_provenance_path = firmware_directory.join("build-provenance.json");
    locks.extend(context.snapshot_materials.to_vec());
    let provenance = create_provenance(
        &context.source_root,
        selection,
        build_id.clone(),
        context.source_materials.to_vec(),
        subjects,
        locks,
        artifacts.environment.clone(),
    )?;
    atomic_json(&context.directory.join(&build_provenance_path), &provenance)?;
    artifact.build_id = Some(build_id);
    artifact.build_provenance_path = Some(build_provenance_path);
    Ok((artifact, archived_application))
}

pub(super) fn create_provenance(
    root: &Path,
    selection: (
        ImageClass,
        LayoutSeed,
        &oer_hil_image_class::FeatureDelta,
        // The chip, its Rust target and how it boots.
        (&str, &str, oer_chip_profile::Boot),
    ),
    build_id: String,
    sources: Vec<build::SourceMaterial>,
    subjects: Vec<build::BuildSubject>,
    effective_locks: Vec<build::BuildFileMaterial>,
    environment: build::BuildEnvironment,
) -> Result<build::BuildProvenance> {
    let (image, layout_seed, features, (chip, rust_target, boot)) = selection;
    let files = match boot {
        oer_chip_profile::Boot::Staged => vec![
            ("workspace-lock", String::from("Cargo.lock")),
            (
                "embedded-workspace",
                String::from("hil/targets/esp32s31/Cargo.toml"),
            ),
            (
                "stack-policy",
                String::from("hil/targets/esp32s31/stack.toml"),
            ),
            // The HIL stack policy extends the production one.
            (
                "stack-policy-base",
                String::from("platform/esp32s31/stack.toml"),
            ),
            (
                "partition-table",
                String::from("platform/esp32s31/partitions/applications.csv"),
            ),
        ],
        // The chip profile fixes the flash layout the image is written with.
        oer_chip_profile::Boot::EspIdfBootloader => vec![
            ("workspace-lock", String::from("Cargo.lock")),
            (
                "embedded-workspace",
                format!("hil/targets/{chip}/Cargo.toml"),
            ),
            ("chip-profile", format!("platform/{chip}/chip.toml")),
        ],
    };
    let mut files = files
        .into_iter()
        .map(|(name, path)| build::build_file_material(root, name, Path::new(&path)))
        .collect::<Result<Vec<_>>>()?;
    files.extend(effective_locks);
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(build::BuildProvenance {
        schema: build::BUILD_PROVENANCE_SCHEMA,
        build_id,
        build_type: String::from("open-esp-radio-hil-firmware/v1"),
        parameters: build::BuildParameters {
            image,
            // An ESP-IDF application has no network integration.
            network: (boot == oer_chip_profile::Boot::Staged).then(|| NETWORK.to_owned()),
            runtime_profile: image.runtime_profile().to_owned(),
            target: rust_target.to_owned(),
            runtime_features: features.apply(&Recipe.runtime_features(
                image,
                (boot == oer_chip_profile::Boot::Staged).then_some(NETWORK),
            )?),
            layout_seed,
            features: features.clone(),
        },
        source_reconstructable: sources
            .iter()
            .all(|source| source.rebuild_status != build::SourceRebuildStatus::Incomplete),
        sources,
        files,
        environment,
        subjects,
        reproducibility: build::BuildReproducibility::Unverified,
    })
}
