use super::*;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn an_esp_idf_bundle_takes_its_catalog_boot_files_and_has_no_ota_selection() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let application = file("app.bin", b"application");
    let bootloader = file("bootloader.bin", b"bootloader");
    let table = file("partition-table.bin", b"table");
    let output = directory.path().join("bundle");
    let bundle = around(
        &root,
        "esp32c5",
        &application,
        BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &table,
        },
        &output,
    )
    .unwrap();
    assert_eq!(bundle.boot, Boot::EspIdfBootloader);
    assert_eq!(bundle.otadata(), None);
    let segments = bundle.segments();
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.offset, std::fs::read(&segment.file).unwrap()))
            .collect::<Vec<_>>(),
        [
            (0x2000, b"bootloader".to_vec()),
            (0x8000, b"table".to_vec()),
            (0x1_0000, b"application".to_vec()),
        ]
    );
    // A loaded bundle is the written one; one without its files is refused.
    assert_eq!(ImageBundle::load(&output).unwrap(), bundle);
    std::fs::remove_file(bundle.partitions()).unwrap();
    let error = ImageBundle::load(&output).unwrap_err().to_string();
    assert!(error.contains("partition table"), "{error}");
    let elf = file("app.elf", b"");
    assert!(
        around(
            &root,
            "esp32c5",
            &application,
            BootFiles::Encode { elf: &elf },
            &directory.path().join("other"),
        )
        .is_err()
    );
}

#[test]
fn a_run_flashes_a_copy_holding_exactly_its_archived_application() {
    let root = repository();
    let directory = tempfile::tempdir().unwrap();
    let file = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let application = file("app.bin", b"application");
    let bootloader = file("bootloader.bin", b"bootloader");
    let table = file("table.bin", b"table");
    let built = around(
        &root,
        "esp32c5",
        &application,
        BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &table,
        },
        &directory.path().join("built"),
    )
    .unwrap();
    let archived = file("archived.bin", b"application");
    let copy = built
        .with_application(&archived, &directory.path().join("run"))
        .unwrap();
    assert_eq!(std::fs::read(copy.application()).unwrap(), b"application");
    assert_eq!(std::fs::read(copy.bootloader()).unwrap(), b"bootloader");
    assert_eq!(ImageBundle::load(&copy.directory).unwrap(), copy);
    let other = file("other.bin", b"other");
    assert!(
        built
            .with_application(&other, &directory.path().join("refused"))
            .is_err()
    );
}
