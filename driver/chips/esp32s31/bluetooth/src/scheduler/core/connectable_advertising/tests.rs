use open_esp_radio_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{AdvertisingInterval, LegacyAdvertisingData, PrimaryAdvertisingChannelMap},
    connectable_advertising::{
        LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
        LegacyConnectableAdvertisingSet, LegacyScanResponseData,
    },
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
    BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
    BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
    BluetoothPeripheralConnectionDefaultTxPowerDbm,
    BluetoothPeripheralConnectionMemoryGraphModelAddress,
    BluetoothPeripheralConnectionMemoryGraphStorage,
};

use super::{
    BluetoothLegacyConnectableAdvertisingAdmissionObservation,
    BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    BluetoothLegacyConnectableAdvertisingEventPreparationError,
    BluetoothLegacyConnectableAdvertisingEventPrepared,
    BluetoothLegacyConnectableAdvertisingSequenceObservation,
};
use crate::{
    BluetoothClockedResources, BluetoothControllerRuntimeResources,
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
    BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    BluetoothLegacyAdvertisingRecurringTimingObservation,
    BluetoothLegacyAdvertisingTimingObservation,
    BluetoothLegacyConnectableAdvertisingRuntimeResources,
    BluetoothPeripheralConnectionRuntimeConfig, BluetoothPeripheralConnectionRuntimeResources,
    BluetoothRadioHardware, BluetoothSchedulerInitialized, BluetoothSchedulerInstant,
    BluetoothSchedulerReservationError, BluetoothStopped,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingSetPrepared, refine_portable_set,
    },
};

struct TestPlatform;

fn scheduler<const CAPACITY: usize>() -> BluetoothSchedulerInitialized<TestPlatform, 1, CAPACITY> {
    let stopped =
        BluetoothStopped::from_hardware(TestPlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = BluetoothClockedResources::for_validation(registers, platform);
    clocked
        .initialize_controller_hal_with(|_, _| {})
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::new())
}

fn definition(kind: LeDeviceAddressKind) -> BluetoothLegacyConnectableAdvertisingSetPrepared {
    refine_portable_set(LegacyConnectableAdvertisingSet::new(
        LegacyConnectableAdvertisement::new(
            LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 0xc6], kind),
            LegacyAdvertisingData::new_owned(&[2, 1, 6]).unwrap(),
            LeChannelSelectionAlgorithmTwoSupport::Unsupported,
        ),
        LegacyScanResponseData::new_owned(&[3, 3, 0xaa, 0xfe]).unwrap(),
        PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
        AdvertisingInterval::new(32).unwrap(),
    ))
    .unwrap()
}

fn connectable_runtime(base: u32) -> BluetoothLegacyConnectableAdvertisingRuntimeResources {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::new(),
    ));
    BluetoothLegacyConnectableAdvertisingRuntimeResources::claim_static_model(
        storage,
        BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(base).unwrap(),
        BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(4),
    )
    .unwrap()
}

fn peripheral_runtime(base: u32) -> BluetoothPeripheralConnectionRuntimeResources {
    let graph = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothPeripheralConnectionMemoryGraphStorage::new(),
    ));
    let receive = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothNonScanningRxMemoryStorage::new(),
    ));
    BluetoothPeripheralConnectionRuntimeResources::claim_static_model(
        graph,
        BluetoothPeripheralConnectionMemoryGraphModelAddress::new(base).unwrap(),
        receive,
        BluetoothNonScanningRxMemoryModelAddress::new(base + 0x1000).unwrap(),
        BluetoothPeripheralConnectionRuntimeConfig::new(
            BluetoothPeripheralConnectionDefaultTxPowerDbm::new(4),
        ),
    )
    .unwrap()
}

fn candidate<const CAPACITY: usize>(
    scheduler: &BluetoothSchedulerInitialized<TestPlatform, 1, CAPACITY>,
    connectable: &mut BluetoothLegacyConnectableAdvertisingRuntimeResources,
    peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    current_micros: u32,
) -> BluetoothLegacyConnectableAdvertisingEventCandidate {
    let prepared = match connectable.begin_event(definition, peripheral) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the disjoint idle owners must prepare"),
    };
    prepared
        .form_first_event_candidate(
            BluetoothLegacyAdvertisingTimingObservation {
                current: BluetoothSchedulerInstant::from_image(current_micros),
                radio_ready: BluetoothSchedulerInstant::from_image(current_micros),
                epoch: BluetoothControllerSchedulerEpoch::new(
                    BluetoothControllerTimeSample::for_validation(100),
                    1_000,
                    scheduler.controller_time_scale(),
                ),
            },
            scheduler.scheduler_config(),
        )
        .unwrap_or_else(|failure| {
            let _prepared = failure.into_prepared();
            panic!("the bounded response window must project")
        })
}

fn restore(
    cancelled: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled,
    connectable: &mut BluetoothLegacyConnectableAdvertisingRuntimeResources,
    peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
) {
    let _definition = connectable
        .restore_cancelled(cancelled, peripheral)
        .unwrap_or_else(|_| panic!("the exact graph and receive pool must restore"));
    assert!(connectable.event_is_idle());
    assert!(peripheral.allocation_is_idle());
}

fn prepare<const CAPACITY: usize>(
    task: &mut crate::BluetoothControllerPoweredTaskRuntime<'_, CAPACITY>,
    candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
) -> BluetoothLegacyConnectableAdvertisingEventPrepared {
    let start = candidate.raw_window().start();
    let admitted = match task.admit_legacy_connectable_advertising_first_event(
        candidate,
        BluetoothLegacyConnectableAdvertisingAdmissionObservation {
            sample: BluetoothControllerTimeSample::for_validation(start.wrapping_sub(20_000)),
        },
    ) {
        Ok(admitted) => admitted,
        Err(_) => panic!("the guarded admission sample must remain early"),
    };
    match task.prepare_legacy_connectable_advertising_event(
        admitted,
        BluetoothLegacyConnectableAdvertisingSequenceObservation {
            sample: BluetoothControllerTimeSample::for_validation(start.wrapping_sub(10_000)),
        },
    ) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the independent sequence sample must remain early"),
    }
}

fn cancel_merge<const CAPACITY: usize>(
    task: &mut crate::BluetoothControllerPoweredTaskRuntime<'_, CAPACITY>,
    merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
) -> crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled {
    match task.cancel_legacy_connectable_advertising_empty_list_merge(merged) {
        Ok(cancelled) => cancelled,
        Err(_) => panic!("the originating list and receive pool must release"),
    }
}

#[test]
fn overlapping_candidate_rejection_retains_both_events_and_releases_cleanly() {
    let mut scheduler = scheduler::<1>();
    let mut first_connectable = connectable_runtime(0x2f00_1000);
    let mut first_peripheral = peripheral_runtime(0x2f00_3000);
    let mut second_connectable = connectable_runtime(0x2f00_6000);
    let mut second_peripheral = peripheral_runtime(0x2f00_8000);
    let first = candidate(
        &scheduler,
        &mut first_connectable,
        &mut first_peripheral,
        definition(LeDeviceAddressKind::Public),
        10_000,
    );
    let second = candidate(
        &scheduler,
        &mut second_connectable,
        &mut second_peripheral,
        definition(LeDeviceAddressKind::Random),
        10_000,
    );
    let first_start = first.raw_window().start();
    let second_start = second.raw_window().start();
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let first = match task.admit_legacy_connectable_advertising_first_event(
        first,
        BluetoothLegacyConnectableAdvertisingAdmissionObservation {
            sample: BluetoothControllerTimeSample::for_validation(first_start.wrapping_sub(20_000)),
        },
    ) {
        Ok(admitted) => admitted,
        Err(_) => panic!("the first response window must reserve the sole slot"),
    };
    let failure = match task.admit_legacy_connectable_advertising_first_event(
        second,
        BluetoothLegacyConnectableAdvertisingAdmissionObservation {
            sample: BluetoothControllerTimeSample::for_validation(
                second_start.wrapping_sub(20_000),
            ),
        },
    ) {
        Ok(_) => panic!("the overlapping event cannot enter the full timeline"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(
            BluetoothSchedulerReservationError::TimelineFull
        )
    );
    let second_cancelled = failure
        .into_candidate()
        .cancel()
        .unwrap_or_else(|_| panic!("the second event retains its exact pool"));
    let first_cancelled = task
        .cancel_legacy_connectable_advertising_pre_sequence(first)
        .unwrap_or_else(|_| panic!("the first event retains its exact pool"));
    restore(
        second_cancelled,
        &mut second_connectable,
        &mut second_peripheral,
    );
    restore(
        first_cancelled,
        &mut first_connectable,
        &mut first_peripheral,
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn rejected_sequence_sample_releases_timeline_and_all_graph_owners() {
    let mut scheduler = scheduler::<1>();
    let mut connectable = connectable_runtime(0x2f00_b000);
    let mut peripheral = peripheral_runtime(0x2f00_d000);
    let candidate = candidate(
        &scheduler,
        &mut connectable,
        &mut peripheral,
        definition(LeDeviceAddressKind::Public),
        10_000,
    );
    let start = candidate.raw_window().start();
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let admitted = match task.admit_legacy_connectable_advertising_first_event(
        candidate,
        BluetoothLegacyConnectableAdvertisingAdmissionObservation {
            sample: BluetoothControllerTimeSample::for_validation(start.wrapping_sub(20_000)),
        },
    ) {
        Ok(admitted) => admitted,
        Err(_) => panic!("the first guarded sample must remain early"),
    };
    let failure = match task.prepare_legacy_connectable_advertising_event(
        admitted,
        BluetoothLegacyConnectableAdvertisingSequenceObservation {
            sample: BluetoothControllerTimeSample::for_validation(start),
        },
    ) {
        Ok(_) => panic!("a sample at the event start must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothLegacyConnectableAdvertisingEventPreparationError::Sequence(
            crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired
        )
    );
    let cancelled = failure
        .into_candidate()
        .cancel()
        .unwrap_or_else(|_| panic!("the rejected event retains its exact pool"));
    restore(cancelled, &mut connectable, &mut peripheral);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn phase_locked_connectable_recurrence_reserves_and_cancels_losslessly() {
    let mut scheduler = scheduler::<1>();
    let mut connectable = connectable_runtime(0x2f00_e000);
    let mut peripheral = peripheral_runtime(0x2f01_0000);
    let definition = definition(LeDeviceAddressKind::Public);
    let first = candidate(
        &scheduler,
        &mut connectable,
        &mut peripheral,
        definition,
        10_000,
    );
    let previous_phase = first.phase();
    let cancelled = first
        .cancel()
        .unwrap_or_else(|_| panic!("the first candidate retains the exact receive pool"));
    restore(cancelled, &mut connectable, &mut peripheral);

    let prepared = connectable
        .begin_event(definition, &mut peripheral)
        .unwrap_or_else(|_| panic!("the restored runtime accepts the successor"));
    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scheduler.controller_time_scale(),
    );
    let recurring = prepared
        .form_recurring_event_candidate(
            BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
            previous_phase,
            definition.set().interval().as_micros(),
            scheduler.scheduler_config(),
        )
        .unwrap_or_else(|failure| {
            let _prepared = failure.into_prepared();
            panic!("one selected-channel successor fits the retained epoch")
        });
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let admitted = task
        .admit_legacy_connectable_advertising_recurring_event(recurring)
        .unwrap_or_else(|failure| {
            let _candidate = failure.into_candidate();
            panic!("an empty timeline accepts the phase-locked successor")
        });
    let cancelled = task
        .cancel_legacy_connectable_advertising_pre_sequence(admitted)
        .unwrap_or_else(|_| panic!("recurring cancellation returns every affine owner"));
    restore(cancelled, &mut connectable, &mut peripheral);
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn colliding_connectable_recurrence_keeps_both_role_graphs_and_nominal_phase() {
    let mut scheduler = scheduler::<2>();
    let first_definition = definition(LeDeviceAddressKind::Public);
    let second_definition = definition(LeDeviceAddressKind::Random);
    let mut first_connectable = connectable_runtime(0x2f02_0000);
    let mut first_peripheral = peripheral_runtime(0x2f02_2000);
    let mut second_connectable = connectable_runtime(0x2f02_5000);
    let mut second_peripheral = peripheral_runtime(0x2f02_7000);

    let first_initial = candidate(
        &scheduler,
        &mut first_connectable,
        &mut first_peripheral,
        first_definition,
        10_000,
    );
    let previous_phase = first_initial.phase();
    let first_cancelled = first_initial
        .cancel()
        .unwrap_or_else(|_| panic!("the first initial candidate remains CPU-owned"));
    restore(
        first_cancelled,
        &mut first_connectable,
        &mut first_peripheral,
    );

    let second_initial = candidate(
        &scheduler,
        &mut second_connectable,
        &mut second_peripheral,
        second_definition,
        10_000,
    );
    assert_eq!(second_initial.phase(), previous_phase);
    let second_cancelled = second_initial
        .cancel()
        .unwrap_or_else(|_| panic!("the second initial candidate remains CPU-owned"));
    restore(
        second_cancelled,
        &mut second_connectable,
        &mut second_peripheral,
    );

    let epoch = BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        1_000,
        scheduler.controller_time_scale(),
    );
    let start_offset_micros = first_definition.set().interval().as_micros();
    let first = first_connectable
        .begin_event(first_definition, &mut first_peripheral)
        .unwrap_or_else(|_| panic!("the first restored runtime accepts its successor"))
        .form_recurring_event_candidate(
            BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
            previous_phase,
            start_offset_micros,
            scheduler.scheduler_config(),
        )
        .unwrap_or_else(|failure| {
            let _prepared = failure.into_prepared();
            panic!("the first successor projects from its nominal phase")
        });
    let second = second_connectable
        .begin_event(second_definition, &mut second_peripheral)
        .unwrap_or_else(|_| panic!("the second restored runtime accepts its successor"))
        .form_recurring_event_candidate(
            BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
            previous_phase,
            start_offset_micros,
            scheduler.scheduler_config(),
        )
        .unwrap_or_else(|failure| {
            let _prepared = failure.into_prepared();
            panic!("the second successor projects from the same nominal phase")
        });
    let recurring_phase = first.phase();
    assert_eq!(second.phase(), recurring_phase);

    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let first = task
        .admit_legacy_connectable_advertising_recurring_event(first)
        .unwrap_or_else(|failure| {
            let _candidate = failure.into_candidate();
            panic!("the first recurring event reserves the empty timeline")
        });
    let failure = match task.admit_legacy_connectable_advertising_recurring_event(second) {
        Ok(_) => panic!("a phase-locked collision cannot be displaced"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(
            BluetoothSchedulerReservationError::RecurringOverlapUnsupported
        )
    );
    assert_eq!(failure.candidate.phase(), recurring_phase);

    let second_cancelled = failure
        .into_candidate()
        .cancel()
        .unwrap_or_else(|_| panic!("collision rejection retains the second graph"));
    let first_cancelled = task
        .cancel_legacy_connectable_advertising_pre_sequence(first)
        .unwrap_or_else(|_| panic!("the accepted reservation retains the first graph"));
    restore(
        first_cancelled,
        &mut first_connectable,
        &mut first_peripheral,
    );
    restore(
        second_cancelled,
        &mut second_connectable,
        &mut second_peripheral,
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn occupied_list_rejects_second_item_without_losing_either_owner() {
    let mut scheduler = scheduler::<2>();
    let mut first_connectable = connectable_runtime(0x2f01_0000);
    let mut first_peripheral = peripheral_runtime(0x2f01_2000);
    let mut second_connectable = connectable_runtime(0x2f01_5000);
    let mut second_peripheral = peripheral_runtime(0x2f01_7000);
    let first = candidate(
        &scheduler,
        &mut first_connectable,
        &mut first_peripheral,
        definition(LeDeviceAddressKind::Random),
        10_000,
    );
    let second = candidate(
        &scheduler,
        &mut second_connectable,
        &mut second_peripheral,
        definition(LeDeviceAddressKind::Public),
        30_000,
    );
    let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
    let first = prepare(&mut task, first);
    let second = prepare(&mut task, second);
    let first = match task.prepare_legacy_connectable_advertising_empty_list_merge(first) {
        Ok(merged) => merged,
        Err(_) => panic!("the pristine list must accept its first item"),
    };
    let failure = match task.prepare_legacy_connectable_advertising_empty_list_merge(second) {
        Ok(_) => panic!("the exclusive list must reject a second first item"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        super::BluetoothSchedulerEmptyListMergeError::ListNotEmpty
    );
    let second = failure.into_prepared();
    let first_cancelled = cancel_merge(&mut task, first);
    let second = match task.prepare_legacy_connectable_advertising_empty_list_merge(second) {
        Ok(merged) => merged,
        Err(_) => panic!("cancelling the first item must reopen the exact empty list"),
    };
    let second_cancelled = cancel_merge(&mut task, second);
    restore(
        first_cancelled,
        &mut first_connectable,
        &mut first_peripheral,
    );
    restore(
        second_cancelled,
        &mut second_connectable,
        &mut second_peripheral,
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}
