use oer_esp32s31_platform_layout::stage_two::{self, HEADER_BYTES, Header};
use oer_firmware::pack_runtime;

fn image() -> Vec<u8> {
    let header = Header {
        magic: stage_two::MAGIC,
        abi_version: stage_two::ABI_VERSION,
        load_address: 0x1000,
        entry: 0x1040,
        payload_end: 0x1080,
        bss_start: 0x1080,
        bss_end: 0x1080,
        header_size: HEADER_BYTES as u32,
        text_start: 0x1030,
        text_end: 0x1080,
        payload_crc32: 0,
    };
    let mut image = vec![0x5a; 128];
    image[..HEADER_BYTES].copy_from_slice(&header.to_le_bytes());
    image
}

#[test]
fn packing_stores_the_payload_crc_and_is_repeatable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    let image = image();
    std::fs::write(&path, &image).unwrap();
    let checksum = pack_runtime(&path).unwrap();
    let packed = std::fs::read(&path).unwrap();
    assert_eq!(checksum, pack_runtime(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), packed);
    let stored = Header::from_le_bytes(&packed).unwrap();
    assert_eq!(stored.payload_crc32, checksum);
    assert_eq!(
        Header {
            payload_crc32: 0,
            ..stored
        },
        Header::from_le_bytes(&image).unwrap()
    );
    assert_eq!(packed[HEADER_BYTES..], image[HEADER_BYTES..]);
    assert_eq!(stage_two::payload_crc32(&packed), checksum);
}

#[test]
fn malformed_payload_is_rejected_without_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.bin");
    let mut incompatible = image();
    let mut header = Header::from_le_bytes(&incompatible).unwrap();
    header.abi_version += 1;
    incompatible[..HEADER_BYTES].copy_from_slice(&header.to_le_bytes());
    for image in [vec![], vec![0x5a; 128], incompatible] {
        std::fs::write(&path, &image).unwrap();
        assert!(pack_runtime(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), image);
    }
}
