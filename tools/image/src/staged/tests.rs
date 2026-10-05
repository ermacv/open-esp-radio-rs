use super::*;
use crate::bundle::{BootFiles, around};

fn repository() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A relocatable RV32 ELF whose only content is four bytes of `.text`.
pub(crate) fn elf() -> Vec<u8> {
    let code = [0x13, 0, 0, 0];
    let names = b"\0.text\0.shstrtab\0";
    let text = 52;
    let strings = text + code.len();
    let sections = (strings + names.len()).next_multiple_of(4);
    let mut out = Vec::with_capacity(sections + 120);
    out.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    for half in [1u16, 243] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    for word in [1u32, 0, 0, sections as u32, 1] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for half in [52u16, 0, 0, 40, 3, 2] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(names);
    out.resize(sections, 0);
    let header = |out: &mut Vec<u8>, fields: [u32; 10]| {
        for field in fields {
            out.extend_from_slice(&field.to_le_bytes());
        }
    };
    header(&mut out, [0; 10]);
    header(
        &mut out,
        [1, 1, 6, 0, text as u32, code.len() as u32, 0, 0, 2, 0],
    );
    header(
        &mut out,
        [7, 3, 0, 0, strings as u32, names.len() as u32, 0, 0, 1, 0],
    );
    out
}

#[test]
fn the_flash_map_is_the_partition_tables() {
    let root = repository();
    let profile = oer_chip_profile::Profile::load(&root, CHIP).unwrap();
    let flash = profile.flash.unwrap();
    let table = partitions(&root, &flash).unwrap();
    let application = table
        .iter()
        .find(|partition| partition.application)
        .unwrap();
    assert_eq!(
        (application.name.as_str(), application.offset),
        ("ota_0", flash.application)
    );
    let otadata = table.iter().find(|partition| partition.otadata).unwrap();
    assert_eq!(Some(otadata.offset), flash.otadata);
    assert!(flash.partition_table < otadata.offset);
    assert!(flash.bootloader < flash.partition_table);
}

#[test]
fn the_boot_files_are_encoded_into_the_bundle() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let application = directory.path().join("app.bin");
    std::fs::write(&application, b"application").unwrap();
    let elf_path = directory.path().join("bootstrap.elf");
    std::fs::write(&elf_path, elf()).unwrap();
    let output = directory.path().join("bundle");
    let bundle = around(
        &root,
        CHIP,
        &application,
        BootFiles::Encode { elf: &elf_path },
        &output,
    )
    .unwrap();
    // The ROM reads the bootloader in DIO, and it ends at its digest.
    let bootloader = std::fs::read(bundle.bootloader()).unwrap();
    assert_eq!(encode::rom_bootloader(&bootloader).unwrap(), bootloader);
    assert_eq!(bootloader[2], 2, "DIO");
    let csv = root.join(bundle.flash.partitions.as_ref().unwrap());
    assert_eq!(
        std::fs::read(bundle.partitions()).unwrap(),
        encode::partition_table(&csv).unwrap()
    );
    assert_eq!(
        std::fs::read(bundle.otadata().unwrap()).unwrap(),
        encode::ota_selector_image(0)
    );
    let segments = bundle
        .segments()
        .into_iter()
        .map(|segment| (segment.offset, segment.description))
        .collect::<Vec<_>>();
    assert_eq!(
        segments,
        [
            (0x2000, "bootloader"),
            (0x8000, "partition table"),
            (0x1_0000, "application"),
            (0xd000, "OTA selection"),
        ]
    );
    assert_eq!(crate::ImageBundle::load(&output).unwrap(), bundle);
    // The other chip's boot files are refused.
    assert!(
        around(
            &root,
            CHIP,
            &application,
            BootFiles::Given {
                bootloader: &application,
                partition_table: &application,
            },
            &directory.path().join("other"),
        )
        .is_err()
    );
}

#[test]
fn the_application_is_encoded_in_qio_and_the_bootloader_in_dio() {
    let root = repository();
    let profile = oer_chip_profile::Profile::load(&root, CHIP).unwrap();
    let csv = root.join(profile.flash.unwrap().partitions.unwrap());
    let chip = encode::chip(&profile.espflash_chip).unwrap();
    let application =
        encode::encode(&elf(), chip, APPLICATION, Some(&csv), 0x8000, Some("ota_0")).unwrap();
    let rom = encode::encode(&elf(), chip, ROM, Some(&csv), 0x8000, None).unwrap();
    // Byte 2 of an ESP image header is its flash mode: 0 QIO, 2 DIO.
    assert_eq!(application.application[2], 0);
    assert_eq!(rom.bootloader[2], 2);
    assert_eq!(application.partition_table, rom.partition_table);
}
