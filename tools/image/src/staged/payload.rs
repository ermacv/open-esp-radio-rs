//! Stage-two image packing; the checksum field is zero during calculation.
use crate::Result;
use oer_esp32s31_platform_layout::stage_two::{self, Header};
use std::{fs, path::Path};
const CRC_FIELD: std::ops::Range<usize> = stage_two::CRC_OFFSET..stage_two::CRC_OFFSET + 4;
pub fn pack_runtime(path: &Path) -> Result<u32> {
    let mut bytes = fs::read(path)?;
    let header = Header::from_le_bytes(&bytes).ok_or("runtime image is shorter than its header")?;
    if header.magic != stage_two::MAGIC {
        return Err("runtime image has the wrong stage-two magic".into());
    }
    if !header.is_compatible() {
        return Err("runtime image has an incompatible stage-two header".into());
    }
    let crc = stage_two::payload_crc32(&bytes);
    bytes[CRC_FIELD].copy_from_slice(&crc.to_le_bytes());
    fs::write(path, bytes)?;
    let packed = fs::read(path)?;
    let stored = Header::from_le_bytes(&packed)
        .ok_or("packed runtime lost its header")?
        .payload_crc32;
    if stored != crc || stage_two::payload_crc32(&packed) != crc {
        return Err("runtime CRC did not survive packing".into());
    }
    Ok(crc)
}
