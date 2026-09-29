//! The ESP-IDF bootloader flow: the catalog bootloader, the partition table
//! and the application at the chip profile's offsets.

use std::{fs, path::Path};

use crate::{Board, Companions, FlashImage, Result, espflash, reset, run, with_flash_retries};

/// The flow of a chip the ESP-IDF second-stage bootloader starts.
#[derive(Clone, Debug)]
pub struct EspIdf {
    /// The chip as `espflash` names it.
    pub espflash_chip: String,
    pub flash: oer_chip_profile::FlashLayout,
}

impl EspIdf {
    /// The flow of `profile`'s chip, which must name its flash layout.
    pub fn new(profile: &oer_chip_profile::Profile) -> Result<Self> {
        let flash = profile.flash.ok_or_else(|| {
            format!(
                "platform/{}/chip.toml names no flash layout for its HIL images",
                profile.id
            )
        })?;
        Ok(Self {
            espflash_chip: profile.espflash_chip.clone(),
            flash,
        })
    }
}

impl Board for EspIdf {
    /// The three regions are merged into one image from the bootloader's
    /// offset, the gaps erased (`0xff`), and written by one `espflash
    /// write-bin` that resets the chip through its USB Serial/JTAG. That
    /// write ends in the ROM bootloader, since the stub's own reset can leave
    /// an esp32c5 in download mode; a power-on reset through the board's EN
    /// path starts the application ([`reset::power_on_reset`]), an RTS reset
    /// one without.
    fn flash(&self, image: &FlashImage, port: &Path) -> Result<()> {
        let Companions::EspIdf {
            bootloader,
            partition_table,
        } = &image.companions
        else {
            return Err(
                "an ESP-IDF bootloader chip takes a bootloader and a partition table".into(),
            );
        };
        let merged = merged_image(
            self.flash,
            [
                &fs::read(bootloader)?,
                &fs::read(partition_table)?,
                &fs::read(&image.application)?,
            ],
        )?;
        fs::create_dir_all(&image.work)?;
        let path = image.work.join("flash-image.bin");
        fs::write(&path, merged)?;
        with_flash_retries(port, || {
            let mut write = espflash();
            write
                .args([
                    "write-bin",
                    "--non-interactive",
                    "--chip",
                    &self.espflash_chip,
                ])
                .args(["--before", "usb-reset", "--after", "no-reset", "--port"])
                .arg(port)
                .arg(format!("{:#x}", self.flash.bootloader))
                .arg(&path);
            run(&mut write, "write the HIL image")
        })?;
        if !reset::power_on_reset(port)? {
            drop(reset::reset_into_application(port)?);
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_staged_image_is_refused() {
        let flow = EspIdf {
            espflash_chip: String::from("esp32c5"),
            flash: oer_chip_profile::FlashLayout {
                bootloader: 0x2000,
                partition_table: 0x8000,
                application: 0x10000,
            },
        };
        let image = FlashImage {
            application: "application.bin".into(),
            companions: Companions::Staged {
                bootstrap_elf: None,
            },
            work: "work".into(),
        };
        assert!(flow.flash(&image, Path::new("/dev/null")).is_err());
    }
}
