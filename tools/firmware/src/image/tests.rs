use super::*;
use std::time::{Duration, Instant};

#[test]
fn cancellation_harness() {
    let Some(stage) = env::var_os("OER_IMAGE_TEST_STAGE") else {
        return;
    };
    let _signals = oer_process::install_signal_handlers().unwrap();
    let unused = Path::new("unused-fixture-path");
    let error = if stage == "nm" {
        audit_runtime(unused, unused).unwrap_err()
    } else {
        audit_psram_stack_entry_instructions(unused).unwrap_err()
    };
    assert!(oer_process::is_cancelled(&*error), "{error}");
}

#[test]
fn cancellation_stops_both_placement_inspection_tools() {
    let directory = tempfile::tempdir().unwrap();
    let tool = directory.path().join("image-tool");
    let status = Command::new("rustc")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_tool.rs"))
        .arg("-o")
        .arg(&tool)
        .status()
        .unwrap();
    assert!(status.success());

    for stage in ["nm", "objdump"] {
        let marker = directory.path().join(stage);
        let mut harness = oer_process::owned::Child::spawn(
            Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "image::tests::cancellation_harness",
                    "--nocapture",
                ])
                .env("OER_IMAGE_TEST_STAGE", stage)
                .env("OER_IMAGE_TEST_READY", &marker)
                .env("LLVM_NM", &tool)
                .env("LLVM_OBJDUMP", &tool),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "inspection tool did not start");
            assert!(
                harness.try_wait().unwrap().is_none(),
                "harness exited early"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let marker = fs::read_to_string(marker).unwrap();
        let (parent, tool) = marker.split_once(' ').unwrap();
        assert!(
            Command::new("kill")
                .args(["-TERM", parent])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            harness
                .wait_timeout(Some(Duration::from_secs(5)))
                .unwrap()
                .success()
        );
        assert!(!Path::new(&format!("/proc/{tool}")).exists());
    }
}

#[test]
fn the_handler_word_is_read_at_its_address_in_its_section() {
    let table = "5032d878 g     O .rodata\t00000004 __oer_interrupt_body_SYSTIMER_TARGET2\n2f003a9e g     F .isr.text\t00000164 SYSTIMER_TARGET2\n";
    assert_eq!(
        body_words(table),
        [(
            "SYSTIMER_TARGET2".to_owned(),
            0x5032_d878,
            ".rodata".to_owned()
        )]
    );
    // Another section's row before it, and the word across two rows.
    let dump = "Contents of section .runtime.header:\n 50010000 53544732 01000000\nContents of section .rodata:\n 5032d870 00000000 00000000 ee37\n 5032d87a 1b50 0000\n";
    assert_eq!(dumped_word(dump, 0x5032_d878), Some(0x501b_37ee));
    assert_eq!(dumped_word(dump, 0x5032_d900), None);
}
