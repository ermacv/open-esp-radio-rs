//! Shared firmware publication for observations and build-only records.
use super::{
    build::{self, BuildFileMaterial, BuildSubject, BuildSubjectRole, SourceMaterial},
    run::{FirmwareArtifact, atomic_json},
};
use crate::{
    Result,
    image::{Artifacts, ImageClass},
};
use std::path::{Path, PathBuf};

pub(super) struct Context<'a> {
    pub directory: &'a Path,
    pub target_directory: &'a Path,
    pub source_root: &'a Path,
    pub source_materials: &'a [SourceMaterial],
    pub snapshot_materials: &'a [BuildFileMaterial],
}
pub(super) fn archive(
    context: Context<'_>,
    image: ImageClass,
    artifacts: &Artifacts,
) -> Result<(FirmwareArtifact, PathBuf)> {
    let selection = (image, artifacts.network);
    let application = &artifacts.application_image;
    let runtime_elf = &artifacts.runtime_elf;
    let runtime_bin = &artifacts.runtime_bin;
    let bootstrap_elf = &artifacts.bootstrap_elf;
    let effective_locks = (
        &artifacts.effective_embedded_lock,
        &artifacts.effective_bootstrap_lock,
    );
    let firmware_directory = PathBuf::from("firmware").join(image.id());
    let application_path = firmware_directory.join("application.bin");
    let archived_application = context.directory.join(&application_path);
    let runtime_elf_path = firmware_directory.join("runtime.elf");
    let runtime_bin_path = firmware_directory.join("runtime.bin");
    let bootstrap_elf_path = firmware_directory.join("bootstrap.elf");

    let application = build::archive_content_addressed(
        application,
        &archived_application,
        context.target_directory,
    )?;
    let runtime_elf = build::archive_content_addressed(
        runtime_elf,
        &context.directory.join(&runtime_elf_path),
        context.target_directory,
    )?;
    let runtime_bin = build::archive_content_addressed(
        runtime_bin,
        &context.directory.join(&runtime_bin_path),
        context.target_directory,
    )?;
    let bootstrap_elf = build::archive_content_addressed(
        bootstrap_elf,
        &context.directory.join(&bootstrap_elf_path),
        context.target_directory,
    )?;
    let mut locks = Vec::new();
    for (name, original, filename, source) in [
        (
            "embedded-lock",
            "hil/targets/esp32s31/Cargo.lock",
            "effective-Cargo.lock",
            effective_locks.0,
        ),
        (
            "bootstrap-lock",
            "platform/esp32s31/Cargo.lock",
            "bootstrap-Cargo.lock",
            effective_locks.1,
        ),
    ] {
        let path = firmware_directory.join(filename);
        let archived = build::archive_content_addressed(
            source,
            &context.directory.join(&path),
            context.target_directory,
        )?;
        locks.push(build::archived_file_material(
            name,
            Path::new(original),
            path,
            &archived,
        ));
    }
    let subjects = vec![
        BuildSubject {
            role: BuildSubjectRole::Application,
            path: application_path.clone(),
            size_bytes: application.size_bytes,
            sha256: application.sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::BootstrapElf,
            path: bootstrap_elf_path.clone(),
            size_bytes: bootstrap_elf.size_bytes,
            sha256: bootstrap_elf.sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::RuntimeBin,
            path: runtime_bin_path.clone(),
            size_bytes: runtime_bin.size_bytes,
            sha256: runtime_bin.sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::RuntimeElf,
            path: runtime_elf_path.clone(),
            size_bytes: runtime_elf.size_bytes,
            sha256: runtime_elf.sha256.clone(),
        },
    ];
    let build_id = build::build_id(&subjects);
    let build_provenance_path = firmware_directory.join("build-provenance.json");
    locks.extend(context.snapshot_materials.to_vec());
    let provenance = build::create_provenance(
        context.source_root,
        selection,
        build_id.clone(),
        context.source_materials.to_vec(),
        subjects,
        locks,
        artifacts.environment.clone(),
    )?;
    atomic_json(&context.directory.join(&build_provenance_path), &provenance)?;
    let artifact = FirmwareArtifact {
        image,
        replayed_from: None,
        build_id: Some(build_id),
        build_provenance_path: Some(build_provenance_path),
        application_path,
        application_size_bytes: application.size_bytes,
        application_sha256: application.sha256,
        runtime_elf_path: Some(runtime_elf_path),
        runtime_elf_size_bytes: Some(runtime_elf.size_bytes),
        runtime_elf_sha256: runtime_elf.sha256,
        runtime_bin_path: Some(runtime_bin_path),
        runtime_bin_size_bytes: Some(runtime_bin.size_bytes),
        runtime_bin_sha256: runtime_bin.sha256,
        bootstrap_elf_path: Some(bootstrap_elf_path),
        bootstrap_elf_size_bytes: Some(bootstrap_elf.size_bytes),
        bootstrap_elf_sha256: bootstrap_elf.sha256,
    };
    Ok((artifact, archived_application))
}
