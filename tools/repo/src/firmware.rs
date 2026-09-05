//! Complete application images using the platform boot contract.
mod monitor;
mod workspace;

pub use workspace::FirmwareBuild;

use crate::{Context, Result, process};
use oer_firmware::{BOOTSTRAP_BIN, TARGET};
use std::{env, fs, path::Path};

pub fn build(
    ctx: &Context,
    example: &str,
    features: &[String],
    no_default_features: bool,
) -> Result<FirmwareBuild> {
    let directory = ctx
        .root
        .join("examples")
        .join(format!("esp32s31-{example}"));
    let manifest = directory.join("Cargo.toml");
    let contents = fs::read_to_string(&manifest)?;
    let data: toml::Table = toml::from_str(&contents)?;
    let binary = data
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or("example has no package name")?;
    let directory_output = ctx
        .root
        .join("target/firmware")
        .join(format!("esp32s31-{example}"));
    let workspace = workspace::Workspace::acquire(&directory_output)?;
    let output = workspace.output();
    let budget = open_esp_radio_memory_report::StackBudget::load(
        &ctx.root.join("platform/esp32s31/stack.toml"),
    )?;
    let runtime_target = workspace.cache().join("runtime");
    let mut command = ctx.cargo();
    command
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            TARGET,
            "--manifest-path",
        ])
        .arg(&manifest)
        .args(["--bin", binary])
        .env("CARGO_TARGET_DIR", &runtime_target)
        .env("CARGO_INCREMENTAL", "0");
    if no_default_features {
        command.arg("--no-default-features");
    }
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    oer_firmware::stack::enable_stack_checks(&mut command, &budget);
    process::run(&mut command)?;
    let runtime = workspace.snapshot(
        &runtime_target.join(TARGET).join("release").join(binary),
        "runtime.elf",
    )?;
    audit_stack(&runtime, &output.join("runtime-stack.txt"), &budget)?;
    let packed = output.join("runtime.bin");
    process::run(
        ctx.command(env::var_os("LLVM_OBJCOPY").unwrap_or_else(|| "llvm-objcopy".into()))
            .args(["-O", "binary"])
            .arg(&runtime)
            .arg(&packed),
    )?;
    oer_firmware::pack_runtime(&packed)?;
    fs::write(
        output.join("placement.txt"),
        oer_firmware::audit_runtime(&runtime, &packed, true)?,
    )?;
    let bootstrap_target = workspace.cache().join("bootstrap");
    let mut command = ctx.cargo();
    oer_firmware::bootstrap_command(&mut command, &ctx.root, &packed, &bootstrap_target);
    command.arg("--locked");
    oer_firmware::stack::enable_stack_checks(&mut command, &budget);
    process::run(&mut command)?;
    let bootstrap = workspace.snapshot(
        &bootstrap_target
            .join(TARGET)
            .join("release")
            .join(BOOTSTRAP_BIN),
        "bootstrap.elf",
    )?;
    audit_stack(&bootstrap, &output.join("bootstrap-stack.txt"), &budget)?;
    let image = output.join("application.bin");
    let mut command = ctx.command(env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()));
    oer_firmware::save_image_command(&mut command, &ctx.root, &bootstrap, &image);
    process::run(&mut command)?;
    oer_firmware::audit_application_image(&image)?;
    let rom_container = output.join("rom-container.bin");
    let mut command = ctx.command(env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()));
    oer_firmware::save_rom_image_command(&mut command, &ctx.root, &bootstrap, &rom_container);
    process::run(&mut command)?;
    let container = fs::read(&rom_container)?;
    fs::write(
        output.join("bootloader.bin"),
        oer_firmware::flash::rom_bootloader(&container)?,
    )?;
    fs::remove_file(rom_container)?;
    let mut command = ctx.command(env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()));
    command
        .args(["partition-table", "--to-binary", "--output"])
        .arg(output.join("partitions.bin"))
        .arg(
            ctx.root
                .join("platform/esp32s31/partitions/applications.csv"),
        );
    process::run(&mut command)?;
    fs::write(
        output.join("otadata.bin"),
        oer_firmware::flash::ota0_selector_image(),
    )?;
    fs::copy(
        directory.join("Cargo.lock"),
        output.join("runtime-Cargo.lock"),
    )?;
    fs::copy(
        ctx.root.join("platform/esp32s31/Cargo.lock"),
        output.join("bootstrap-Cargo.lock"),
    )?;
    println!("application image: {}", image.display());
    println!("bootstrap ELF: {}", bootstrap.display());
    Ok(workspace.finish())
}

fn audit_stack(
    elf: &Path,
    output: &Path,
    budget: &open_esp_radio_memory_report::StackBudget,
) -> Result<()> {
    let report = oer_firmware::stack::analyze_elf_stack(elf, budget)?;
    fs::write(
        output,
        open_esp_radio_memory_report::render_stack_report(&report),
    )?;
    open_esp_radio_memory_report::audit_stack(&report)?;
    Ok(())
}

/// Flash the exact audited images and select ota_0 without erasing other partitions.
pub fn flash(
    ctx: &Context,
    build: &FirmwareBuild,
    port: Option<&Path>,
    monitor: bool,
) -> Result<()> {
    use oer_firmware::flash::{
        BOOTLOADER_OFFSET, OTA_0_OFFSET, OTA_SELECTOR_OFFSET, PARTITION_TABLE_OFFSET,
    };
    let output = build.directory();
    let lease = oer_firmware::device::DeviceLease::select(port)?;
    let port = Some(lease.port());
    for (address, filename, reset) in [
        (BOOTLOADER_OFFSET, "bootloader.bin", "no-reset"),
        (PARTITION_TABLE_OFFSET, "partitions.bin", "no-reset"),
        (OTA_0_OFFSET, "application.bin", "no-reset"),
        (OTA_SELECTOR_OFFSET, "otadata.bin", "hard-reset"),
    ] {
        let mut command = ctx.command(env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()));
        oer_firmware::flash::write_bin_command(
            &mut command,
            port,
            address,
            &output.join(filename),
            reset,
        );
        process::run(&mut command)?;
    }
    if monitor {
        monitor::run(port.ok_or("--monitor requires --port")?)?;
    }
    Ok(())
}
