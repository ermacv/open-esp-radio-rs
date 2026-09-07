use crate::{
    controller::time::{ControllerSchedulerEpoch, ControllerTimeSample},
    le::{
        advertising::LegacyAdvertisingDefaultTxPowerDbm,
        peripheral::{PeripheralConnectionRuntimeConfig, PeripheralConnectionRuntimeResources},
    },
    scheduler::SchedulerSoftwareConfig,
};

use oer_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        AdvertisingDelay, AdvertisingInterval, LegacyAdvertisingData, PrimaryAdvertisingChannelMap,
    },
    connectable_advertising::{
        LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
        LegacyConnectableAdvertiserStandby, LegacyConnectableAdvertisingEventInFlight,
        LegacyConnectableAdvertisingSet, LegacyScanResponseData,
    },
    connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeLegacyConnectionRequest,
        LePeripheralConnection,
    },
};

use oer_esp32s31_bluetooth_memory::{
    LeReceivedPdu, LegacyConnectableAdvertisingMemoryGraphModelAddress,
    LegacyConnectableAdvertisingMemoryGraphStorage,
    LegacyConnectableAdvertisingSchedulerItemCompletionStatus, NonScanningRxMemoryModelAddress,
    NonScanningRxMemoryStorage, PeripheralConnectionDefaultTxPowerDbm,
    PeripheralConnectionMemoryGraphModelAddress, PeripheralConnectionMemoryGraphStorage,
};

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    LegacyConnectableAdvertisingPortableRxOutcome, LegacyConnectableAdvertisingReceivedPdu,
    LegacyConnectableAdvertisingRuntimeBeginFailure, LegacyConnectableAdvertisingRuntimeResources,
    LegacyConnectableAdvertisingSetError, PeripheralConnectionAcceptedRequest,
    PeripheralConnectionAcceptedResetCancellationError, classify_received_pdus,
    refine_portable_set,
};

fn definition(
    channels: PrimaryAdvertisingChannelMap,
) -> Result<super::LegacyConnectableAdvertisingSetPrepared, LegacyConnectableAdvertisingSetError> {
    let advertiser =
        LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Public);
    refine_portable_set(LegacyConnectableAdvertisingSet::new(
        LegacyConnectableAdvertisement::new(
            advertiser,
            LegacyAdvertisingData::new_owned(&[2, 1, 6]).unwrap(),
            LeChannelSelectionAlgorithmTwoSupport::Unsupported,
        ),
        LegacyScanResponseData::new_owned(&[3, 3, 0xaa, 0xfe]).unwrap(),
        channels,
        AdvertisingInterval::new(32).unwrap(),
    ))
}

#[derive(Clone, Copy)]
struct TestReceivedPdu<'a>(&'a [u8]);

impl LegacyConnectableAdvertisingReceivedPdu for TestReceivedPdu<'_> {
    fn pdu_bytes(&self) -> &[u8] {
        self.0
    }
}

fn connection_request(advertiser: [u8; 6]) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = 0b0101 | (1 << 6);
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
    pdu[8..14].copy_from_slice(&advertiser);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    pdu[35] = 5 | (4 << 5);
    pdu
}

fn portable_in_flight(
    definition: super::LegacyConnectableAdvertisingSetPrepared,
) -> LegacyConnectableAdvertisingEventInFlight<'static> {
    LegacyConnectableAdvertiserStandby::new()
        .configure(definition.set())
        .enable()
        .unwrap_or_else(|_| panic!("the fresh validation generation must be available"))
        .prepare()
        .into_submitted()
}

fn connectable_runtime(base: u32) -> LegacyConnectableAdvertisingRuntimeResources {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        LegacyConnectableAdvertisingMemoryGraphStorage::new(),
    ));
    LegacyConnectableAdvertisingRuntimeResources::claim_static_model(
        storage,
        LegacyConnectableAdvertisingMemoryGraphModelAddress::new(base).unwrap(),
        LegacyAdvertisingDefaultTxPowerDbm::new(4),
    )
    .unwrap()
}

fn peripheral_runtime(base: u32) -> PeripheralConnectionRuntimeResources {
    let graph = std::boxed::Box::leak(std::boxed::Box::new(
        PeripheralConnectionMemoryGraphStorage::new(),
    ));
    let receive = std::boxed::Box::leak(std::boxed::Box::new(NonScanningRxMemoryStorage::new()));
    PeripheralConnectionRuntimeResources::claim_static_model(
        graph,
        PeripheralConnectionMemoryGraphModelAddress::new(base).unwrap(),
        receive,
        NonScanningRxMemoryModelAddress::new(base + 0x1000).unwrap(),
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(4)),
    )
    .unwrap()
}

fn sample_phase() -> crate::le::advertising::LegacyAdvertisingEventPhase {
    let mut connectable = connectable_runtime(0x2f03_0000);
    let mut peripheral = peripheral_runtime(0x2f03_3000);
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
    let prepared = connectable
        .begin_event(definition, &mut peripheral)
        .unwrap_or_else(|_| panic!("the isolated validation resources must prepare"));
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch =
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(100), 1_000, scale);
    let candidate = prepared
        .form_first_event_candidate(
            crate::le::advertising::LegacyAdvertisingTimingObservation {
                current: crate::SchedulerInstant::from_image(30_000),
                radio_ready: crate::SchedulerInstant::from_image(32_000),
                epoch,
            },
            SchedulerSoftwareConfig::reviewed_standalone(),
        )
        .unwrap_or_else(|_| panic!("the isolated timing window must project"));
    let phase = candidate.scheduler_window.phase();
    let cancelled = candidate
        .cancel()
        .unwrap_or_else(|_| panic!("the exact validation receive pool must rejoin"));
    let _definition = connectable
        .restore_cancelled(cancelled, &mut peripheral)
        .unwrap_or_else(|_| panic!("the validation resources must restore"));
    phase
}

fn accepted_transfer(
    peripheral: &mut PeripheralConnectionRuntimeResources,
    phase: crate::le::advertising::LegacyAdvertisingEventPhase,
    captured_time: u32,
) -> super::LegacyConnectableAdvertisingConnectionTransfer {
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
    let request_bytes =
        connection_request(definition.set().advertisement().advertiser().wire_bytes());
    let request = LeLegacyConnectionRequest::decode(&request_bytes)
        .unwrap_or_else(|_| panic!("the validation CONNECT_IND must decode"));
    let packet = LeReceivedPdu::from_parts_for_validation(&request_bytes, -37, captured_time)
        .unwrap_or_else(|| panic!("the validation packet must be complete"));
    let allocation = peripheral
        .begin_event()
        .unwrap_or_else(|_| panic!("the validation peripheral allocation must be idle"));
    let in_flight = portable_in_flight(definition);
    let identity = in_flight.identity();
    let _configured = in_flight.complete_without_connection().disable();
    super::LegacyConnectableAdvertisingConnectionTransfer {
        advertising_set: definition.set(),
        identity,
        peripheral: PeripheralConnectionAcceptedRequest::new(
            allocation,
            LePeripheralConnection::from_request(request),
            packet,
        ),
        phase,
        scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
        rejected_packets: 2,
    }
}

#[test]
fn empty_dispatch_completes_without_using_scheduler_status_as_an_outcome() {
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
    let in_flight = portable_in_flight(definition);

    let outcome = classify_received_pdus::<TestReceivedPdu<'_>>(in_flight, [None, None], 0, 0);
    let LegacyConnectableAdvertisingPortableRxOutcome::NoConnection {
        complete,
        rejected_packets,
    } = outcome
    else {
        panic!("an empty copied batch cannot accept a connection")
    };
    assert_eq!(rejected_packets, 0);
    assert_eq!(complete.disable().set(), definition.set());
}

#[test]
fn rejected_packet_then_addressed_connect_ind_transfers_the_connection() {
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(false, true, false).unwrap()).unwrap();
    let advertiser = definition.set().advertisement().advertiser().wire_bytes();
    let request = connection_request(advertiser);
    let unrelated_pdu = [0x03, 0];
    let in_flight = portable_in_flight(definition);

    let outcome = classify_received_pdus(
        in_flight,
        [
            Some(TestReceivedPdu(&unrelated_pdu)),
            Some(TestReceivedPdu(&request)),
        ],
        0,
        0,
    );
    let LegacyConnectableAdvertisingPortableRxOutcome::ConnectionAccepted {
        accepted,
        packet,
        rejected_packets,
    } = outcome
    else {
        panic!("the final addressed CONNECT_IND must be admitted")
    };
    assert_eq!(rejected_packets, 1);
    assert_eq!(packet.pdu_bytes(), request.as_slice());
    assert_eq!(accepted.request().advertiser().wire_bytes(), advertiser);
    let (configured, _identity, connection) = accepted.into_parts();
    assert_eq!(configured.set(), definition.set());
    assert_eq!(connection.event_counter(), 0);
}

#[test]
fn packet_after_accepted_connect_ind_is_not_silently_discarded() {
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(false, false, true).unwrap()).unwrap();
    let request = connection_request(definition.set().advertisement().advertiser().wire_bytes());
    let later_pdu = [0x03, 0];
    let in_flight = portable_in_flight(definition);

    let identity = in_flight.identity();
    let outcome = classify_received_pdus(
        in_flight,
        [
            Some(TestReceivedPdu(&request)),
            Some(TestReceivedPdu(&later_pdu)),
        ],
        0,
        0,
    );
    let LegacyConnectableAdvertisingPortableRxOutcome::PacketAfterConnection {
        accepted,
        packet,
        rejected_packets,
    } = outcome
    else {
        panic!("the later PDU must preserve the accepted connection as a sealed outcome")
    };
    assert_eq!(rejected_packets, 0);
    assert!(core::ptr::eq(packet.0, request.as_slice()));
    let (configured, accepted_identity, connection) = accepted.into_parts();
    assert_eq!(accepted_identity, identity);
    assert_eq!(configured.set(), definition.set());
    assert_eq!(
        connection.request().advertiser().wire_bytes(),
        definition.set().advertisement().advertiser().wire_bytes()
    );
}

#[test]
fn multiple_channels_fail_before_any_runtime_checkout() {
    let mut connectable = connectable_runtime(0x2f00_1000);
    let peripheral = peripheral_runtime(0x2f00_3000);

    let error = definition(PrimaryAdvertisingChannelMap::all()).unwrap_err();

    assert_eq!(
        error,
        LegacyConnectableAdvertisingSetError::MultiplePrimaryChannels { selected: 3 }
    );
    assert!(connectable.event_is_idle());
    assert!(peripheral.allocation_is_idle());
    assert_eq!(connectable.default_tx_power_dbm().dbm(), 4);
    // Keep the mutable binding honest for the following tests' API shape.
    let _ = &mut connectable;
}

#[test]
fn preparation_and_cancel_restore_both_originating_runtimes() {
    let mut connectable = connectable_runtime(0x2f00_6000);
    let mut peripheral = peripheral_runtime(0x2f00_8000);
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
    let prepared = match connectable.begin_event(definition, &mut peripheral) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the disjoint idle allocations must prepare"),
    };
    assert!(!connectable.event_is_idle());
    assert!(!peripheral.allocation_is_idle());
    let cancelled = match prepared.cancel() {
        Ok(cancelled) => cancelled,
        Err(_) => panic!("an internally paired pool must cancel losslessly"),
    };
    let restored = connectable
        .restore_cancelled(cancelled, &mut peripheral)
        .unwrap_or_else(|_| panic!("both exact runtime slots must accept their owners"));
    assert_eq!(restored, definition);
    assert!(connectable.event_is_idle());
    assert!(peripheral.allocation_is_idle());

    let prepared = match connectable.begin_event(restored, &mut peripheral) {
        Ok(prepared) => prepared,
        Err(_) => panic!("cancel must restore a graph ready for another event"),
    };
    let cancelled = match prepared.cancel() {
        Ok(cancelled) => cancelled,
        Err(_) => panic!("the repeated event keeps the exact pool"),
    };
    let _restored = connectable
        .restore_cancelled(cancelled, &mut peripheral)
        .unwrap_or_else(|_| panic!("the repeated cancellation restores both runtimes"));
}

#[test]
fn no_connection_restore_returns_both_runtime_slots_atomically() {
    let mut connectable = connectable_runtime(0x2f00_6100);
    let mut peripheral = peripheral_runtime(0x2f00_8100);
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
    let prepared = connectable
        .begin_event(definition, &mut peripheral)
        .unwrap_or_else(|_| panic!("the disjoint idle owners must prepare"));
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch =
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(100), 1_000, scale);
    let candidate = prepared
        .form_first_event_candidate(
            crate::le::advertising::LegacyAdvertisingTimingObservation {
                current: crate::SchedulerInstant::from_image(10_000),
                radio_ready: crate::SchedulerInstant::from_image(12_000),
                epoch,
            },
            SchedulerSoftwareConfig::reviewed_standalone(),
        )
        .unwrap_or_else(|_| panic!("the first event timing must project"));
    let phase = candidate.scheduler_window.phase();
    let cancelled = candidate
        .cancel()
        .unwrap_or_else(|_| panic!("the exact receive pool must rejoin"));
    let super::LegacyConnectableAdvertisingCancelled {
        definition,
        configured,
        graph,
        allocation,
    } = cancelled;
    let complete = configured
        .enable()
        .unwrap_or_else(|_| panic!("the retained validation generation must advance"))
        .prepare()
        .into_submitted()
        .complete_without_connection();
    let outcome = super::LegacyConnectableAdvertisingNoConnection {
            definition,
            graph,
            allocation,
            complete,
            phase,
            scheduler_status: oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
            rejected_packets: 1,
        };

    let restored = connectable
        .restore_no_connection(outcome, &mut peripheral)
        .unwrap_or_else(|_| panic!("both exact runtime slots must accept their owners"));
    assert!(connectable.event_is_idle());
    assert!(peripheral.allocation_is_idle());
    assert_eq!(restored.definition(), definition);
    assert_eq!(restored.phase(), phase);
    assert_eq!(restored.rejected_packets(), 1);
    assert_eq!(
            restored.scheduler_status(),
            oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
    let completed_identity = restored.identity();
    let delay = AdvertisingDelay::from_micros(7_500)
        .unwrap_or_else(|_| panic!("the portable maximum delay contains this value"));
    let scheduled = restored.schedule_next(delay);
    let (
        scheduled_definition,
        portable,
        start_offset_micros,
        scheduled_from_phase,
        previous_status,
        previous_rejected_packets,
    ) = scheduled.into_parts();
    assert_eq!(scheduled_definition, definition);
    let successor_identity = portable.identity();
    assert_eq!(
        successor_identity.generation(),
        completed_identity.generation()
    );
    assert_eq!(
        successor_identity.event().get(),
        completed_identity.event().get() + 1
    );
    let configured = match portable {
        super::LegacyConnectableAdvertisingNextEventPortable::Event(event) => {
            let configured = event.disable();
            assert_eq!(configured.set(), definition.set());
            configured
        }
        super::LegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(_) => {
            panic!("the first validation recurrence cannot exhaust event identity")
        }
    };
    connectable
        .restore_disabled_advertiser(configured)
        .unwrap_or_else(|_| panic!("the stopped advertiser must rejoin its idle runtime"));
    assert_eq!(
        start_offset_micros,
        definition.set().interval().as_micros() + u64::from(delay.as_micros())
    );
    assert_eq!(scheduled_from_phase, phase);
    assert_eq!(
            previous_status,
            oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
    assert_eq!(previous_rejected_packets, 1);

    let next_enable = connectable
        .begin_event(definition, &mut peripheral)
        .unwrap_or_else(|_| panic!("disable must restore the next Enable generation"));
    assert_ne!(
        next_enable.identity().generation(),
        completed_identity.generation()
    );
    assert_eq!(next_enable.identity().event().get(), 0);
}

#[test]
fn foreign_no_connection_restore_preserves_the_exact_role_owner() {
    let mut origin = connectable_runtime(0x2f02_0000);
    let mut foreign = connectable_runtime(0x2f02_2000);
    let mut peripheral = peripheral_runtime(0x2f02_4000);
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
    let prepared = origin
        .begin_event(definition, &mut peripheral)
        .unwrap_or_else(|_| panic!("the disjoint idle owners must prepare"));
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch =
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(100), 1_000, scale);
    let candidate = prepared
        .form_first_event_candidate(
            crate::le::advertising::LegacyAdvertisingTimingObservation {
                current: crate::SchedulerInstant::from_image(20_000),
                radio_ready: crate::SchedulerInstant::from_image(22_000),
                epoch,
            },
            SchedulerSoftwareConfig::reviewed_standalone(),
        )
        .unwrap_or_else(|_| panic!("the bounded response window must project"));
    let phase = candidate.scheduler_window.phase();
    let cancelled = candidate
        .cancel()
        .unwrap_or_else(|_| panic!("the exact receive pool must rejoin"));
    let super::LegacyConnectableAdvertisingCancelled {
        definition,
        configured,
        graph,
        allocation,
    } = cancelled;
    let complete = configured
        .enable()
        .unwrap_or_else(|_| panic!("the retained validation generation must advance"))
        .prepare()
        .into_submitted()
        .complete_without_connection();
    let outcome = super::LegacyConnectableAdvertisingNoConnection {
            definition,
            graph,
            allocation,
            complete,
            phase,
            scheduler_status: oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero,
            rejected_packets: 0,
        };

    let outcome = match foreign.restore_no_connection(outcome, &mut peripheral) {
        Ok(_) => panic!("a foreign runtime cannot consume the exact completed graph"),
        Err(outcome) => outcome,
    };
    assert!(foreign.event_is_idle());
    assert!(!origin.event_is_idle());
    assert!(!peripheral.allocation_is_idle());

    let restored = origin
        .restore_no_connection(outcome, &mut peripheral)
        .unwrap_or_else(|_| panic!("the origin must recover the unchanged owner"));
    assert!(origin.event_is_idle());
    assert!(peripheral.allocation_is_idle());
    assert_eq!(restored.phase(), phase);
    assert_eq!(restored.rejected_packets(), 0);
}

#[test]
fn busy_peripheral_rolls_back_the_connectable_graph() {
    let mut connectable = connectable_runtime(0x2f00_b000);
    let mut peripheral = peripheral_runtime(0x2f00_d000);
    let held = peripheral.begin_event().unwrap();
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(false, true, false).unwrap()).unwrap();

    let failure = match connectable.begin_event(definition, &mut peripheral) {
        Ok(_) => panic!("the checked-out connection allocation must reject advertising"),
        Err(failure) => failure,
    };
    let LegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
        definition: returned_definition,
        error,
    } = failure
    else {
        panic!("the busy peripheral must return its ordinary admission error")
    };
    assert_eq!(returned_definition, definition);
    assert_eq!(
        error,
        crate::le::peripheral::PeripheralConnectionRuntimeBeginError::EventActive
    );
    assert!(connectable.event_is_idle());
    assert!(!peripheral.allocation_is_idle());

    peripheral
        .restore_idle(held)
        .unwrap_or_else(|_| panic!("the retained connection allocation remains exact"));
    assert!(peripheral.allocation_is_idle());
}

#[test]
fn memory_preparation_failure_restores_both_runtime_slots() {
    let mut connectable = connectable_runtime(0x2f00_f000);
    let mut peripheral = peripheral_runtime(0x2f00_e000);
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();

    let failure = match connectable.begin_event(definition, &mut peripheral) {
        Ok(_) => panic!("overlapping role allocations must fail before publication"),
        Err(failure) => failure,
    };
    let LegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
        definition: returned_definition,
        error,
    } = failure
    else {
        panic!("overlapping allocations must return the unchanged definition")
    };
    assert_eq!(returned_definition, definition);
    assert_eq!(error, oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph);
    assert!(connectable.event_is_idle());
    assert!(peripheral.allocation_is_idle());
}

#[test]
fn active_connectable_event_does_not_touch_an_unrelated_peripheral_runtime() {
    let mut connectable = connectable_runtime(0x2f01_0000);
    let mut first_peripheral = peripheral_runtime(0x2f01_2000);
    let mut second_peripheral = peripheral_runtime(0x2f01_5000);
    let definition =
        definition(PrimaryAdvertisingChannelMap::new(false, false, true).unwrap()).unwrap();
    let prepared = match connectable.begin_event(definition, &mut first_peripheral) {
        Ok(prepared) => prepared,
        Err(_) => panic!("the first response event must prepare"),
    };

    let failure = match connectable.begin_event(definition, &mut second_peripheral) {
        Ok(_) => panic!("one response graph cannot start a second event"),
        Err(failure) => failure,
    };
    let LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
        definition: returned_definition,
    } = failure
    else {
        panic!("an active advertiser must return the unchanged definition")
    };
    assert_eq!(returned_definition, definition);
    assert!(second_peripheral.allocation_is_idle());

    let cancelled = match prepared.cancel() {
        Ok(cancelled) => cancelled,
        Err(_) => panic!("the first event must retain its exact pool"),
    };
    let _restored = connectable
        .restore_cancelled(cancelled, &mut first_peripheral)
        .unwrap_or_else(|_| panic!("the first event restores its exact runtimes"));
    assert!(connectable.event_is_idle());
    assert!(first_peripheral.allocation_is_idle());
}

#[test]
fn explicit_reset_restores_the_exact_accepted_connection_allocation_losslessly() {
    let phase = sample_phase();
    let mut origin = peripheral_runtime(0x2f04_0000);
    let mut foreign = peripheral_runtime(0x2f04_3000);
    let mut busy = peripheral_runtime(0x2f04_6000);
    let foreign_held = foreign
        .begin_event()
        .unwrap_or_else(|_| panic!("the foreign validation allocation must begin idle"));
    let transfer = accepted_transfer(&mut origin, phase, 41_234);
    let identity = transfer.identity();
    assert!(!origin.allocation_is_idle());

    let failure = match transfer.cancel_peripheral_for_reset(&mut busy) {
        Ok(_) => panic!("an occupied runtime must not retire the accepted connection"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.cause(),
        PeripheralConnectionAcceptedResetCancellationError::RuntimeBusy
    );
    assert!(busy.allocation_is_idle());

    let failure = match failure
        .into_transfer()
        .cancel_peripheral_for_reset(&mut foreign)
    {
        Ok(_) => panic!("a foreign runtime must not retire the accepted connection"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.cause(),
        PeripheralConnectionAcceptedResetCancellationError::GraphIdentityMismatch
    );
    assert!(!origin.allocation_is_idle());
    assert!(!foreign.allocation_is_idle());

    let evidence = failure
        .into_transfer()
        .cancel_peripheral_for_reset(&mut origin)
        .unwrap_or_else(|_| panic!("the originating runtime must accept its exact owner"));
    assert!(origin.allocation_is_idle());
    assert_eq!(evidence.identity(), identity);
    assert_eq!(
        evidence.advertising_set().advertisement().advertiser(),
        definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap())
            .unwrap()
            .set()
            .advertisement()
            .advertiser()
    );
    assert_eq!(
        evidence.accepted_packet().as_bytes(),
        connection_request(
            evidence
                .advertising_set()
                .advertisement()
                .advertiser()
                .wire_bytes()
        )
        .as_slice()
    );
    assert_eq!(evidence.phase(), phase);
    assert_eq!(
        evidence.scheduler_status(),
        LegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
    );
    assert_eq!(evidence.rejected_packets(), 2);

    foreign
        .restore_idle(foreign_held)
        .unwrap_or_else(|_| panic!("the foreign allocation was not changed by rejection"));
    assert!(foreign.allocation_is_idle());

    let reusable = origin
        .begin_event()
        .unwrap_or_else(|_| panic!("Reset cancellation must make the allocation reusable"));
    origin
        .restore_idle(reusable)
        .unwrap_or_else(|_| panic!("the restored allocation must retain its identity"));
}
