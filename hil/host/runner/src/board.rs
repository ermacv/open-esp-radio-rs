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
