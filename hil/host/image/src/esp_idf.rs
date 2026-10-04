//! Images of chips whose ESP-IDF second-stage bootloader loads the
//! application from its partition.
//!
//! The HIL agent of `hil/targets/<chip>` is built for the class's features and
//! encoded as an ESP application with `espflash save-image`. The chip's
//! catalog bootloader and its partition table come from the ESP-IDF build of
//! `hil/bootloaders/<chip>`, which only the `cargo hil` wrapper can run: the
//! runner asks it back through [`CLI_ENV`]. That build's
//! `flasher_args.json` must place them at the chip profile's offsets.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use oer_chip_profile::{FlashLayout, Profile};

use super::{
    Artifacts, BootArtifacts, BuildLog, EspIdfBoot, FeatureDelta, ImageClass, program_from_env,
    require_file,
};
use crate::Result;

/// Names the `cargo hil` wrapper's own executable, which builds a chip's
/// catalog bootloader for the runner.
pub const CLI_ENV: &str = "OER_HIL_CLI";

/// Whether `chip`'s HIL agent at `root` builds `class`: the agent declares
/// every feature the class selects.
pub fn serves(root: &Path, chip: &str, class: ImageClass) -> Result<bool> {
    let path = oer_chip_profile::Profile::load(root, chip)
        .map_err(|error| error.to_string())?
        .hil_agent_manifest(root);
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
    let declared = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} declares no features", path.display()))?;
    Ok(class
        .runtime_features()
        .split(',')
        .all(|feature| declared.contains_key(feature)))
}

/// Build `class` for `profile`'s chip from the sources at `source`, into
/// `output`, compiling in `cache`.
pub fn build(
    source: &Path,
    profile: &Profile,
    class: ImageClass,
    features: &FeatureDelta,
    output: &Path,
    cache: &Path,
) -> Result<Artifacts> {
    let flash = profile.flash.ok_or_else(|| {
        format!(
            "platform/{}/chip.toml names no flash layout for its HIL images",
            profile.id
        )
    })?;
    let workspace = source.join("hil/targets").join(&profile.id);
    let package = profile.hil_agent_package();
    fs::create_dir_all(output)?;
    fs::write(output.join("image-class.txt"), format!("{}\n", class.id()))?;
    let log = BuildLog::create(&output.join("build.log"))?;

    // These chips have no network integration: the class's own features.
    let runtime_features = features.apply(class.runtime_features());
    let mut runtime = super::cargo_command();
    runtime
        .current_dir(&workspace)
        .arg("build")
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .args(["-p", &package, "--release", "--locked"])
        .args(["--target", &profile.rust_target])
        .args(["--no-default-features", "--features", &runtime_features])
        .env("CARGO_TARGET_DIR", cache)
        .env("CARGO_INCREMENTAL", "0");
    super::ensure_fetched(&workspace, &workspace.join("Cargo.toml"), |_| {})?;
    log.run(&mut runtime, "build the HIL agent")?;
    let compiled = cache
        .join(&profile.rust_target)
        .join("release")
        .join(&package);
    require_file(&compiled, "runtime ELF")?;
    // Artifacts are copied out of the compile cache, which a later build of
    // the same class may overwrite.
    let runtime_elf = output.join("runtime.elf");
    fs::copy(&compiled, &runtime_elf)?;
    let effective_embedded_lock = output.join("effective-Cargo.lock");
    fs::copy(workspace.join("Cargo.lock"), &effective_embedded_lock)?;

    let application_image = output.join("application.bin");
    let mut save_image = Command::new(program_from_env("ESPFLASH", "espflash"));
    save_image
        .args(["save-image", "--chip", &profile.espflash_chip])
        .arg(&runtime_elf)
        .arg(&application_image);
    log.run(&mut save_image, "encode the ESP application image")?;

    let build = catalog_bootloader(&profile.id)?;
    let (bootloader, partition_table) = flash_files(&build, flash)?;
    let copied_bootloader = output.join("bootloader.bin");
    let copied_partition_table = output.join("partition-table.bin");
    fs::copy(&bootloader, &copied_bootloader)?;
    fs::copy(&partition_table, &copied_partition_table)?;

    Ok(Artifacts {
        chip: profile.id.clone(),
        rust_target: profile.rust_target.clone(),
        output: output.to_owned(),
        runtime_elf,
        effective_embedded_lock,
        application_image,
        boot: BootArtifacts::EspIdf(EspIdfBoot {
            espflash_chip: profile.espflash_chip.clone(),
            bootloader: copied_bootloader,
            partition_table: copied_partition_table,
            flash,
        }),
        source_inputs: None,
        environment: oer_hil_evidence::build::BuildEnvironment::capture(),
        layout_seed: None,
        features: features.clone(),
        rom_summaries: Default::default(),
    })
}

/// The ESP-IDF build directory of `chip`'s catalog bootloader, built by the
/// `cargo hil` wrapper this runner was started from.
fn catalog_bootloader(chip: &str) -> Result<PathBuf> {
    let cli = std::env::var_os(CLI_ENV).ok_or_else(|| {
        format!(
            "{chip} images need the chip's catalog bootloader, which only `cargo hil` builds; \
             run the runner through `cargo hil`"
        )
    })?;
    let output = oer_process::output(
        Command::new(cli).args(["firmware", "bootloader", chip]),
        Some(std::time::Duration::from_secs(1800)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "building the {chip} catalog bootloader failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let directory = String::from_utf8(output.stdout)?;
    let directory = directory
        .lines()
        .last()
        .ok_or("the bootloader build named no directory")?;
    Ok(PathBuf::from(directory.trim()))
}

/// The bootloader and partition table the ESP-IDF build at `build` flashes,
/// refused unless its `flasher_args.json` places them and the application
/// at `flash`'s offsets.
pub fn flash_files(build: &Path, flash: FlashLayout) -> Result<(PathBuf, PathBuf)> {
    let arguments: serde_json::Value =
        serde_json::from_slice(&fs::read(build.join("flasher_args.json"))?)?;
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

    const LAYOUT: FlashLayout = FlashLayout {
        bootloader: 0x2000,
        partition_table: 0x8000,
        application: 0x10000,
    };

    fn build_with(arguments: &str) -> tempfile::TempDir {
        let build = tempfile::tempdir().unwrap();
        fs::write(build.path().join("flasher_args.json"), arguments).unwrap();
        build
    }

    #[test]
    fn the_esp32c5_target_builds_the_boot_smoke_and_system_watchdog_images() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        assert!(serves(&root, "esp32c5", ImageClass::BootSmoke).unwrap());
        assert!(serves(&root, "esp32c5", ImageClass::SystemWatchdog).unwrap());
        assert!(!serves(&root, "esp32c5", ImageClass::Correctness).unwrap());
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
        let (bootloader, partition_table) = flash_files(build.path(), LAYOUT).unwrap();
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
        let error = flash_files(moved.path(), LAYOUT).unwrap_err().to_string();
        assert!(error.contains("bootloader at 0x0"), "{error}");
        assert!(flash_files(build_with("{}").path(), LAYOUT).is_err());
    }
}
