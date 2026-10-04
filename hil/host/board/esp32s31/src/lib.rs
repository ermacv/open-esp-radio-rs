//! The esp32s31's board support: the staged boot flow, in which the ROM
//! loads the platform bootstrap that stages the packed runtime, and the
//! two-slot calibration layout.

use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_esp32s31_firmware::flash::{
    AfterFlash, FlashSegment, OTA_0_OFFSET, OTA_SELECTOR_OFFSET, PARTITION_TABLE_OFFSET,
    ota0_selector_image,
};
use oer_hil_board::{
    Board, Companions, FlashImage, Flashed, Result, espflash, reset, run, with_flash_retries,
};

/// The esp32s31's staged flow, reading the partition tables of the
/// repository at `root`.
#[derive(Clone, Debug)]
pub struct Staged {
    pub root: PathBuf,
}

impl Board for Staged {
    /// Write the ESP-IDF bootloader for the board's flash layout, the HIL
    /// partition table, the application into `ota_0` and an `ota_0`
    /// selector, then reset.
    ///
    /// The bootloader is written on every flash: the stand's boards are
    /// shared, and a consumer flashing its own ESP-IDF image (for example
    /// with a 2 MB flash size) leaves a bootloader that rejects the HIL
    /// partition table. It comes from the same `espflash` encoding of the
    /// image's bootstrap ELF that standalone firmware flashes; without an ELF
    /// the board's bootloader stays.
    fn flash(&self, image: &FlashImage, port: &Path) -> Result<Flashed> {
        let Companions::Staged { bootstrap_elf } = &image.companions else {
            return Err("a staged chip takes its bootstrap ELF, not an ESP-IDF bootloader".into());
        };
        fs::create_dir_all(&image.work)?;
        let mut segments = Vec::new();
        match bootstrap_elf {
            Some(bootstrap) => segments.push(self.bootloader(bootstrap, &image.work)?),
            None => eprintln!(
                "hil: {} has no bootstrap ELF; the board keeps its current bootloader",
                image.application.display()
            ),
        }
        let partition_bin = image.work.join("partitions.bin");
        let mut partition = espflash();
        partition
            .args(["partition-table", "--to-binary", "--output"])
            .arg(&partition_bin)
            .arg(self.root.join(APPLICATION_PARTITIONS));
        run(&mut partition, "encode HIL partition table")?;
        fs::write(
            image.work.join("otadata-ota0-valid.bin"),
            ota0_selector_image(),
        )?;
        segments.push(FlashSegment {
            address: PARTITION_TABLE_OFFSET,
            data: fs::read(&partition_bin)?,
            description: "HIL partition table",
        });
        segments.push(FlashSegment {
            address: OTA_0_OFFSET,
            data: fs::read(&image.application)?,
            description: "HIL application",
        });
        // The selector is written last: an interrupted write leaves the
        // previous selection pointing at an image whose checksum no longer
        // validates.
        segments.push(FlashSegment {
            address: OTA_SELECTOR_OFFSET,
            data: ota0_selector_image().to_vec(),
            description: "HIL ota_0 selector",
        });
        with_flash_retries(port, || {
            oer_esp32s31_firmware::flash::write_segments(port, &segments, AfterFlash::HardReset)
                .map_err(|error| {
                    format!("flash the HIL image through {}: {error}", port.display()).into()
                })
        })?;
        Ok(Flashed::Started)
    }
}

impl Staged {
    /// The ESP-IDF bootloader segment encoded from `bootstrap`, written into
    /// `work` as well.
    fn bootloader(&self, bootstrap: &Path, work: &Path) -> Result<FlashSegment> {
        let container = work.join("rom-container.bin");
        let mut encode = espflash();
        oer_esp32s31_firmware::save_rom_image_command(
            &mut encode,
            &self.root,
            bootstrap,
            &container,
        );
        run(&mut encode, "encode the ESP-IDF bootloader")?;
        let data = oer_esp32s31_firmware::flash::rom_bootloader(&fs::read(&container)?)?.to_vec();
        fs::write(work.join("bootloader.bin"), &data)?;
        Ok(FlashSegment {
            address: oer_esp32s31_firmware::flash::BOOTLOADER_OFFSET,
            data,
            description: "ESP-IDF bootloader",
        })
    }

    /// Write `application` into `slot` of the two-slot calibration layout,
    /// with the ESP-IDF bootloader encoded from `bootstrap` (when given) and
    /// the two-slot partition table, and leave the OTA selection and the chip
    /// in the ROM bootloader; [`boot_slot`] starts a slot. `work` receives
    /// the encoded table.
    pub fn flash_slot(
        &self,
        application: &Path,
        bootstrap: Option<&Path>,
        slot: Slot,
        work: &Path,
        port: &Path,
    ) -> Result<SlotImage> {
        fs::create_dir_all(work)?;
        let mut segments = Vec::new();
        if let Some(bootstrap) = bootstrap {
            segments.push(self.bootloader(bootstrap, work)?);
        }
        let table = work.join("calibration-slots.bin");
        let mut partition = espflash();
        partition
            .args(["partition-table", "--to-binary", "--output"])
            .arg(&table)
            .arg(self.root.join(SLOT_PARTITIONS));
        run(&mut partition, "encode the two-slot partition table")?;
        segments.push(FlashSegment {
            address: PARTITION_TABLE_OFFSET,
            data: fs::read(&table)?,
            description: "two-slot partition table",
        });
        let image = SlotImage {
            slot,
            application: fs::read(application)?,
        };
        segments.push(slot_segment(&image)?);
        with_flash_retries(port, || {
            oer_esp32s31_firmware::flash::write_segments(
                port,
                &segments,
                AfterFlash::StayInBootloader,
            )
            .map_err(|error| {
                format!("flash slot {slot:?} through {}: {error}", port.display()).into()
            })
        })?;
        Ok(image)
    }
}

/// The single-slot HIL partition table's source.
const APPLICATION_PARTITIONS: &str = "platform/esp32s31/partitions/applications.csv";
/// The two-slot calibration partition table's source.
const SLOT_PARTITIONS: &str = "platform/esp32s31/partitions/calibration-slots.csv";
const NVS: (u32, usize) = (0x9000, 0x4000);
const PHY_INIT: (u32, usize) = (0xf000, 0x1000);
/// How long a boot may take to name the partition it loads.
const SLOT_BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// An application slot of the two-slot calibration layout
/// (`platform/esp32s31/partitions/calibration-slots.csv`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    Ota0,
    Ota1,
}

impl Slot {
    /// The slot's flash offset in the two-slot layout.
    pub const fn offset(self) -> u32 {
        match self {
            Self::Ota0 => 0x1_0000,
            Self::Ota1 => 0x80_0000,
        }
    }

    /// The slot's index in the OTA selection.
    const fn index(self) -> u32 {
        match self {
            Self::Ota0 => 0,
            Self::Ota1 => 1,
        }
    }
}

/// Whether booting a slot first erases the calibration another firmware
/// left: the `phy_init` and `nvs` partitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EraseCalibration {
    Yes,
    No,
}

/// An application written into a slot, as [`boot_slot`] checks it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotImage {
    pub slot: Slot,
    pub application: Vec<u8>,
}

/// What [`boot_slot`] saw of the boot it started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootedSlot {
    pub slot: Slot,
    /// The console from the reset to the bootloader's line naming the
    /// partition it loaded.
    pub console: Vec<u8>,
}

/// Select `image`'s slot and erase the other firmware's calibration when
/// asked, leaving the chip in the ROM bootloader for the caller's own reset,
/// so it owns the console from the new boot's first byte; [`booted_slot`]
/// then tells which slot that boot loaded.
///
/// The slot's application is written again in the same connection, which
/// skips it after an MD5 comparison when the flash still holds it, so a
/// slot another tool overwrote is restored instead of booting the wrong
/// firmware. Only the OTA data and the erased partitions are written
/// otherwise: seconds, where a flash of the application takes a minute.
pub fn select_slot(port: &Path, image: &SlotImage, erase: EraseCalibration) -> Result<()> {
    let mut segments = vec![slot_segment(image)?];
    if erase == EraseCalibration::Yes {
        for (address, size, description) in [
            (PHY_INIT.0, PHY_INIT.1, "erased phy_init"),
            (NVS.0, NVS.1, "erased nvs"),
        ] {
            segments.push(FlashSegment {
                address,
                data: vec![0xff; size],
                description,
            });
        }
    }
    segments.push(FlashSegment {
        address: OTA_SELECTOR_OFFSET,
        data: oer_esp32s31_firmware::flash::ota_selector_image(image.slot.index()).to_vec(),
        description: "OTA selection",
    });
    with_flash_retries(port, || {
        oer_esp32s31_firmware::flash::write_segments(port, &segments, AfterFlash::StayInBootloader)
            .map_err(|error| {
                format!(
                    "select slot {:?} through {}: {error}",
                    image.slot,
                    port.display()
                )
                .into()
            })
    })
}

/// [`select_slot`], then reset into the slot and read its boot until the
/// bootloader names the partition it loaded, which must be that slot's.
pub fn boot_slot(port: &Path, image: &SlotImage, erase: EraseCalibration) -> Result<BootedSlot> {
    select_slot(port, image, erase)?;
    let mut serial = reset::reset_into_application(port)?;
    let started = std::time::Instant::now();
    let mut console = Vec::new();
    let mut buffer = [0; 1024];
    while started.elapsed() < SLOT_BOOT_TIMEOUT {
        match std::io::Read::read(&mut serial, &mut buffer) {
            Ok(read) => console.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(offset) = loaded_partition(&console) {
            if booted_slot(&console) != Some(image.slot) {
                return Err(format!(
                    "the bootloader loaded the partition at {offset:#x}, not slot {:?} at {:#x}",
                    image.slot,
                    image.slot.offset()
                )
                .into());
            }
            return Ok(BootedSlot {
                slot: image.slot,
                console,
            });
        }
    }
    Err(format!(
        "the boot of slot {:?} named no loaded partition within {}s",
        image.slot,
        SLOT_BOOT_TIMEOUT.as_secs()
    )
    .into())
}

fn slot_segment(image: &SlotImage) -> Result<FlashSegment> {
    let capacity: usize = match image.slot {
        Slot::Ota0 => 0x7f_0000,
        Slot::Ota1 => 0x80_0000,
    };
    if image.application.len() > capacity {
        return Err(format!(
            "the application ({} bytes) exceeds slot {:?} ({capacity} bytes)",
            image.application.len(),
            image.slot
        )
        .into());
    }
    Ok(FlashSegment {
        address: image.slot.offset(),
        data: image.application.clone(),
        description: match image.slot {
            Slot::Ota0 => "ota_0 application",
            Slot::Ota1 => "ota_1 application",
        },
    })
}

/// The slot a boot's console shows the ESP-IDF bootloader loading, from its
/// `Loaded app from partition at offset 0x…` line; `None` before the line
/// is complete or when it names no slot of the two-slot layout.
pub fn booted_slot(console: &[u8]) -> Option<Slot> {
    let offset = loaded_partition(console)?;
    [Slot::Ota0, Slot::Ota1]
        .into_iter()
        .find(|slot| slot.offset() == offset)
}

/// The partition offset the ESP-IDF bootloader's `Loaded app from partition
/// at offset 0x…` line names, once the line is complete.
fn loaded_partition(console: &[u8]) -> Option<u32> {
    const MARKER: &str = "Loaded app from partition at offset ";
    let text = String::from_utf8_lossy(console);
    let rest = &text[text.find(MARKER)? + MARKER.len()..];
    let line = rest.get(..rest.find(['\r', '\n'])?)?;
    u32::from_str_radix(line.trim().strip_prefix("0x")?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boot_names_the_partition_it_loaded() {
        let console = b"I (236) boot: Loaded app from partition at offset 0x800000\r\nI (237)";
        assert_eq!(loaded_partition(console), Some(Slot::Ota1.offset()));
        assert_eq!(booted_slot(console), Some(Slot::Ota1));
        // The single-slot HIL layout's ota_0 is the two-slot layout's too.
        assert_eq!(
            booted_slot(b"boot: Loaded app from partition at offset 0x10000\r\n"),
            Some(Slot::Ota0)
        );
        assert_eq!(
            booted_slot(b"boot: Loaded app from partition at offset 0x20000\r\n"),
            None
        );
        // A line still arriving names nothing yet.
        assert_eq!(
            loaded_partition(b"I (236) boot: Loaded app from partition at offset 0x80"),
            None
        );
        assert_eq!(
            loaded_partition(b"I (100) boot: ESP-IDF bootloader\r\n"),
            None
        );
    }

    #[test]
    fn a_slot_refuses_an_application_it_cannot_hold() {
        let fits = SlotImage {
            slot: Slot::Ota0,
            application: vec![0; 0x7f_0000],
        };
        assert_eq!(slot_segment(&fits).unwrap().address, 0x1_0000);
        let too_large = SlotImage {
            slot: Slot::Ota0,
            application: vec![0; 0x7f_0001],
        };
        assert!(slot_segment(&too_large).is_err());
    }

    #[test]
    fn an_esp_idf_image_is_refused() {
        let flow = Staged {
            root: PathBuf::from("."),
        };
        let image = FlashImage {
            application: "application.bin".into(),
            companions: Companions::EspIdf {
                bootloader: "bootloader.bin".into(),
                partition_table: "partition-table.bin".into(),
            },
            work: "work".into(),
        };
        assert!(flow.flash(&image, Path::new("/dev/null")).is_err());
    }
}
