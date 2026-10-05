//! The images this runner flashes into its device under test: the bundles a
//! build or a run's archive hands it, written by the flash operation under
//! the run's lock of the board.

use std::path::Path;

use oer_hil_arbiter::lock::BoardLock;
use oer_hil_image_class::ImageClass;
use oer_hil_run_bundle::run::Boot;
use oer_image::{ImageBundle, bundle::BootFiles};

use crate::Result;

/// What the journal records of an image the runner flashes.
pub(crate) struct Flash<'a> {
    /// The image class.
    pub image: &'a str,
    pub revision: oer_hil_flash::Revision,
    /// The run that flashes it.
    pub origin: String,
}

/// Flash `bundle` into the device under test of `lab` through the flash
/// operation under `lock`: written, journaled, then started.
pub(crate) fn flash_dut(
    lab: &oer_hil_lab::config::LabConfig,
    lock: &BoardLock,
    bundle: &ImageBundle,
    flash: Flash<'_>,
) -> Result<()> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?.with_stand_file(lab.path().to_owned());
    oer_hil_flash::flash(
        &arbiter,
        &oer_hil_arbiter::owner_from_environment()?,
        lock,
        &lab.dut_board()?,
        &oer_hil_flash::Image {
            bundle,
            name: flash.image,
            revision: flash.revision,
            origin: flash.origin,
        },
        oer_hil_board::Via::Usb,
    )
}

/// The bundle of an archived `application` of `chip` with what a run keeps
/// beside it: the bootstrap ELF of a staged image, from which the image
/// pipeline encodes its boot files again, the bootloader and partition table
/// of an ESP-IDF one. The bundle goes below this checkout's replay
/// directory of `run_id`.
pub(crate) fn archived(
    root: &Path,
    chip: &str,
    application: &Path,
    run_id: &str,
    image: ImageClass,
) -> Result<ImageBundle> {
    let bootstrap = application.with_file_name("bootstrap.elf");
    let bootloader = application.with_file_name("bootloader.bin");
    let partition_table = application.with_file_name("partition-table.bin");
    let boot = match oer_hil_image::chip_profile(chip)?.boot {
        Boot::Staged => {
            if !bootstrap.is_file() {
                return Err(format!(
                    "the archived {} image has no bootstrap ELF to encode its boot files from; \
                     rebuild it",
                    image.id()
                )
                .into());
            }
            BootFiles::Encode { elf: &bootstrap }
        }
        Boot::EspIdfBootloader => BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &partition_table,
        },
    };
    oer_image::bundle::around(
        root,
        chip,
        application,
        boot,
        &root
            .join("target/hil")
            .join(chip)
            .join("replay")
            .join(run_id)
            .join(image.id()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_becomes_its_boot_flow_s_bundle_in_its_chip_s_replay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let directory = tempfile::tempdir().unwrap();
        let application = directory.path().join("application.bin");
        for (name, bytes) in [
            ("application.bin", &b"application"[..]),
            ("bootloader.bin", b"bootloader"),
            ("partition-table.bin", b"table"),
        ] {
            std::fs::write(directory.path().join(name), bytes).unwrap();
        }
        let run = format!("r{}", std::process::id());
        let bundle = archived(
            &root,
            "esp32c5",
            &application,
            &run,
            ImageClass::SystemWatchdog,
        )
        .unwrap();
        assert_eq!(std::fs::read(bundle.bootloader()).unwrap(), b"bootloader");
        assert_eq!(std::fs::read(bundle.partitions()).unwrap(), b"table");
        assert!(
            bundle
                .directory
                .ends_with(format!("target/hil/esp32c5/replay/{run}/system-watchdog"))
        );
        std::fs::remove_dir_all(bundle.directory.parent().unwrap()).unwrap();
        // A staged archive without its bootstrap ELF cannot be replayed.
        let error = archived(
            &root,
            "esp32s31",
            &application,
            &run,
            ImageClass::SystemWatchdog,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("bootstrap ELF"), "{error}");
    }
}
