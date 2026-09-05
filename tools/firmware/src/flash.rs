//! Board flash transactions. ROM reads DIO; ESP-IDF enables QIO for applications.
use crate::Result;
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};
pub const BOOTLOADER_OFFSET: u32 = 0x2000;
pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
pub const OTA_SELECTOR_OFFSET: u32 = 0xd000;
pub const OTA_0_OFFSET: u32 = 0x1_0000;
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

pub fn write_bin_command(
    command: &mut Command,
    port: Option<&Path>,
    address: u32,
    image: &Path,
    after: &str,
) {
    command.args(["write-bin", "--chip", "esp32s31", "--non-interactive"]);
    if let Some(port) = port {
        command.arg("--port").arg(port);
    }
    command
        .args(["--after", after])
        .arg(format!("{address:#x}"))
        .arg(image);
}

pub fn ota0_selector_image() -> [u8; OTA_DATA_SIZE] {
    let sequence = 1_u32;
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
