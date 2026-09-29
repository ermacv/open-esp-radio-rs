//! Board flash transactions. ROM reads DIO; ESP-IDF enables QIO for applications.
use crate::Result;
use sha2::{Digest, Sha256};
use std::path::Path;
pub const BOOTLOADER_OFFSET: u32 = 0x2000;
pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
pub const OTA_SELECTOR_OFFSET: u32 = 0xd000;
pub const OTA_0_OFFSET: u32 = 0x1_0000;
/// Size of `ota_0` in `platform/esp32s31/partitions/applications.csv`, the
/// partition every application image is encoded for.
pub const OTA_0_BYTES: u32 = 0xff_0000;
/// Share of `ota_0` from which image encoding warns that the partition is
/// nearly full, before an image stops fitting.
pub const OTA_0_WARNING_PERCENT: u64 = 90;

/// A warning when an application image of `bytes` fills at least
/// [`OTA_0_WARNING_PERCENT`] of `ota_0`.
pub fn ota_0_budget_warning(bytes: u64) -> Option<String> {
    let capacity = u64::from(OTA_0_BYTES);
    let percent = bytes * 100 / capacity;
    (percent >= OTA_0_WARNING_PERCENT).then(|| {
        format!(
            "application image uses {percent}% of ota_0 ({bytes} of {capacity} bytes); \
             reduce code size before it stops fitting"
        )
    })
}
const OTA_DATA_SIZE: usize = 0x2000;

/// Extract a complete ROM image from an espflash merged container. Validate
/// its segment checksum and digest before any hardware write is requested.
pub fn rom_bootloader(container: &[u8]) -> Result<&[u8]> {
    let bytes = container
        .get(BOOTLOADER_OFFSET as usize..PARTITION_TABLE_OFFSET as usize)
        .ok_or("ROM container does not cover the bootloader partition")?;
    if bytes[0] != 0xe9 || bytes[1] == 0 || bytes[2] != 2 || bytes[23] != 1 {
        return Err("ROM bootloader must be a hashed DIO ESP image".into());
    }
    let mut position: usize = 24;
    let mut checksum = 0xef;
    for _ in 0..bytes[1] {
        let header = bytes
            .get(position..position + 8)
            .ok_or("truncated ROM segment header")?;
        let length = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        position += 8;
        let end = position
            .checked_add(length)
            .ok_or("ROM segment length overflow")?;
        let data = bytes
            .get(position..end)
            .ok_or("ROM segment exceeds bootloader partition")?;
        for byte in data {
            checksum ^= byte;
        }
        position = end;
    }
    let digest_start = position.checked_add(16).ok_or("ROM image size overflow")? & !15;
    let image = bytes
        .get(..digest_start + 32)
        .ok_or("truncated ROM checksum or digest")?;
    if image[digest_start - 1] != checksum
        || Sha256::digest(&image[..digest_start]).as_slice() != &image[digest_start..]
    {
        return Err("ROM image checksum or SHA-256 mismatch".into());
    }
    Ok(image)
}

pub fn ota0_selector_image() -> [u8; OTA_DATA_SIZE] {
    ota_selector_image(0)
}

/// The OTA data that makes the ESP-IDF bootloader boot application slot
/// `slot` (0 for `ota_0`, 1 for `ota_1`): its sequence number modulo the
/// number of slots, plus one.
pub fn ota_selector_image(slot: u32) -> [u8; OTA_DATA_SIZE] {
    let sequence = slot + 1;
    let mut image = [0xff; OTA_DATA_SIZE];
    image[0..4].copy_from_slice(&sequence.to_le_bytes());
    image[24..28].copy_from_slice(&2_u32.to_le_bytes());
    image[28..32].copy_from_slice(&crc32_idf(&sequence.to_le_bytes()).to_le_bytes());
    image
}

fn crc32_idf(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc ^ u32::MAX
}

#[cfg(test)]
mod tests;

/// One region of a flash write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlashSegment {
    pub address: u32,
    pub data: Vec<u8>,
    pub description: &'static str,
}

/// What to do with the chip after the last segment is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AfterFlash {
    /// Reset into the written application.
    HardReset,
    /// Stay in the ROM bootloader.
    StayInBootloader,
}

/// Write every segment through one connection to the ROM stub, in order.
///
/// Each separate `espflash write-bin` reconnects, resets into the bootloader
/// and uploads its stub again, which dominated flashing time while the
/// written regions are small. A segment whose flash contents already match
/// is skipped after an MD5 comparison, so an unchanged bootloader or
/// partition table costs one checksum.
#[cfg(feature = "device")]
pub fn write_segments(port: &Path, segments: &[FlashSegment], after: AfterFlash) -> Result<()> {
    use espflash::{
        connection::{Connection, ResetAfterOperation, ResetBeforeOperation},
        flasher::Flasher,
        image_format::Segment,
        target::{Chip, DefaultProgressCallback},
    };
    use serialport::SerialPortType;

    let port_name = port.to_string_lossy().into_owned();
    let usb = serialport::available_ports()?
        .into_iter()
        .find(|info| {
            info.port_name == port_name
                || std::fs::canonicalize(&info.port_name).ok() == std::fs::canonicalize(port).ok()
        })
        .and_then(|info| match info.port_type {
            SerialPortType::UsbPort(usb) => Some(usb),
            _ => None,
        })
        .ok_or_else(|| format!("{port_name} is not an attached USB serial port"))?;
    let serial = serialport::new(&port_name, 115_200).open_native()?;
    let connection = Connection::new(
        serial,
        usb,
        match after {
            AfterFlash::HardReset => ResetAfterOperation::HardReset,
            AfterFlash::StayInBootloader => ResetAfterOperation::NoReset,
        },
        ResetBeforeOperation::DefaultReset,
        115_200,
    );
    let mut flasher = Flasher::connect(connection, true, false, true, Some(Chip::Esp32s31), None)?;
    let segments = segments
        .iter()
        .map(|segment| Segment::new(segment.address, &segment.data))
        .collect::<Vec<_>>();
    flasher.write_bins_to_flash(&segments, &mut DefaultProgressCallback)?;
    flasher.connection().reset_after(true, Chip::Esp32s31)?;
    Ok(())
}
