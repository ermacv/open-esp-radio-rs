use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample, SchedulerInstant,
    le::advertising::LegacyAdvertisingTimingObservation, scheduler::SchedulerSoftwareConfig,
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

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    LegacyAdvertisingCancelledRestoreOutcome, LegacyAdvertisingDefaultTxPowerDbm,
    LegacyAdvertisingFirstEventCandidateOutcome, LegacyAdvertisingLinkStateResetOutcome,
    LegacyAdvertisingPrepared, LegacyAdvertisingRuntimeBeginError,
    LegacyAdvertisingRuntimeResources,
};

fn advertising_set(
    channels: PrimaryAdvertisingChannelMap,
) -> LegacyNonconnectableAdvertisingSet<'static> {
    let advertisement = LegacyNonconnectableAdvertisement::new(
        LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
        LegacyAdvertisingData::new(&[2, 1, 6]).expect("the fixed data fits legacy advertising"),
    );
    LegacyNonconnectableAdvertisingSet::new(
        advertisement,
        channels,
        AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS)
            .expect("the minimum interval is valid"),
    )
}

fn enabled(
    channels: PrimaryAdvertisingChannelMap,
) -> oer_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
    LegacyAdvertiserStandby::new()
        .configure(advertising_set(channels))
        .enable()
        .expect("the first generation is available")
}

fn memory() -> LegacyAdvertisingMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        LegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = LegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    LegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
        .expect("the advertising graph fits physical controller SRAM")
}

fn runtime_at(base: u32) -> LegacyAdvertisingRuntimeResources {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        LegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = LegacyAdvertisingMemoryGraphModelAddress::new(base)
        .expect("the model base uses controller SRAM syntax");
    LegacyAdvertisingRuntimeResources::claim_static_model(
        storage,
        base,
        LegacyAdvertisingDefaultTxPowerDbm::new(6),
    )
    .expect("the advertising runtime graph fits controller SRAM")
}

#[test]
fn runtime_checks_out_and_restores_only_its_exact_graph() {
    let mut runtime = runtime_at(0x2f00_1000);
    assert_eq!(runtime.default_tx_power_dbm().dbm(), 6);
    assert!(runtime.event_is_idle());
    let event = runtime
        .begin_event(advertising_set(PrimaryAdvertisingChannelMap::all()))
        .expect("the idle advertiser and graph check out once");
    let (prepared, power) = event.into_parts();
    assert_eq!(power.dbm(), 6);
    assert_eq!(prepared.identity().generation().get(), 1);
    assert!(matches!(
        runtime.begin_event(advertising_set(PrimaryAdvertisingChannelMap::all())),
        Err(LegacyAdvertisingRuntimeBeginError::EventActive)
    ));
    assert!(matches!(
        runtime.restore_cancelled(prepared.cancel()),
        LegacyAdvertisingCancelledRestoreOutcome::Restored
    ));
    assert!(runtime.event_is_idle());

    let event = runtime
        .begin_event(advertising_set(PrimaryAdvertisingChannelMap::all()))
        .expect("the restored advertiser starts the next generation");
    let (prepared, _) = event.into_parts();
    assert_eq!(prepared.identity().generation().get(), 2);
    assert!(matches!(
        runtime.restore_cancelled(prepared.cancel()),
        LegacyAdvertisingCancelledRestoreOutcome::Restored
    ));

    let mut foreign = runtime_at(0x2f00_2000);
    let foreign_event = foreign
        .begin_event(advertising_set(PrimaryAdvertisingChannelMap::all()))
        .expect("the foreign runtime checks out");
    let (foreign_prepared, _) = foreign_event.into_parts();
    let foreign_cancelled = match runtime.restore_cancelled(foreign_prepared.cancel()) {
        LegacyAdvertisingCancelledRestoreOutcome::Rejected(cancelled) => cancelled,
        LegacyAdvertisingCancelledRestoreOutcome::Restored => {
            panic!("a different graph identity cannot enter this runtime")
        }
    };
    assert!(matches!(
        foreign.restore_cancelled(foreign_cancelled),
        LegacyAdvertisingCancelledRestoreOutcome::Restored
    ));
}

#[test]
fn preparation_retains_identity_and_cancel_restores_the_same_event() {
    let prepared =
        LegacyAdvertisingPrepared::prepare(enabled(PrimaryAdvertisingChannelMap::all()), memory())
            .expect("bounded validated advertising data always fits the chip PDU");
    let identity = prepared.identity();

    assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
    assert_eq!(prepared.channels(), PrimaryAdvertisingChannelMap::all());
    let (enabled, _memory) = prepared.cancel().into_parts();
    assert_eq!(enabled.prepare_event().identity(), identity);
}

#[test]
fn portable_primary_channel_plan_survives_chip_preparation() {
    for channels in [
        PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
        PrimaryAdvertisingChannelMap::new(false, true, true).unwrap(),
        PrimaryAdvertisingChannelMap::all(),
    ] {
        let prepared = LegacyAdvertisingPrepared::prepare(enabled(channels), memory())
            .expect("bounded validated advertising data always fits the chip PDU");
        assert_eq!(prepared.channels(), channels);
    }
}

#[test]
fn reset_retains_the_exact_protocol_work_and_remains_cancellable() {
    let prepared =
        LegacyAdvertisingPrepared::prepare(enabled(PrimaryAdvertisingChannelMap::all()), memory())
            .expect("bounded validated advertising data always fits the chip PDU");
    let identity = prepared.identity();
    let reset = match prepared.reset_link_state(LegacyAdvertisingDefaultTxPowerDbm::new(0)) {
        LegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        LegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the portable producer emits the restricted PDU form")
        }
    };

    assert_eq!(reset.identity(), identity);
    assert_eq!(reset.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
    let (enabled, memory) = reset.cancel().into_parts();
    assert_eq!(enabled.prepare_event().identity(), identity);
    assert!(memory.prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6]).is_ok());
}

#[test]
fn sealed_live_timing_forms_a_cancellable_first_event_candidate() {
    let prepared =
        LegacyAdvertisingPrepared::prepare(enabled(PrimaryAdvertisingChannelMap::all()), memory())
            .expect("bounded validated advertising data always fits the chip PDU");
    let reset = match prepared.reset_link_state(LegacyAdvertisingDefaultTxPowerDbm::new(0)) {
        LegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        LegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the portable producer emits the restricted PDU form")
        }
    };
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let timing = LegacyAdvertisingTimingObservation {
        current: SchedulerInstant::from_image(10_000),
        radio_ready: SchedulerInstant::from_image(11_999),
        epoch: ControllerSchedulerEpoch::new(
            ControllerTimeSample::for_validation(100),
            1_000,
            scale,
        ),
    };
    let candidate = match reset
        .form_first_event_candidate(timing, SchedulerSoftwareConfig::reviewed_standalone())
    {
        LegacyAdvertisingFirstEventCandidateOutcome::Candidate(candidate) => candidate,
        LegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(_) => {
            panic!("the reviewed timing window projects into one raw epoch")
        }
    };

    assert_eq!(candidate.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
    assert_eq!(candidate.projected_window_duration(), 1_554);
    let (enabled, _) = candidate.cancel().into_parts();
    assert_eq!(
        enabled.prepare_event().channels(),
        PrimaryAdvertisingChannelMap::all()
    );
}
