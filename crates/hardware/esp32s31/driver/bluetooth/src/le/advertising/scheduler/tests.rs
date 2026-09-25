//! Role scheduler scenarios over the validation hardware runtime.
#[allow(unused_imports)]
use crate::le::advertising::scheduler::LegacyAdvertisingScheduling;

use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample, SchedulerInstant,
    clock::ClockedResources,
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};

use oer_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertiser::LegacyAdvertiserStandby,
    advertising::{
        AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
        LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannelMap,
    },
};

use oer_esp32s31_bluetooth_memory::{
    LegacyAdvertisingMemoryGraphCpuOwned, LegacyAdvertisingMemoryGraphModelAddress,
    LegacyAdvertisingMemoryGraphStorage,
};

fn legacy_advertiser_enabled() -> oer_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
    let advertisement = LegacyNonconnectableAdvertisement::new(
        LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
        LegacyAdvertisingData::new(&[2, 1, 6]).expect("the fixed data fits"),
    );
    LegacyAdvertiserStandby::new()
        .configure(LegacyNonconnectableAdvertisingSet::new(
            advertisement,
            PrimaryAdvertisingChannelMap::all(),
            AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS)
                .expect("the minimum interval is valid"),
        ))
        .enable()
        .expect("the first generation is available")
}

fn legacy_advertising_memory() -> LegacyAdvertisingMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        LegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = LegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    LegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
        .expect("the advertising graph fits physical controller SRAM")
}

#[test]
fn first_advertising_event_uses_common_admission_and_cancels_losslessly() {
    struct AdvertisingPlatform;

    let stopped = BluetoothStopped::from_hardware(
        AdvertisingPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let scale = scheduler.controller_time_scale();
    let config = scheduler.scheduler_config();
    let prepared = crate::le::advertising::LegacyAdvertisingPrepared::prepare(
        legacy_advertiser_enabled(),
        legacy_advertising_memory(),
    )
    .expect("the bounded portable packet fits");
    let reset = match prepared
        .reset_link_state(crate::le::advertising::LegacyAdvertisingDefaultTxPowerDbm::new(0))
    {
        crate::le::advertising::LegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        crate::le::advertising::LegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the portable packet selects the restricted reset")
        }
    };
    let candidate = match reset.form_first_event_candidate(
        crate::le::advertising::LegacyAdvertisingTimingObservation {
            current: SchedulerInstant::from_image(10_000),
            radio_ready: SchedulerInstant::from_image(11_999),
            epoch: ControllerSchedulerEpoch::new(
                ControllerTimeSample::for_validation(100),
                1_000,
                scale,
            ),
        },
        config,
    ) {
        crate::le::advertising::LegacyAdvertisingFirstEventCandidateOutcome::Candidate(
            candidate,
        ) => candidate,
        crate::le::advertising::LegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(_) => {
            panic!("the first event projects into the retained epoch")
        }
    };
    let identity = candidate.identity();
    let raw_start = candidate.raw_window().start();

    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let admitted = task
        .admit_legacy_advertising_first_event(
            candidate,
            crate::le::advertising::scheduler::LegacyAdvertisingAdmissionObservation {
                sample: ControllerTimeSample::for_validation(raw_start.wrapping_sub(200)),
            },
        )
        .expect("the first guarded deadline remains open");
    let (enabled, memory) = task
        .cancel_legacy_advertising_first_pre_sequence(admitted)
        .into_parts();
    let prepared = crate::le::advertising::LegacyAdvertisingPrepared::prepare(enabled, memory)
        .expect("the cancelled portable packet remains bounded");
    let reset = match prepared
        .reset_link_state(crate::le::advertising::LegacyAdvertisingDefaultTxPowerDbm::new(0))
    {
        crate::le::advertising::LegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        crate::le::advertising::LegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the cancelled packet retains the restricted reset")
        }
    };
    let candidate = match reset.form_first_event_candidate(
        crate::le::advertising::LegacyAdvertisingTimingObservation {
            current: SchedulerInstant::from_image(10_000),
            radio_ready: SchedulerInstant::from_image(11_999),
            epoch: ControllerSchedulerEpoch::new(
                ControllerTimeSample::for_validation(100),
                1_000,
                scale,
            ),
        },
        config,
    ) {
        crate::le::advertising::LegacyAdvertisingFirstEventCandidateOutcome::Candidate(
            candidate,
        ) => candidate,
        crate::le::advertising::LegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(_) => {
            panic!("the restored first event projects into the same epoch")
        }
    };
    let raw_start = candidate.raw_window().start();
    let admitted = task
        .admit_legacy_advertising_first_event(
            candidate,
            crate::le::advertising::scheduler::LegacyAdvertisingAdmissionObservation {
                sample: ControllerTimeSample::for_validation(raw_start.wrapping_sub(200)),
            },
        )
        .expect("cancellation released the first guarded reservation");
    let prepared = task
        .prepare_legacy_advertising_first_event(
            admitted,
            crate::le::advertising::scheduler::LegacyAdvertisingSequenceObservation {
                sample: ControllerTimeSample::for_validation(raw_start.wrapping_sub(100)),
            },
        )
        .expect("the second guarded deadline remains open");
    assert_eq!(prepared.identity(), identity);
    assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);

    let merged = match task.prepare_legacy_advertising_empty_list_merge(prepared) {
        Ok(merged) => merged,
        Err(_) => panic!("the pristine exclusive list must accept the advertising item"),
    };
    let prepared = match task.cancel_legacy_advertising_empty_list_merge(merged) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the same scheduler epoch must restore the unpublished event"),
    };
    let cancelled = task.cancel_legacy_advertising_first_event(prepared);
    let (enabled, memory) = cancelled.into_parts();
    assert_eq!(enabled.prepare_event().identity(), identity);
    assert!(memory.prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6]).is_ok());
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}
