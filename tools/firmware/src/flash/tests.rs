use super::*;

#[test]
fn ota0_selector_uses_valid_idf_entry() {
    let image = ota0_selector_image();
    assert_eq!(u32::from_le_bytes(image[0..4].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(image[24..28].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(image[28..32].try_into().unwrap()),
        crc32_idf(&1_u32.to_le_bytes())
    );
    assert!(image[32..].iter().all(|byte| *byte == 0xff));
}

fn rom_container() -> Vec<u8> {
    let mut container = vec![0xff; PARTITION_TABLE_OFFSET as usize];
    let image = &mut container[BOOTLOADER_OFFSET as usize..];
    image[..24].fill(0);
    image[0] = 0xe9;
    image[1] = 1;
    image[2] = 2;
    image[23] = 1;
    image[24..28].copy_from_slice(&0x2f00_0000_u32.to_le_bytes());
    image[28..32].copy_from_slice(&4_u32.to_le_bytes());
    image[32..36].copy_from_slice(b"boot");
    image[36..48].fill(0);
    image[47] = b"boot".iter().fold(0xef, |sum, byte| sum ^ byte);
    let digest = Sha256::digest(&image[..48]);
    image[48..80].copy_from_slice(&digest);
    container
}

#[test]
fn rom_extraction_preserves_the_complete_verified_image() {
    let container = rom_container();
    let image = rom_bootloader(&container).unwrap();
    assert_eq!(
        image,
        &container[BOOTLOADER_OFFSET as usize..BOOTLOADER_OFFSET as usize + 80]
    );
}

#[test]
fn rom_extraction_rejects_qio_corruption_and_out_of_bounds_segments() {
    let valid = rom_container();
    for offset in [2, 32, 47, 48] {
        let mut corrupt = valid.clone();
        corrupt[BOOTLOADER_OFFSET as usize + offset] ^= 2;
        assert!(rom_bootloader(&corrupt).is_err());
    }
    let mut truncated = valid;
    truncated[BOOTLOADER_OFFSET as usize + 28..BOOTLOADER_OFFSET as usize + 32]
        .copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(rom_bootloader(&truncated).is_err());
    assert!(rom_bootloader(&[]).is_err());
}
