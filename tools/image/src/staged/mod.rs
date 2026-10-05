//! The staged pipeline: the ROM loads the platform bootstrap, which embeds
//! the packed stage-two runtime and stages it into PSRAM (ESP32-S31).
//!
//! The runtime is compiled, its stacks gated and its placement audited; it
//! is flattened and packed with its checksum, embedded in the bootstrap, whose
//! own stack is gated; the bootstrap ELF is then encoded as the QIO
//! application of the chip's partition table, and the ROM-readable DIO
//! bootloader, the partition tables and the OTA selection of the first slot
//! are encoded beside it.

pub mod payload;
pub mod placement;

use std::path::Path;

use espflash::flasher::{FlashFrequency, FlashMode, FlashSize};

use crate::{
    ImageBundle, ImageSpec, Result,
    build_log::BuildLog,
    bundle::{Lock, Staged, files},
    encode::{self, Encoding},
    exclusion::BuildLock,
    source_inputs,
    stack::{self, StackPolicy},
};

pub use payload::pack_runtime;

/// The chip whose images boot staged.
pub const CHIP: &str = "esp32s31";

/// The application's flash settings; ESP-IDF enables QIO for it.
const APPLICATION: Encoding = Encoding {
    mode: Some(FlashMode::Qio),
    frequency: Some(FlashFrequency::_80Mhz),
    size: Some(FlashSize::_16Mb),
    mmu_page_size: Some(0x1_0000),
};
/// The ROM reads the bootloader in DIO.
const ROM: Encoding = Encoding {
    mode: Some(FlashMode::Dio),
    ..APPLICATION
};

/// The ROM summaries and pin the stack gate reads, besides the policy.
const STACK_INPUTS: [&str; 2] = [
    "verification/esp32s31/artifacts.toml",
    crate::interrupt_stack::ROM_SUMMARIES,
];

pub(crate) fn build(spec: &ImageSpec, profile: &oer_chip_profile::Profile) -> Result<ImageBundle> {
    let root = spec.root.as_path();
    let target = profile.rust_target.as_str();
    let bootstrap_package = profile
        .bootstrap_package()
        .ok_or("a staged chip names its bootstrap")?;
    let platform = profile.platform_workspace(root);
    let flash = profile.flash.clone().ok_or("no flash map")?;
    std::fs::create_dir_all(&spec.output)?;
    let mut bundle = ImageBundle::new(&spec.output, profile, flash);
    bundle.layout_seed = spec.layout_seed;
    let log = BuildLog::create(&spec.output.join(files::BUILD_LOG))?;
    let _cache = crate::exclusion::output_directory(&spec.cache)?;
    let policy = StackPolicy::load(&root.join(&spec.stack_policy))?;
    policy.stacks()?;
    // Private copies of both committed catalogs: patched networks and local
    // overrides resolve into them, never into the source tree.
    let workspace = root.join(&spec.application.workspace);
    let runtime_lock = BuildLock::prepare(&workspace, &spec.output.join("locks/runtime"))?;
    let bootstrap_lock = BuildLock::prepare(&platform, &spec.output.join("locks/bootstrap"))?;

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
        log.run(&mut runtime, "build the stage-two runtime")?;
    }
    let release = runtime_target.join(target).join("release");
    let runtime_elf = crate::snapshot(
        &release.join(&spec.application.binary),
        &spec.output,
        files::RUNTIME_ELF,
    )?;
    // Local overrides deliberately resolve path packages.
    if spec.overrides.is_empty() {
        runtime_lock.validate()?;
    }

    let stacks =
        stack::audit_runtime_stacks(root, &runtime_elf, &policy, spec.interrupts, &spec.output)
            .map_err(|error| log.failed("runtime stack gate", error))?;
    bundle
        .reports
        .extend([stack::INTERRUPT_REPORT, stack::RUNTIME_REPORT].map(str::to_owned));
    bundle.rom_summaries = stacks.summaries;
    bundle.warnings.extend(stacks.warnings);

    let runtime_bin = spec.output.join(files::RUNTIME_BIN);
    let mut objcopy = oer_toolchain::command(oer_toolchain::Tool::LlvmObjcopy)?;
    objcopy
        .args(["-O", "binary"])
        .arg(&runtime_elf)
        .arg(&runtime_bin);
    log.run(&mut objcopy, "flatten the stage-two runtime")?;
    let crc = pack_runtime(&runtime_bin).map_err(|error| log.failed("runtime packing", error))?;
    let placement = placement::audit_runtime(&runtime_elf, &runtime_bin)
        .and_then(|report| {
            if let Some(audit) = &spec.audit {
                audit(&oer_elf::Elf::parse(&std::fs::read(&runtime_elf)?)?)?;
            }
            Ok(report)
        })
        .map_err(|error| log.failed("runtime placement audit", error))?;
    eprintln!("runtime_crc32={crc:08x}");
    std::fs::write(spec.output.join(files::PLACEMENT_REPORT), placement)?;
    bundle.reports.push(files::PLACEMENT_REPORT.to_owned());

    // The bootstrap embeds the runtime with `include_bytes!`; a stable path
    // that keeps its timestamp while its bytes are unchanged leaves the
    // bootstrap fresh.
    let bootstrap_target = spec.cache.join("bootstrap");
    let embedded = std::path::absolute(bootstrap_target.join("stage-two-runtime.bin"))?;
    crate::replace_if_changed(&runtime_bin, &embedded)?;
    let mut bootstrap = crate::cargo::command();
    bootstrap
        .current_dir(root)
        .args(["build", "--manifest-path"])
        .arg(platform.join("Cargo.toml"))
        .args(["-p", &bootstrap_package, "--release", "--target", target])
        .env("CARGO_TARGET_DIR", &bootstrap_target)
        .env("CARGO_INCREMENTAL", "0")
        .env("PSRAM_RUNTIME_BIN", &embedded);
    if spec.overrides.esp_hal.is_none() {
        bootstrap.arg("--locked");
    }
    bootstrap_lock.configure(&mut bootstrap);
    spec.overrides.apply_esp_hal(&mut bootstrap);
    oer_toolchain::image::configure(
        &mut bootstrap,
        &policy.image_compiler(root, &spec.cache.join("image-linker"), target),
    )?;
    if spec.overrides.esp_hal.is_none() {
        crate::cargo::ensure_fetched(root, &platform.join("Cargo.toml"), |command| {
            bootstrap_lock.configure(command)
        })?;
    }
    log.run(&mut bootstrap, "build the Flash/SRAM bootstrap")?;
    let bootstrap_release = bootstrap_target.join(target).join("release");
    let bootstrap_elf = crate::snapshot(
        &bootstrap_release.join(&bootstrap_package),
        &spec.output,
        files::BOOTSTRAP_ELF,
    )?;
    let warnings = stack::audit_bootstrap_stack(root, &bootstrap_elf, &policy, &spec.output)
        .map_err(|error| log.failed("bootstrap stack gate", error))?;
    bundle.reports.push(stack::BOOTSTRAP_REPORT.to_owned());
    bundle.warnings.extend(warnings);

    let elf = std::fs::read(&bootstrap_elf)?;
    let application = encode_application(root, profile, &elf, &bundle)
        .map_err(|error| log.failed("encode the ESP application image", error))?;
    std::fs::write(bundle.application(), application)?;
    encode_boot_files(root, profile, &elf, &mut bundle)
        .map_err(|error| log.failed("encode the boot files", error))?;
    bundle.staged = Some(Staged {
        bootstrap_package: bootstrap_package.clone(),
    });

    for (lock, committed, file) in [
        (
            &runtime_lock,
            &spec.application.workspace,
            files::RUNTIME_LOCK,
        ),
        (
            &bootstrap_lock,
            &platform.strip_prefix(root)?.to_owned(),
            files::BOOTSTRAP_LOCK,
        ),
    ] {
        std::fs::copy(lock.path(), spec.output.join(file))?;
        bundle.locks.push(Lock {
            committed: committed.join("Cargo.lock"),
            file: file.to_owned(),
        });
    }

    let compiled = source_inputs::collect(
        root,
        &[
            (&release, spec.application.binary.as_str()),
            (&bootstrap_release, bootstrap_package.as_str()),
        ],
    )?;
    let map = bundle.flash.clone();
    let mut reads = vec![
        spec.stack_policy.clone(),
        Path::new("platform").join(&profile.id).join("chip.toml"),
        Path::new("platform").join(&profile.id).join("stack.toml"),
        Path::new("platform")
            .join(&profile.id)
            .join("stack-coverage.toml"),
    ];
    reads.extend(STACK_INPUTS.iter().map(std::path::PathBuf::from));
    reads.extend(map.partitions.iter().cloned());
    reads.extend(spec.reads.iter().cloned());
    let platform_relative = platform.strip_prefix(root)?.to_owned();
    let mut inputs = source_inputs::configuration(
        root,
        &compiled,
        &[spec.application.workspace.as_path(), &platform_relative],
        &reads
            .iter()
            .map(std::path::PathBuf::as_path)
            .collect::<Vec<_>>(),
    )?;
    inputs.extend(compiled);
    inputs.extend(builder_inputs(spec)?);
    source_inputs::write(&spec.output, &inputs)?;
    bundle.write()?;
    Ok(bundle)
}

/// The builder sources of `spec`: the pipeline's and its caller's closure.
pub(crate) fn builder_inputs(
    spec: &ImageSpec,
) -> Result<std::collections::BTreeSet<std::path::PathBuf>> {
    let roots: Vec<&str> = crate::PIPELINE
        .iter()
        .copied()
        .chain(spec.builders.iter().map(String::as_str))
        .collect();
    source_inputs::builder(&spec.root, &roots)
}

/// The QIO application image of the bootstrap ELF `elf` for the partition
/// at the flash map's application offset, audited against that partition.
fn encode_application(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: &[u8],
    bundle: &ImageBundle,
) -> Result<Vec<u8>> {
    let table = partitions(root, &bundle.flash)?;
    let partition = table
        .iter()
        .find(|partition| partition.application && partition.offset == bundle.flash.application)
        .ok_or_else(|| {
            format!(
                "the partition table has no application partition at {:#x}",
                bundle.flash.application
            )
        })?;
    let encoded = encode::encode(
        elf,
        encode::chip(&profile.espflash_chip)?,
        APPLICATION,
        Some(&root.join(table_path(&bundle.flash)?)),
        bundle.flash.partition_table,
        Some(&partition.name),
    )?;
    placement::audit_application_image(&encoded.application, partition.size)?;
    Ok(encoded.application)
}

/// Encode the staged boot's bootloader (in the ROM's DIO, from any ELF of
/// the application), the partition tables and the OTA selection of the
/// first slot into `bundle`.
pub(crate) fn encode_boot_files(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: &[u8],
    bundle: &mut ImageBundle,
) -> Result<()> {
    let flash = bundle.flash.clone();
    let table = partitions(root, &flash)?;
    let otadata = table
        .iter()
        .find(|partition| partition.otadata)
        .ok_or("the partition table has no OTA data partition")?;
    if flash.otadata != Some(otadata.offset) {
        return Err(format!(
            "the flash map puts the OTA data at {:?}, the partition table at {:#x}",
            flash.otadata, otadata.offset
        )
        .into());
    }
    let csv = root.join(table_path(&flash)?);
    let rom = encode::encode(
        elf,
        encode::chip(&profile.espflash_chip)?,
        ROM,
        Some(&csv),
        flash.partition_table,
        None,
    )?;
    std::fs::write(
        bundle.bootloader(),
        encode::rom_bootloader(&rom.bootloader)?,
    )?;
    std::fs::write(bundle.partitions(), &rom.partition_table)?;
    std::fs::write(bundle.path(files::OTADATA), encode::ota_selector_image(0))?;
    bundle.otadata = true;
    Ok(())
}

fn table_path(flash: &crate::FlashMap) -> Result<&Path> {
    Ok(flash
        .partitions
        .as_deref()
        .ok_or("a staged chip's flash map names its partition table")?)
}

/// The partitions of the flash map's partition table at `root`.
pub fn partitions(root: &Path, flash: &crate::FlashMap) -> Result<Vec<encode::Partition>> {
    encode::partitions(&root.join(table_path(flash)?))
}

#[cfg(test)]
mod tests;
