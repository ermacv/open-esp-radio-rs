use oer_firmware::{RUNTIME_CRC_OFFSET, crc32, pack_runtime};

#[test]
fn packing_preserves_payload_and_is_repeatable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    let mut image = vec![0x5a; 128];
    image[..4].copy_from_slice(&0x3247_5453_u32.to_le_bytes());
    image[28..32].copy_from_slice(&44_u32.to_le_bytes());
    std::fs::write(&path, &image).unwrap();
    let checksum = pack_runtime(&path).unwrap();
    let packed = std::fs::read(&path).unwrap();
    assert_eq!(checksum, pack_runtime(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), packed);
    assert_eq!(&packed[..RUNTIME_CRC_OFFSET], &image[..RUNTIME_CRC_OFFSET]);
    assert_eq!(
        &packed[RUNTIME_CRC_OFFSET + 4..],
        &image[RUNTIME_CRC_OFFSET + 4..]
    );
    let mut checked = packed;
    checked[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    assert_eq!(crc32(&checked), checksum);
    checked[100] ^= 1;
    assert_ne!(crc32(&checked), checksum);
}

#[test]
fn malformed_payload_is_rejected_without_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    for image in [vec![], vec![0x5a; 128]] {
        std::fs::write(&path, &image).unwrap();
        assert!(pack_runtime(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), image);
    }
}

#[test]
fn crc_matches_the_standard_check_vector() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
}
