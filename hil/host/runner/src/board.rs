//! The boards this runner flashes: each chip's boot flow, chosen from its
//! profile, and the images a build or a run's archive hands it.

use std::path::Path;

use oer_hil_board::{Board, Companions, EspIdf, FlashImage};
use oer_hil_evidence::run::Boot;
use oer_hil_image::{Artifacts, BootArtifacts};
use oer_hil_image_class::ImageClass;

use crate::Result;

/// The boot flow of `chip`'s boards.
pub(crate) fn for_chip(root: &Path, chip: &str) -> Result<Box<dyn Board>> {
    let profile = oer_hil_image::chip_profile(chip)?;
    Ok(match profile.boot {
        Boot::Staged => Box::new(oer_esp32s31_hil_board::Staged {
            root: root.to_owned(),
        }),
        Boot::EspIdfBootloader => Box::new(EspIdf::new(&profile)?),
    })
}

/// A build's image as its board writes it.
pub(crate) fn built(artifacts: &Artifacts) -> FlashImage {
    FlashImage {
        application: artifacts.application_image.clone(),
        companions: match &artifacts.boot {
            BootArtifacts::Staged { bootstrap_elf, .. } => Companions::Staged {
                bootstrap_elf: Some(bootstrap_elf.clone()),
            },
            BootArtifacts::EspIdf(boot) => Companions::EspIdf {
                bootloader: boot.bootloader.clone(),
                partition_table: boot.partition_table.clone(),
            },
        },
        work: artifacts.output.clone(),
    }
}

/// An archived `application` of `chip` with what a run keeps beside it: the
/// bootstrap ELF of a staged image, the bootloader and partition table of an
/// ESP-IDF one. The encoded flash contents go below this checkout's replay
/// directory of `run_id`.
pub(crate) fn archived(
    root: &Path,
    chip: &str,
    application: &Path,
    run_id: &str,
    image: ImageClass,
) -> Result<FlashImage> {
    let companions = match oer_hil_image::chip_profile(chip)?.boot {
        Boot::Staged => {
            let bootstrap = application.with_file_name("bootstrap.elf");
            Companions::Staged {
                bootstrap_elf: bootstrap.is_file().then_some(bootstrap),
            }
        }
        Boot::EspIdfBootloader => Companions::EspIdf {
            bootloader: application.with_file_name("bootloader.bin"),
            partition_table: application.with_file_name("partition-table.bin"),
        },
    };
    Ok(FlashImage {
        application: application.to_owned(),
        companions,
        work: root
            .join("target/hil")
            .join(chip)
            .join("replay")
            .join(run_id)
            .join(image.id()),
    })
}

/// The device under test's port, ready for a flash: a board that is not on
/// USB, because its image switched its USB Serial/JTAG off, is put into its
/// ROM's download mode first through its hub port's power.
pub(crate) fn flashable_port(lab: &oer_hil_stand::config::LabConfig) -> Result<std::path::PathBuf> {
    let port = lab.dut.serial.clone();
    if port.exists() {
        return Ok(port);
    }
    let mac = lab.dut_mac()?;
    let control = oer_hil_stand::control::BoardControl::of_board(&port, &mac)?;
    let entry = control.download_entry().ok_or_else(|| {
        format!(
            "board `{}` is not on USB and does not reset by power; a person must reset it",
            lab.dut.id
        )
    })?;
    let banner = entry()?;
    eprintln!(
        "hil: board `{}` was not on USB; its ROM now waits for the flash: {}",
        lab.dut.id,
        oer_hil_stand::control::reset_line(&banner)
            .as_deref()
            .unwrap_or("no reset line")
    );
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_keeps_each_boot_flow_s_companions_and_its_chip_s_replay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let application = Path::new("/runs/r/firmware/system-watchdog/application.bin");
        let image = archived(
            &root,
            "esp32c5",
            application,
            "r",
            ImageClass::SystemWatchdog,
        )
        .unwrap();
        assert_eq!(
            image.companions,
            Companions::EspIdf {
                bootloader: application.with_file_name("bootloader.bin"),
                partition_table: application.with_file_name("partition-table.bin"),
            }
        );
        assert!(
            image
                .work
                .ends_with("target/hil/esp32c5/replay/r/system-watchdog")
        );
        let staged = archived(
            &root,
            "esp32s31",
            application,
            "r",
            ImageClass::SystemWatchdog,
        )
        .unwrap();
        assert_eq!(
            staged.companions,
            Companions::Staged {
                bootstrap_elf: None
            }
        );
        assert!(
            staged
                .work
                .ends_with("target/hil/esp32s31/replay/r/system-watchdog")
        );
    }
}
