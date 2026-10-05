use super::*;

#[test]
fn a_link_failure_is_retried_and_an_image_failure_is_not() {
    let mut calls = 0;
    let mut resets = Vec::new();
    let written = retry_transient(
        ATTEMPTS,
        &mut || {
            calls += 1;
            if calls == 1 {
                Err("espflash: Protocol error (os error 71)".into())
            } else {
                Ok(calls)
            }
        },
        |attempt, _| resets.push(attempt),
    );
    assert_eq!(written.unwrap(), 2);
    assert_eq!(resets, [1]);

    let mut calls = 0;
    let refused = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            Err("the HIL application overlaps the next flash region".into())
        },
        |_, _| {},
    );
    assert!(refused.is_err());
    assert_eq!(calls, 1);
}

#[test]
fn a_link_that_keeps_failing_ends_after_the_last_attempt() {
    let mut calls = 0;
    let mut resets = 0;
    let result = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            Err("serial port timed out".into())
        },
        |_, _| resets += 1,
    );
    assert!(result.is_err());
    assert_eq!(calls, ATTEMPTS);
    assert_eq!(resets, ATTEMPTS - 1);
}

#[test]
fn a_start_by_power_leaves_the_chip_in_its_rom() {
    assert_eq!(After::of(oer_chip_profile::Start::Reset), After::HardReset);
    assert_eq!(
        After::of(oer_chip_profile::Start::PowerOn),
        After::StayInBootloader
    );
}

#[test]
fn a_bundle_s_segments_end_with_its_ota_selection() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let profile = oer_image::profile(&root, "esp32s31").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut bundle =
        oer_image::ImageBundle::new(directory.path(), &profile, profile.flash.clone().unwrap());
    bundle.otadata = true;
    for (file, bytes) in [
        (bundle.bootloader(), &b"boot"[..]),
        (bundle.partitions(), b"table"),
        (bundle.application(), b"app"),
        (bundle.otadata().unwrap(), b"select"),
    ] {
        std::fs::write(file, bytes).unwrap();
    }
    let segments = Segment::of(&bundle).unwrap();
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.description)
            .collect::<Vec<_>>(),
        [
            "bootloader",
            "partition table",
            "application",
            "OTA selection"
        ]
    );
    assert_eq!(segments.last().unwrap().data, b"select");
    assert_eq!(segments.last().unwrap().offset, 0xd000);
}
