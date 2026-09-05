use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn image_signature(
    driver_observation: bool,
    task_poll: bool,
    rx_delivery: bool,
    mac_irq: bool,
    ieee802154_event_status: bool,
    ieee802154_ed_event: bool,
) -> ImageCapabilitySignature {
    ImageCapabilitySignature {
        driver_observation,
        task_poll,
        tx_architecture_probe: false,
        core0_rx_cycles: false,
        rx_delivery,
        mac_irq,
        ieee802154_event_status,
        ieee802154_ed_event,
        psram_task_stack: true,
    }
}

fn scratch_directory(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = env::temp_dir().join(format!(
        "open-esp-radio-hil-runner-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn qualified_profile_name_is_stable() {
    assert_eq!(QUALIFIED_PROFILE, "psram-code-psram-data");
    assert_eq!(TARGET, "riscv32imafc-unknown-none-elf");
}

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(crate::image::ImageClass::ALL.len(), 12);
    assert!(
        crate::image::ImageClass::ALL
            .into_iter()
            .all(crate::image::ImageClass::uses_psram_task_stack)
    );
    assert_eq!(crate::image::ImageClass::Performance.id(), "performance");
    assert_eq!(crate::image::ImageClass::Correctness.id(), "correctness");
    assert_eq!(
        crate::image::ImageClass::Correctness.runtime_features(),
        "open-radio-hil,driver-observation,psram-task-stack,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticMacIrq.runtime_features(),
        "open-radio-hil,psram-task-stack,mac-irq-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTaskResidence.runtime_features(),
        "open-radio-hil,psram-task-stack,task-residence-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTxArchitecture.runtime_features(),
        "open-radio-hil,psram-task-stack,tx-architecture-probes,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticTaskPoll.runtime_features(),
        "open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticCore0RxCoarse.runtime_features(),
        "open-radio-hil,psram-task-stack,core0-rx-coarse-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticCore0RxCycles.runtime_features(),
        "open-radio-hil,psram-task-stack,core0-rx-cycle-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticRxDelivery.runtime_features(),
        "open-radio-hil,psram-task-stack,rx-delivery-telemetry,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154EventStatus.runtime_features(),
        "open-radio-hil,ieee802154-event-status-probe,psram-task-stack,code-psram,profile-psram-data"
    );
    assert_eq!(
        crate::image::ImageClass::DiagnosticIeee802154EdEvent.runtime_features(),
        "open-radio-hil,ieee802154-ed-event-probe,psram-task-stack,code-psram,profile-psram-data"
    );
}

#[test]
fn image_capability_classifier_preserves_every_exclusive_class() {
    use crate::image::ImageClass;

    for (signals, expected) in [
        (
            image_signature(false, false, false, false, false, false),
            ImageClass::Performance,
        ),
        (
            image_signature(true, false, false, false, false, false),
            ImageClass::Correctness,
        ),
        (
            image_signature(true, false, false, true, false, false),
            ImageClass::DiagnosticMacIrq,
        ),
        (
            image_signature(false, true, false, false, false, false),
            ImageClass::DiagnosticTaskResidence,
        ),
        (
            image_signature(true, true, false, false, false, false),
            ImageClass::DiagnosticTaskPoll,
        ),
        (
            image_signature(true, false, true, false, false, false),
            ImageClass::DiagnosticRxDelivery,
        ),
        (
            image_signature(false, false, false, false, true, false),
            ImageClass::DiagnosticIeee802154EventStatus,
        ),
        (
            image_signature(false, false, false, false, false, true),
            ImageClass::DiagnosticIeee802154EdEvent,
        ),
    ] {
        assert_eq!(classify_image_signature(signals), Some(expected));
    }
    let mut tx_architecture = image_signature(false, true, false, false, false, false);
    tx_architecture.tx_architecture_probe = true;
    assert_eq!(
        classify_image_signature(tx_architecture),
        Some(ImageClass::DiagnosticTxArchitecture),
    );
    let mut core0_rx_cycles = image_signature(true, true, false, false, false, false);
    core0_rx_cycles.core0_rx_cycles = true;
    assert_eq!(
        classify_image_signature(core0_rx_cycles),
        Some(ImageClass::DiagnosticCore0RxCycles),
    );
    let mut core0_rx_coarse = image_signature(false, true, false, false, false, false);
    core0_rx_coarse.core0_rx_cycles = true;
    assert_eq!(
        classify_image_signature(core0_rx_coarse),
        Some(ImageClass::DiagnosticCore0RxCoarse),
    );
}

#[test]
fn image_capability_classifier_rejects_mixed_or_non_psram_images() {
    assert_eq!(
        classify_image_signature(image_signature(true, false, false, false, true, false)),
        None
    );
    assert_eq!(
        classify_image_signature(image_signature(false, true, false, false, true, false)),
        None
    );
    assert_eq!(
        classify_image_signature(image_signature(false, false, false, false, true, true)),
        None
    );

    let mut performance = image_signature(false, false, false, false, false, false);
    performance.psram_task_stack = false;
    assert_eq!(classify_image_signature(performance), None);
}

#[test]
fn tracked_file_snapshot_restores_exact_contents() {
    let directory = scratch_directory("restore");
    let lockfile = directory.join("Cargo.lock");
    let original = b"version = 4\n\n[[package]]\nname = \"fixture\"\n";
    fs::write(&lockfile, original).unwrap();

    let mut snapshot = TrackedFileSnapshot::capture(lockfile.clone()).unwrap();
    fs::write(&lockfile, b"rewritten by cargo\n").unwrap();
    snapshot.restore().unwrap();

    assert_eq!(fs::read(&lockfile).unwrap(), original);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tracked_file_snapshot_drop_removes_new_file() {
    let directory = scratch_directory("drop");
    let lockfile = directory.join("Cargo.lock");
    {
        let _snapshot = TrackedFileSnapshot::capture(lockfile.clone()).unwrap();
        fs::write(&lockfile, b"generated by cargo\n").unwrap();
    }

    assert!(!lockfile.exists());
    fs::remove_dir_all(directory).unwrap();
}
