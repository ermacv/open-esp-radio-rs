use super::*;

#[test]
fn the_ota_0_selector_is_a_valid_idf_entry() {
    let image = ota_selector_image(0);
    assert_eq!(u32::from_le_bytes(image[0..4].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(image[24..28].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(image[28..32].try_into().unwrap()),
        crc32_idf(&1_u32.to_le_bytes())
    );
    assert!(image[32..].iter().all(|byte| *byte == 0xff));
}

#[test]
fn the_second_slot_is_selected_by_the_next_sequence_number() {
    let image = ota_selector_image(1);
    // The bootloader boots slot (sequence - 1) modulo the slot count.
    assert_eq!(u32::from_le_bytes(image[0..4].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(image[28..32].try_into().unwrap()),
        crc32_idf(&2_u32.to_le_bytes())
    );
}

/// A one-segment hashed DIO image followed by erased flash.
fn rom_image() -> Vec<u8> {
    let mut image = vec![0xff; 0x100];
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
    image
}

#[test]
fn the_rom_bootloader_ends_at_its_verified_digest() {
    let image = rom_image();
    assert_eq!(rom_bootloader(&image).unwrap(), &image[..80]);
}

#[test]
fn the_rom_bootloader_rejects_qio_corruption_and_out_of_bounds_segments() {
    let valid = rom_image();
    for offset in [2, 32, 47, 48] {
        let mut corrupt = valid.clone();
        corrupt[offset] ^= 2;
        assert!(rom_bootloader(&corrupt).is_err());
    }
    let mut truncated = valid;
    truncated[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(rom_bootloader(&truncated).is_err());
    assert!(rom_bootloader(&[]).is_err());
}

#[test]
fn a_nearly_full_partition_warns() {
    let capacity = 0xff_0000;
    assert!(partition_budget_warning(capacity * 89 / 100, capacity).is_none());
    assert!(partition_budget_warning(capacity.div_ceil(100) * 90, capacity).is_some());
}

#[test]
fn a_partition_table_csv_reads_and_encodes() {
    let directory = tempfile::tempdir().unwrap();
    let csv = directory.path().join("table.csv");
    std::fs::write(
        &csv,
        "# Name, Type, SubType, Offset, Size, Flags\n\
         nvs, data, nvs, 0x9000, 0x4000,\n\
         otadata, data, ota, 0xd000, 0x2000,\n\
         ota_0, app, ota_0, 0x10000, 0x100000,\n",
    )
    .unwrap();
    let table = partitions(&csv).unwrap();
    assert_eq!(
        table
            .iter()
            .map(|p| (p.name.as_str(), p.offset, p.application, p.otadata))
            .collect::<Vec<_>>(),
        [
            ("nvs", 0x9000, false, false),
            ("otadata", 0xd000, false, true),
            ("ota_0", 0x10000, true, false),
        ]
    );
    let binary = partition_table(&csv).unwrap();
    // Three 32-byte entries, then the MD5 entry and erased flash.
    assert_eq!(&binary[..2], &[0xaa, 0x50]);
    assert_eq!(binary.len(), 0xc00);
}

#[test]
fn an_image_is_the_chip_s_only_with_its_chip_id() {
    let chips = [Chip::Esp32c3, Chip::Esp32c6];
    let mut image = rom_image();
    image[12..14].copy_from_slice(&chips[0].id().to_le_bytes());
    require_chip_image(&image, chips[0]).unwrap();
    let error = require_chip_image(&image, chips[1])
        .unwrap_err()
        .to_string();
    assert!(error.contains("chip id"), "{error}");
    assert!(require_chip_image(b"application", chips[0]).is_err());
    let mut no_segments = image.clone();
    no_segments[1] = 0;
    assert!(require_chip_image(&no_segments, chips[0]).is_err());
}

#[test]
fn only_a_binary_partition_table_is_one() {
    let csv = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        csv.path(),
        "nvs, data, nvs, 0x9000, 0x6000,\nfactory, app, factory, 0x10000, 0x100000,\n",
    )
    .unwrap();
    require_partition_table(&partition_table(csv.path()).unwrap()).unwrap();
    assert!(require_partition_table(b"application").is_err());
    assert!(require_partition_table(&rom_image()).is_err());
}
