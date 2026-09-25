//! Role scheduler scenarios over the validation hardware runtime.
#[allow(unused_imports)]
use crate::le::scanning::scheduler::PassiveScanScheduling;

use crate::{
    ControllerTimeSample,
    clock::ClockedResources,
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};

use oer_esp32s31_bluetooth_memory::{
    PassiveScanDefaultTxPowerDbm, PassiveScanMemoryGraphModelAddress,
    PassiveScanMemoryGraphStorage, PassiveScanPrimaryChannel, PassiveScanResetConfig,
    PassiveScanSchedulerAllocationConfig,
};

use oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime;

fn passive_scan_candidate() -> crate::le::scanning::scheduler::PassiveScanFirstEventCandidate {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(PassiveScanMemoryGraphStorage::new()));
    let base = PassiveScanMemoryGraphModelAddress::new(0x2f00_1000)
        .expect("the model base uses controller SRAM syntax");
    let reset = PassiveScanResetConfig::le_1m_public_accept_all(
        PassiveScanDefaultTxPowerDbm::new(0),
        BluetoothControllerLatchedTime::from_bits(10_000),
    );
    let allocation = PassiveScanSchedulerAllocationConfig::new(0, 0)
        .expect("the restricted product limits fit the scanner graph");
    let graph = PassiveScanMemoryGraphStorage::pin_static_model(storage, base, reset, allocation)
        .expect("the scanner graph fits physical controller SRAM");
    crate::le::scanning::scheduler::PassiveScanFirstEventCandidate::new(
        graph,
        PassiveScanPrimaryChannel::Channel37,
        crate::scheduler::SchedulerRawWindow::from_projected_scheduler_window(11_000, 12_000)
            .expect("the scanner window is non-empty and forward"),
        BluetoothControllerLatchedTime::from_bits(10_100),
    )
}

#[test]
fn passive_scanner_merge_cancellation_restores_both_cpu_owned_lists() {
    struct ScannerPlatform;

    let stopped =
        BluetoothStopped::from_hardware(ScannerPlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let admitted = task
        .admit_passive_scan_first_event(
            passive_scan_candidate(),
            crate::le::scanning::scheduler::PassiveScanAdmissionObservation {
                sample: ControllerTimeSample::for_validation(10_000),
            },
        )
        .unwrap_or_else(|_| panic!("the requested scanner window must be admitted"));
    let event = task
        .prepare_passive_scan_first_event(
            admitted,
            crate::le::scanning::scheduler::PassiveScanSequenceObservation {
                sample: ControllerTimeSample::for_validation(10_001),
            },
        )
        .unwrap_or_else(|_| panic!("the retained scanner deadline must remain open"));
    let channel = event.channel();
    let window = event.window();
    let merged = task
        .prepare_passive_scan_empty_list_merge(event)
        .unwrap_or_else(|_| panic!("the pristine common list must accept the scanner item"));
    let event = task
        .cancel_passive_scan_empty_list_merge(merged)
        .unwrap_or_else(|_| panic!("the same epoch must restore the scanner item"));
    assert_eq!(event.channel(), channel);
    assert_eq!(event.window(), window);

    let merged = task
        .prepare_passive_scan_empty_list_merge(event)
        .unwrap_or_else(|_| panic!("cancellation must reopen the common list"));
    let event = task
        .cancel_passive_scan_empty_list_merge(merged)
        .unwrap_or_else(|_| panic!("the restored private chain must remain cancellable"));
    assert_eq!(event.channel(), channel);
    assert_eq!(event.window(), window);
    let _graph = task.cancel_passive_scan_first_event(event);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn passive_scanner_pre_sequence_cancellation_releases_the_timeline() {
    struct ScannerPlatform;

    let stopped =
        BluetoothStopped::from_hardware(ScannerPlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let admitted = task
        .admit_passive_scan_first_event(
            passive_scan_candidate(),
            crate::le::scanning::scheduler::PassiveScanAdmissionObservation {
                sample: ControllerTimeSample::for_validation(10_000),
            },
        )
        .unwrap_or_else(|_| panic!("the first scanner candidate must be admitted"));
    let _graph = task.cancel_passive_scan_first_pre_sequence(admitted);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}
