//! Complete application images using the platform boot contract.
mod monitor;
mod workspace;

pub use workspace::FirmwareBuild;

use crate::{Context, Result};
use oer_esp32s31_firmware::{BOOTSTRAP_BIN, TARGET};
use oer_process as process;
use std::{env, fs, path::Path};

pub fn build(
    ctx: &Context,
    example: &str,
    features: &[String],
    no_default_features: bool,
) -> Result<FirmwareBuild> {
    let directory = ctx.root.join("examples/esp32s31").join(example);
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
    let budget =
        oer_memory_report::StackBudget::load(&ctx.root.join("platform/esp32s31/stack.toml"))?;
    let runtime_target = workspace.cache().join("runtime");
    // A patched dependency resolves into this private copy, never the
    // example's catalog. The examples share one workspace and its lockfile.
    let runtime_lock = oer_esp32s31_firmware::network::BuildLock::prepare(
        &ctx.root.join("examples/esp32s31"),
        &workspace.cache().join("lock"),
    )?;
    let mut command = ctx.cargo();
    command
        .args(["build", "--release", "--target", TARGET, "--manifest-path"])
        .arg(&manifest)
        .args(["--bin", binary])
        .env("CARGO_TARGET_DIR", &runtime_target)
        .env("CARGO_INCREMENTAL", "0");
    command.arg("--locked");
    runtime_lock.configure(&mut command);
    if no_default_features {
        command.arg("--no-default-features");
    }
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    oer_esp32s31_firmware::compiler::configure_image_compiler(&mut command, &budget);
    process::run(&mut command)?;
    runtime_lock.validate(&ctx.root, Default::default())?;
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
    oer_esp32s31_firmware::pack_runtime(&packed)?;
    fs::write(
        output.join("placement.txt"),
        oer_esp32s31_firmware::audit_runtime(&runtime, &packed)?,
    )?;
    let bootstrap_target = workspace.cache().join("bootstrap");
    let mut command = ctx.cargo();
    oer_esp32s31_firmware::bootstrap_command(&mut command, &ctx.root, &packed, &bootstrap_target);
    command.arg("--locked");
    oer_esp32s31_firmware::compiler::configure_image_compiler(&mut command, &budget);
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
    oer_esp32s31_firmware::save_image_command(&mut command, &ctx.root, &bootstrap, &image);
    process::run(&mut command)?;
    oer_esp32s31_firmware::audit_application_image(&image)?;
    let rom_container = output.join("rom-container.bin");
    let mut command = ctx.command(env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()));
    oer_esp32s31_firmware::save_rom_image_command(
        &mut command,
        &ctx.root,
        &bootstrap,
        &rom_container,
    );
    process::run(&mut command)?;
    let container = fs::read(&rom_container)?;
    fs::write(
        output.join("bootloader.bin"),
        oer_esp32s31_firmware::flash::rom_bootloader(&container)?,
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
        oer_esp32s31_firmware::flash::ota0_selector_image(),
    )?;
    fs::copy(runtime_lock.path(), output.join("runtime-Cargo.lock"))?;
    fs::copy(
        ctx.root.join("platform/esp32s31/Cargo.lock"),
        output.join("bootstrap-Cargo.lock"),
    )?;
    println!("application image: {}", image.display());
    println!("bootstrap ELF: {}", bootstrap.display());
    Ok(workspace.finish())
}

fn audit_stack(elf: &Path, output: &Path, budget: &oer_memory_report::StackBudget) -> Result<()> {
    let report = oer_esp32s31_firmware::stack::analyze_elf_stack(elf, budget)?;
    fs::write(output, oer_memory_report::render_stack_report(&report))?;
    oer_memory_report::audit_stack(&report)?;
    Ok(())
}

/// Flash the exact audited images and select ota_0 without erasing other partitions.
///
/// The HIL stand is leased for the flash and the optional monitor; the lease
/// has no watchdog, because an interactive monitor has no budget.
pub fn flash(
    build: &FirmwareBuild,
    example: &str,
    port: Option<&Path>,
    monitor: bool,
) -> Result<()> {
    use oer_esp32s31_firmware::flash::{
        AfterFlash, BOOTLOADER_OFFSET, FlashSegment, OTA_0_OFFSET, OTA_SELECTOR_OFFSET,
        PARTITION_TABLE_OFFSET, write_segments,
    };
    use sha2::Digest as _;
    let output = build.directory();
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let registered = match port {
        Some(_) => None,
        None => only_attached_board("esp32s31", &arbiter.devices()?),
    };
    let port = port.or(registered.as_deref());
    let board = port
        .map(|port| oer_hil_arbiter::port_mac(port).unwrap_or_else(|| port.display().to_string()));
    let mut request = oer_hil_arbiter::Request::from_environment(format!(
        "build firmware {example} --flash{}",
        if monitor { " --monitor" } else { "" }
    ))?;
    request.claims = match &board {
        Some(board) => vec![
            oer_hil_arbiter::Claim::board(board),
            oer_hil_arbiter::Claim::shared(oer_hil_arbiter::AIR),
        ],
        None => Vec::new(),
    };
    let _grant = arbiter.acquire(&request)?;
    let lease = oer_esp32s31_firmware::device::DeviceLease::select(port)?;
    // The selector goes last: an interrupted write leaves the previous
    // selection pointing at an image whose checksum no longer validates.
    let segments = [
        (BOOTLOADER_OFFSET, "bootloader.bin", "bootloader"),
        (PARTITION_TABLE_OFFSET, "partitions.bin", "partition table"),
        (OTA_0_OFFSET, "application.bin", "application"),
        (OTA_SELECTOR_OFFSET, "otadata.bin", "ota_0 selector"),
    ]
    .into_iter()
    .map(|(address, filename, description)| {
        Ok(FlashSegment {
            address,
            data: fs::read(output.join(filename))?,
            description,
        })
    })
    .collect::<Result<Vec<_>>>()?;
    write_segments(lease.port(), &segments, AfterFlash::HardReset)?;
    let port = Some(lease.port());
    let application = &segments[2].data;
    arbiter.record_board(
        lease
            .port()
            .canonicalize()
            .ok()
            .as_deref()
            .and_then(oer_hil_arbiter::port_mac),
        oer_hil_arbiter::BoardEventKind::Flashed {
            image: example.to_owned(),
            application_sha256: format!("{:x}", sha2::Sha256::digest(application)),
            commit: None,
            dirty: None,
            origin: format!("xtask build firmware {example} --flash"),
        },
    )?;
    if monitor {
        monitor::run(port.ok_or("--monitor requires --port")?)?;
    }
    Ok(())
}

/// The port of the only attached board registered as `chip`. Several USB
/// boards are attached to the stand, so automatic selection uses the board
/// registry rather than the number of serial ports.
fn only_attached_board(
    chip: &str,
    devices: &[oer_hil_arbiter::Device],
) -> Option<std::path::PathBuf> {
    select_board(chip, devices, &oer_hil_arbiter::attached_ports())
}

fn select_board(
    chip: &str,
    devices: &[oer_hil_arbiter::Device],
    attached: &[oer_hil_arbiter::AttachedPort],
) -> Option<std::path::PathBuf> {
    let mut matches = attached.iter().filter(|port| {
        devices.iter().any(|device| {
            Some(&device.mac) == port.mac.as_ref() && device.chip.as_deref() == Some(chip)
        })
    });
    match (matches.next(), matches.next()) {
        (Some(port), None) => Some(std::path::PathBuf::from(&port.port)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_flashing_picks_the_only_attached_board_of_the_chip() {
        let device = |mac: &str, chip: &str| oer_hil_arbiter::Device {
            mac: mac.into(),
            chip: Some(chip.into()),
            ..oer_hil_arbiter::Device::default()
        };
        let port = |port: &str, mac: &str| oer_hil_arbiter::AttachedPort {
            port: port.into(),
            mac: Some(mac.into()),
            vid: 0x303a,
            pid: 0x1001,
            product: None,
        };
        let devices = [device("AA", "esp32s31"), device("BB", "esp32c5")];
        let attached = [port("/dev/ttyACM1", "BB"), port("/dev/ttyACM0", "AA")];
        assert_eq!(
            select_board("esp32s31", &devices, &attached),
            Some("/dev/ttyACM0".into())
        );
        assert_eq!(select_board("esp32s3", &devices, &attached), None);
        let two = [device("AA", "esp32s31"), device("BB", "esp32s31")];
        assert_eq!(select_board("esp32s31", &two, &attached), None);
    }
}
