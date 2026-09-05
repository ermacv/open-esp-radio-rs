use open_esp_radio_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertiser::LegacyAdvertiserStandby,
    advertising::{
        AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
        LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannelMap,
    },
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyAdvertisingMemoryGraphModelAddress,
    BluetoothLegacyAdvertisingMemoryGraphStorage,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    BluetoothLegacyAdvertisingCancelledRestoreOutcome, BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    BluetoothLegacyAdvertisingFirstEventCandidateOutcome,
    BluetoothLegacyAdvertisingLinkStateResetOutcome, BluetoothLegacyAdvertisingPrepared,
    BluetoothLegacyAdvertisingRuntimeBeginError, BluetoothLegacyAdvertisingRuntimeResources,
};
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
    BluetoothLegacyAdvertisingTimingObservation, BluetoothSchedulerInstant,
    BluetoothSchedulerSoftwareConfig,
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
) -> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
    LegacyAdvertiserStandby::new()
        .configure(advertising_set(channels))
        .enable()
        .expect("the first generation is available")
}

fn memory() -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
        .expect("the model base uses controller SRAM syntax");
    BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
        .expect("the advertising graph fits physical controller SRAM")
}

fn runtime_at(base: u32) -> BluetoothLegacyAdvertisingRuntimeResources {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
    ));
    let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(base)
        .expect("the model base uses controller SRAM syntax");
    BluetoothLegacyAdvertisingRuntimeResources::claim_static_model(
        storage,
        base,
        BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(6),
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
        Err(BluetoothLegacyAdvertisingRuntimeBeginError::EventActive)
    ));
    assert!(matches!(
        runtime.restore_cancelled(prepared.cancel()),
        BluetoothLegacyAdvertisingCancelledRestoreOutcome::Restored
    ));
    assert!(runtime.event_is_idle());

    let event = runtime
        .begin_event(advertising_set(PrimaryAdvertisingChannelMap::all()))
        .expect("the restored advertiser starts the next generation");
    let (prepared, _) = event.into_parts();
    assert_eq!(prepared.identity().generation().get(), 2);
    assert!(matches!(
        runtime.restore_cancelled(prepared.cancel()),
        BluetoothLegacyAdvertisingCancelledRestoreOutcome::Restored
    ));

    let mut foreign = runtime_at(0x2f00_2000);
    let foreign_event = foreign
        .begin_event(advertising_set(PrimaryAdvertisingChannelMap::all()))
        .expect("the foreign runtime checks out");
    let (foreign_prepared, _) = foreign_event.into_parts();
    let foreign_cancelled = match runtime.restore_cancelled(foreign_prepared.cancel()) {
        BluetoothLegacyAdvertisingCancelledRestoreOutcome::Rejected(cancelled) => cancelled,
        BluetoothLegacyAdvertisingCancelledRestoreOutcome::Restored => {
            panic!("a different graph identity cannot enter this runtime")
        }
    };
    assert!(matches!(
        foreign.restore_cancelled(foreign_cancelled),
        BluetoothLegacyAdvertisingCancelledRestoreOutcome::Restored
    ));
}

#[test]
fn preparation_retains_identity_and_cancel_restores_the_same_event() {
    let prepared = BluetoothLegacyAdvertisingPrepared::prepare(
        enabled(PrimaryAdvertisingChannelMap::all()),
        memory(),
    )
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
        let prepared = BluetoothLegacyAdvertisingPrepared::prepare(enabled(channels), memory())
            .expect("bounded validated advertising data always fits the chip PDU");
        assert_eq!(prepared.channels(), channels);
    }
}

#[test]
fn reset_retains_the_exact_protocol_work_and_remains_cancellable() {
    let prepared = BluetoothLegacyAdvertisingPrepared::prepare(
        enabled(PrimaryAdvertisingChannelMap::all()),
        memory(),
    )
    .expect("bounded validated advertising data always fits the chip PDU");
    let identity = prepared.identity();
    let reset = match prepared.reset_link_state(BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
    {
        BluetoothLegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        BluetoothLegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
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
    let prepared = BluetoothLegacyAdvertisingPrepared::prepare(
        enabled(PrimaryAdvertisingChannelMap::all()),
        memory(),
    )
    .expect("bounded validated advertising data always fits the chip PDU");
    let reset = match prepared.reset_link_state(BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
    {
        BluetoothLegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        BluetoothLegacyAdvertisingLinkStateResetOutcome::Rejected { .. } => {
            panic!("the portable producer emits the restricted PDU form")
        }
    };
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let timing = BluetoothLegacyAdvertisingTimingObservation {
        current: BluetoothSchedulerInstant::from_image(10_000),
        radio_ready: BluetoothSchedulerInstant::from_image(11_999),
        epoch: BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scale,
        ),
    };
    let candidate = match reset.form_first_event_candidate(
        timing,
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
    ) {
        BluetoothLegacyAdvertisingFirstEventCandidateOutcome::Candidate(candidate) => candidate,
        BluetoothLegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(_) => {
            panic!("the reviewed timing window projects into one raw epoch")
        }
    };

    assert_eq!(candidate.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
    assert_eq!(candidate.projected_window_duration(), 192);
    let (enabled, _) = candidate.cancel().into_parts();
    assert_eq!(
        enabled.prepare_event().channels(),
        PrimaryAdvertisingChannelMap::all()
    );
}
