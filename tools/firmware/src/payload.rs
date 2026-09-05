//! Stage-two image packing; the checksum field is zero during calculation.
use crate::Result;
use std::{fs, path::Path};
const RUNTIME_MAGIC: u32 = 0x3247_5453;
pub const RUNTIME_CRC_OFFSET: usize = 40;
const RUNTIME_HEADER_BYTES: usize = 44;
pub fn pack_runtime(path: &Path) -> Result<u32> {
    let mut bytes = fs::read(path)?;
    if bytes.len() < RUNTIME_HEADER_BYTES {
        return Err("runtime image is shorter than its header".into());
    }
    if u32::from_le_bytes(bytes[0..4].try_into()?) != RUNTIME_MAGIC {
        return Err("runtime image has the wrong stage-two magic".into());
    }
    if u32::from_le_bytes(bytes[28..32].try_into()?) as usize != RUNTIME_HEADER_BYTES {
        return Err("runtime image has an incompatible header size".into());
    }
    bytes[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    let crc = crc32(&bytes);
    bytes[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    fs::write(path, bytes)?;
    let packed = fs::read(path)?;
    let stored = u32::from_le_bytes(packed[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].try_into()?);
    let mut verified = packed;
    verified[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    if stored != crc || crc32(&verified) != crc {
        return Err("runtime CRC did not survive packing".into());
    }
    Ok(crc)
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
