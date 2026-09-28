//! Attached-device inspection and exact flash transactions.

use std::{fs, path::Path, process::Command};

use crate::{
    Result,
    image::{Artifacts, program_from_env, run_command},
};

use oer_esp32s31_firmware::flash::{
    AfterFlash, FlashSegment, OTA_0_OFFSET, OTA_SELECTOR_OFFSET, PARTITION_TABLE_OFFSET,
    ota0_selector_image,
};

pub fn flash(root: &Path, artifacts: &Artifacts, port: &Path) -> Result<()> {
    flash_application(
        root,
        &artifacts.application_image,
        Some(&artifacts.bootstrap_elf),
        &artifacts.output,
        port,
    )
}

pub fn flash_archived(
    root: &Path,
    firmware: &crate::evidence::verify::ArchivedFirmware,
    port: &Path,
) -> Result<()> {
    flash_replayed(
        root,
        &firmware.application_path,
        &firmware.run_id,
        firmware.image,
        port,
    )
}

pub fn flash_replayed(
    root: &Path,
    application: &Path,
    run_id: &str,
    image: crate::image::ImageClass,
    port: &Path,
) -> Result<()> {
    let output = root
        .join("target/hil/esp32s31/replay")
        .join(run_id)
        .join(image.id());
    // Every archived image keeps its bootstrap ELF beside the application.
    let bootstrap = application.with_file_name("bootstrap.elf");
    flash_application(
        root,
        application,
        bootstrap.is_file().then_some(bootstrap.as_path()),
        &output,
        port,
    )
}

/// Write the ESP-IDF bootloader for the board's flash layout, the HIL
/// partition table, `application` into `ota_0` and an `ota_0` selector, then
/// reset; `output` receives the encoded images.
///
/// The bootloader is written on every flash: the stand's boards are shared,
/// and a consumer flashing its own ESP-IDF image (for example with a 2 MB
/// flash size) leaves a bootloader that rejects the HIL partition table. It
/// comes from the same `espflash` encoding of the image's `bootstrap` ELF that
/// standalone firmware flashes; without an ELF the board's bootloader stays.
pub fn flash_application(
    root: &Path,
    application: &Path,
    bootstrap: Option<&Path>,
    output: &Path,
    port: &Path,
) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut segments = Vec::new();
    match bootstrap {
        Some(bootstrap) => {
            let container = output.join("rom-container.bin");
            let mut encode = Command::new(program_from_env("ESPFLASH", "espflash"));
            oer_esp32s31_firmware::save_rom_image_command(&mut encode, root, bootstrap, &container);
            run_command(&mut encode, "encode the ESP-IDF bootloader")?;
            let bootloader = output.join("bootloader.bin");
            let container = fs::read(&container)?;
            let data = oer_esp32s31_firmware::flash::rom_bootloader(&container)?.to_vec();
            fs::write(&bootloader, &data)?;
            segments.push(FlashSegment {
                address: oer_esp32s31_firmware::flash::BOOTLOADER_OFFSET,
                data,
                description: "ESP-IDF bootloader",
            });
        }
        None => eprintln!(
            "hil: {} has no bootstrap ELF; the board keeps its current bootloader",
            application.display()
        ),
    }
    let partition_csv = root.join("platform/esp32s31/partitions/applications.csv");
    let partition_bin = output.join("partitions.bin");
    let selector_bin = output.join("otadata-ota0-valid.bin");

    let mut partition = Command::new(program_from_env("ESPFLASH", "espflash"));
    partition
        .args(["partition-table", "--to-binary", "--output"])
        .arg(&partition_bin)
        .arg(&partition_csv);
    run_command(&mut partition, "encode HIL partition table")?;
    fs::write(&selector_bin, ota0_selector_image())?;
    segments.push(FlashSegment {
        address: PARTITION_TABLE_OFFSET,
        data: fs::read(&partition_bin)?,
        description: "HIL partition table",
    });
    segments.push(FlashSegment {
        address: OTA_0_OFFSET,
        data: fs::read(application)?,
        description: "HIL application",
    });
    // The selector is written last: an interrupted write leaves the previous
    // selection pointing at an image whose checksum no longer validates.
    segments.push(FlashSegment {
        address: OTA_SELECTOR_OFFSET,
        data: ota0_selector_image().to_vec(),
        description: "HIL ota_0 selector",
    });
    oer_esp32s31_firmware::flash::write_segments(port, &segments, AfterFlash::HardReset)
        .map_err(|error| format!("flash the HIL image through {}: {error}", port.display()).into())
}
