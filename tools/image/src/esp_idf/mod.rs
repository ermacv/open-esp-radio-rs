//! The ESP-IDF bootloader pipeline: the chip's catalog bootloader, built
//! against the pinned ESP-IDF, loads the application from its partition
//! (ESP32-C5).
//!
//! The application is compiled with the image compiler under the image's
//! stack policy and encoded as an ESP application image with the `espflash`
//! library. Its bootloader and partition table come from the ESP-IDF build
//! of `hil/bootloaders/<chip>` ([`catalog`]), whose `flasher_args.json`
//! must place them, and the application, at the chip's flash map offsets.
//!
//! The stack gate needs the chip's ROM ELF pinned in its `artifacts.toml`
//! and a stack policy that names the stacks; a policy that sets only the
//! move limit (the esp32c5's, which pins no ROM ELF yet) compiles with every
//! image flag, frame records included, and runs no stack gate.

pub mod catalog;
pub mod idf;

use std::path::{Path, PathBuf};

use crate::{
    FlashMap, ImageBundle, ImageSpec, Result,
    build_log::BuildLog,
    bundle::{Lock, files},
    encode::{self, Encoding},
    exclusion::BuildLock,
    source_inputs,
    stack::StackPolicy,
};

pub(crate) fn build(spec: &ImageSpec, profile: &oer_chip_profile::Profile) -> Result<ImageBundle> {
    let root = spec.root.as_path();
    let flash = profile.flash.clone().ok_or("no flash map")?;
    if spec.layout_seed.is_some() {
        return Err(format!("{} images have no layout seeds", profile.id).into());
    }
    std::fs::create_dir_all(&spec.output)?;
    let mut bundle = ImageBundle::new(&spec.output, profile, flash.clone());
    let log = BuildLog::create(&spec.output.join(files::BUILD_LOG))?;
    let _cache = crate::exclusion::output_directory(&spec.cache)?;
    let policy = StackPolicy::load(&root.join(&spec.stack_policy))?;
    if policy.stacks().is_ok() {
        return Err(format!(
            "{} has no stack gate: its stack policy may set only the move limit until the \
             chip pins its ROM ELF",
            profile.id
        )
        .into());
    }
    let workspace = root.join(&spec.application.workspace);
    let runtime_lock = BuildLock::prepare(&workspace, &spec.output.join("locks/runtime"))?;
    let runtime_target = spec.cache.join("runtime");
    let mut runtime = crate::runtime_command(spec, profile, "build", &policy)?;
    runtime.env("CARGO_TARGET_DIR", &runtime_target);
    runtime_lock.configure(&mut runtime);
    if spec.overrides.is_empty() {
        crate::cargo::ensure_fetched(root, &workspace.join("Cargo.toml"), |command| {
            runtime_lock.configure(command)
        })?;
    }
    {
        let _slot = crate::exclusion::slot(&crate::host_build_root()?.join("tokens"))?;
        log.run(&mut runtime, "build the application")?;
    }
    let release = runtime_target.join(&profile.rust_target).join("release");
    let runtime_elf = crate::snapshot(
        &release.join(&spec.application.binary),
        &spec.output,
        files::RUNTIME_ELF,
    )?;
    if spec.overrides.is_empty() {
        runtime_lock.validate()?;
    }
    let elf = std::fs::read(&runtime_elf)?;
    encode::require_app_descriptor(&elf).map_err(|error| log.failed("application audit", error))?;
    if let Some(audit) = &spec.audit {
        audit(&oer_elf::Elf::parse(&elf)?).map_err(|error| log.failed("runtime audit", error))?;
    }
    let encoded = encode::encode(
        &elf,
        encode::chip(&profile.espflash_chip)?,
        Encoding::DEFAULT,
        None,
        flash.partition_table,
        None,
    )
    .map_err(|error| log.failed("encode the ESP application image", error))?;
    std::fs::write(bundle.application(), &encoded.application)?;

    eprintln!("==> build the {} catalog bootloader", profile.id);
    let catalog_build = catalog::bootloader_build(root, &profile.id)
        .map_err(|error| log.failed("build the catalog bootloader", error))?;
    let (bootloader, partition_table) = flash_files(&catalog_build, &flash)?;
    std::fs::copy(bootloader, bundle.bootloader())?;
    std::fs::copy(partition_table, bundle.partitions())?;

    std::fs::copy(runtime_lock.path(), spec.output.join(files::RUNTIME_LOCK))?;
    bundle.locks.push(Lock {
        committed: spec.application.workspace.join("Cargo.lock"),
        file: files::RUNTIME_LOCK.to_owned(),
    });

    let compiled = source_inputs::collect(root, &[(&release, spec.application.binary.as_str())])?;
    let bootloader_project = Path::new("hil/bootloaders").join(&profile.id);
    let mut reads: Vec<PathBuf> = vec![
        spec.stack_policy.clone(),
        Path::new("platform").join(&profile.id).join("chip.toml"),
        Path::new("verification")
            .join(&profile.id)
            .join("artifacts.toml"),
    ];
    let repo = oer_repo::Repo::load(root)?;
    reads.extend(
        repo.below(&bootloader_project.to_string_lossy())
            .map(PathBuf::from),
    );
    reads.extend(spec.reads.iter().cloned());
    let mut inputs = source_inputs::configuration(
        root,
        &compiled,
        &[spec.application.workspace.as_path()],
        &reads.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
    )?;
    inputs.extend(compiled);
    inputs.extend(crate::staged::builder_inputs(spec)?);
    source_inputs::write(&spec.output, &inputs)?;
    bundle.write()?;
    Ok(bundle)
}

/// The bootloader and partition table the ESP-IDF build at `build` flashes,
/// refused unless its `flasher_args.json` places them and the application
/// at `flash`'s offsets.
pub fn flash_files(build: &Path, flash: &FlashMap) -> Result<(PathBuf, PathBuf)> {
    let arguments: serde_json::Value =
        serde_json::from_slice(&std::fs::read(build.join("flasher_args.json"))?)?;
    let entry = |name: &str, expected: u32| -> Result<PathBuf> {
        let offset = arguments[name]["offset"]
            .as_str()
            .and_then(|offset| u32::from_str_radix(offset.trim_start_matches("0x"), 16).ok())
            .ok_or_else(|| format!("flasher_args.json names no {name} offset"))?;
        if offset != expected {
            return Err(format!(
                "the catalog build places the {name} at {offset:#x}, the chip profile at \
                 {expected:#x}"
            )
            .into());
        }
        Ok(build.join(
            arguments[name]["file"]
                .as_str()
                .ok_or_else(|| format!("flasher_args.json names no {name} file"))?,
        ))
    };
    entry("app", flash.application)?;
    Ok((
        entry("bootloader", flash.bootloader)?,
        entry("partition-table", flash.partition_table)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> FlashMap {
        FlashMap {
            bootloader: 0x2000,
            partition_table: 0x8000,
            application: 0x10000,
            otadata: None,
            partitions: None,
            start: oer_chip_profile::Start::PowerOn,
        }
    }

    fn build_with(arguments: &str) -> tempfile::TempDir {
        let build = tempfile::tempdir().unwrap();
        std::fs::write(build.path().join("flasher_args.json"), arguments).unwrap();
        build
    }

    #[test]
    fn the_catalog_build_must_place_its_files_where_the_profile_says() {
        let build = build_with(
            r#"{
                "bootloader": { "offset": "0x2000", "file": "bootloader/bootloader.bin" },
                "partition-table": { "offset": "0x8000", "file": "partition_table/partition-table.bin" },
                "app": { "offset": "0x10000", "file": "app.bin" }
            }"#,
        );
        let (bootloader, partition_table) = flash_files(build.path(), &layout()).unwrap();
        assert_eq!(bootloader, build.path().join("bootloader/bootloader.bin"));
        assert_eq!(
            partition_table,
            build.path().join("partition_table/partition-table.bin")
        );

        let moved = build_with(
            r#"{
                "bootloader": { "offset": "0x0", "file": "bootloader/bootloader.bin" },
                "partition-table": { "offset": "0x8000", "file": "p.bin" },
                "app": { "offset": "0x10000", "file": "app.bin" }
            }"#,
        );
        let error = flash_files(moved.path(), &layout())
            .unwrap_err()
            .to_string();
        assert!(error.contains("bootloader at 0x0"), "{error}");
        assert!(flash_files(build_with("{}").path(), &layout()).is_err());
    }

    #[test]
    fn the_esp32c5_policy_sets_only_the_move_limit() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let policy = StackPolicy::load(&root.join("hil/targets/esp32c5/stack.toml")).unwrap();
        assert!(policy.stacks().is_err());
        assert!(policy.max_move_bytes > 0);
    }
}
