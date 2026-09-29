//! Attached-device inspection and exact flash transactions.

use std::{fs, path::Path, process::Command};

use crate::{
    Result,
    image::{Artifacts, BootArtifacts, program_from_env, run_command},
};

use oer_esp32s31_firmware::flash::{
    AfterFlash, FlashSegment, OTA_0_OFFSET, OTA_SELECTOR_OFFSET, PARTITION_TABLE_OFFSET,
    ota0_selector_image,
};

/// How many times a flash is attempted when espflash's link to the chip
/// fails on its way.
const FLASH_ATTEMPTS: usize = 3;

pub fn flash(root: &Path, artifacts: &Artifacts, port: &Path) -> Result<()> {
    with_flash_retries(port, || flash_once(root, artifacts, port))
}

fn flash_once(root: &Path, artifacts: &Artifacts, port: &Path) -> Result<()> {
    match &artifacts.boot {
        BootArtifacts::Staged { bootstrap_elf, .. } => flash_application(
            root,
            &artifacts.application_image,
            Some(bootstrap_elf),
            &artifacts.output,
            port,
        ),
        BootArtifacts::EspIdf(boot) => flash_esp_idf_application(
            &boot.espflash_chip,
            boot.flash,
            [
                &boot.bootloader,
                &boot.partition_table,
                &artifacts.application_image,
            ],
            &artifacts.output,
            port,
        ),
    }
}

/// Write an ESP-IDF application with its bootloader and partition table at
/// the chip profile's offsets, then start it; `work` receives the written
/// flash image.
///
/// The three regions are merged into one image from the bootloader's offset,
/// the gaps erased (`0xff`), and written by one `espflash write-bin` that
/// resets the chip through its USB Serial/JTAG. That write ends in the ROM
/// bootloader, since the stub's own reset can leave an esp32c5 in download
/// mode; a power-on reset through the board's EN path starts the application
/// (see [`crate::session::reset::power_on_reset`]), an RTS reset one without.
pub fn flash_esp_idf_application(
    espflash_chip: &str,
    flash: oer_chip_profile::FlashLayout,
    [bootloader, partition_table, application]: [&Path; 3],
    work: &Path,
    port: &Path,
) -> Result<()> {
    let image = merged_image(
        flash,
        [
            &fs::read(bootloader)?,
            &fs::read(partition_table)?,
            &fs::read(application)?,
        ],
    )?;
    fs::create_dir_all(work)?;
    let merged = work.join("flash-image.bin");
    fs::write(&merged, image)?;
    let mut write = Command::new(program_from_env("ESPFLASH", "espflash"));
    write
        .args(["write-bin", "--non-interactive", "--chip", espflash_chip])
        .args(["--before", "usb-reset", "--after", "no-reset", "--port"])
        .arg(port)
        .arg(format!("{:#x}", flash.bootloader))
        .arg(&merged);
    run_command(&mut write, "write the HIL image")?;
    if !crate::session::reset::power_on_reset(port)? {
        drop(crate::session::reset::reset_into_application(port)?);
    }
    Ok(())
}

/// The flash contents from the bootloader's offset to the application's end:
/// each region at its offset, erased flash (`0xff`) between them.
fn merged_image(
    flash: oer_chip_profile::FlashLayout,
    [bootloader, partition_table, application]: [&[u8]; 3],
) -> Result<Vec<u8>> {
    let regions = [
        (flash.bootloader, bootloader, "bootloader"),
        (flash.partition_table, partition_table, "partition table"),
        (flash.application, application, "application"),
    ];
    let start = flash.bootloader as usize;
    let end = flash.application as usize + application.len();
    let mut image = vec![0xff; end - start];
    for (index, (offset, data, name)) in regions.iter().enumerate() {
        let from = *offset as usize - start;
        let limit = regions
            .get(index + 1)
            .map_or(end, |(next, ..)| *next as usize);
        if *offset as usize + data.len() > limit {
            return Err(format!("the {name} overlaps the next flash region").into());
        }
        image[from..from + data.len()].copy_from_slice(data);
    }
    Ok(image)
}

pub fn flash_archived(
    root: &Path,
    firmware: &crate::evidence::verify::ArchivedFirmware,
    port: &Path,
) -> Result<()> {
    flash_replayed(
        root,
        &firmware.target,
        &firmware.application_path,
        &firmware.run_id,
        firmware.image,
        port,
    )
}

pub fn flash_replayed(
    root: &Path,
    chip: &str,
    application: &Path,
    run_id: &str,
    image: crate::image::ImageClass,
    port: &Path,
) -> Result<()> {
    with_flash_retries(port, || {
        flash_replayed_once(root, chip, application, run_id, image, port)
    })
}

fn flash_replayed_once(
    root: &Path,
    chip: &str,
    application: &Path,
    run_id: &str,
    image: crate::image::ImageClass,
    port: &Path,
) -> Result<()> {
    let profile = crate::image::chip_profile(chip)?;
    if profile.boot == oer_chip_profile::Boot::EspIdfBootloader {
        // A replay imports the bootloader and partition table beside the
        // application.
        let flash = profile
            .flash
            .ok_or_else(|| format!("platform/{chip}/chip.toml names no flash layout"))?;
        return flash_esp_idf_application(
            &profile.espflash_chip,
            flash,
            [
                &application.with_file_name("bootloader.bin"),
                &application.with_file_name("partition-table.bin"),
                application,
            ],
            &root
                .join("target/hil")
                .join(chip)
                .join("replay")
                .join(run_id)
                .join(image.id()),
            port,
        );
    }
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

/// Run `write` again when espflash's link to the chip failed on its way,
/// with a power-on reset between the attempts where the board has one; each
/// retry is reported on stderr, which the job log keeps.
fn with_flash_retries(port: &Path, mut write: impl FnMut() -> Result<()>) -> Result<()> {
    retry_transient(FLASH_ATTEMPTS, &mut write, |attempt, error| {
        eprintln!(
            "hil: flash attempt {attempt} of {FLASH_ATTEMPTS} through {} failed: {error}; \
             retrying after a power-on reset",
            port.display()
        );
        if let Err(error) = crate::session::reset::power_on_reset(port) {
            eprintln!("hil: the power-on reset before the next flash attempt failed: {error}");
        }
    })
}

fn retry_transient<T>(
    attempts: usize,
    operation: &mut impl FnMut() -> Result<T>,
    mut between: impl FnMut(usize, &str),
) -> Result<T> {
    let mut attempt = 1;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && transient_flash_failure(&error.to_string()) => {
                between(attempt, &error.to_string());
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Whether a flash failure lies in the serial link to the chip, which a
/// reset and a new connection can clear, rather than in the image.
fn transient_flash_failure(message: &str) -> bool {
    const LINK_FAILURES: [&str; 6] = [
        "Protocol error",
        "timed out",
        "Timeout",
        "Broken pipe",
        "Input/output error",
        "Failed to connect",
    ];
    LINK_FAILURES
        .iter()
        .any(|failure| message.contains(failure))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_failure_is_retried_and_an_image_failure_is_not() {
        let mut calls = 0;
        let mut resets = Vec::new();
        let written = retry_transient(
            FLASH_ATTEMPTS,
            &mut || {
                calls += 1;
                if calls == 1 {
                    Err("espflash: Protocol error (os error 71)".into())
                } else {
                    Ok(calls)
                }
            },
            |attempt, _| resets.push(attempt),
        );
        assert_eq!(written.unwrap(), 2);
        assert_eq!(resets, [1]);

        let mut calls = 0;
        let refused = retry_transient(
            FLASH_ATTEMPTS,
            &mut || -> Result<()> {
                calls += 1;
                Err("the HIL application overlaps the next flash region".into())
            },
            |_, _| {},
        );
        assert!(refused.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn a_link_that_keeps_failing_ends_after_the_last_attempt() {
        let mut calls = 0;
        let mut resets = 0;
        let result = retry_transient(
            FLASH_ATTEMPTS,
            &mut || -> Result<()> {
                calls += 1;
                Err("serial port timed out".into())
            },
            |_, _| resets += 1,
        );
        assert!(result.is_err());
        assert_eq!(calls, FLASH_ATTEMPTS);
        assert_eq!(resets, FLASH_ATTEMPTS - 1);
    }

    #[test]
    fn a_merged_image_places_each_region_at_its_offset_over_erased_flash() {
        let flash = oer_chip_profile::FlashLayout {
            bootloader: 0x2000,
            partition_table: 0x8000,
            application: 0x10000,
        };
        let image = merged_image(flash, [&[1, 2], &[3], &[4, 5, 6]]).unwrap();
        assert_eq!(image.len(), 0x10000 - 0x2000 + 3);
        assert_eq!(&image[..3], &[1, 2, 0xff]);
        assert_eq!(image[0x6000], 3);
        assert_eq!(&image[0xe000..], &[4, 5, 6]);
        assert!(image[0x6001..0xe000].iter().all(|byte| *byte == 0xff));
        let oversized = vec![0; 0x7000];
        assert!(merged_image(flash, [&oversized, &[3], &[4]]).is_err());
    }
}
