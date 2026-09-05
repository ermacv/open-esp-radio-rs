use core::ops::ControlFlow;

use open_esp_radio_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeDataChannelMap,
    LeLegacyConnectionRequest, LePeripheralConnection, LePeripheralConnectionEventCompleted,
    LePeripheralConnectionEventDelta, LePeripheralConnectionEventPeerActivity,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    BluetoothPeripheralConnectionRecurringCandidateError, prepare_recurring_protocol_proposal,
};
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionLocalSleepClockAccuracy,
    BluetoothPeripheralConnectionPacketStartTiming, BluetoothPeripheralConnectionRecurringPhase,
    BluetoothPeripheralConnectionRecurringTimingError,
    BluetoothPeripheralConnectionRecurringTimingPolicy,
    BluetoothPeripheralConnectionWindowWideningMode,
};
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
    BluetoothSchedulerSoftwareConfig,
};

fn request(interval_units: u16, central_sca: u8) -> LeLegacyConnectionRequest {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = 0x25;
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&interval_units.to_le_bytes());
    pdu[28..30].copy_from_slice(&3200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&LeDataChannelMap::all().wire_bytes());
    pdu[35] = 5 | (central_sca << 5);
    LeLegacyConnectionRequest::decode(&pdu).expect("the connection request is valid")
}

fn epoch(micros_anchor: u32) -> BluetoothControllerSchedulerEpoch {
    BluetoothControllerSchedulerEpoch::new(
        BluetoothControllerTimeSample::for_validation(100),
        micros_anchor,
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    )
}

fn software_policy() -> BluetoothPeripheralConnectionRecurringTimingPolicy {
    BluetoothPeripheralConnectionRecurringTimingPolicy::new(
        Some(
            BluetoothPeripheralConnectionLocalSleepClockAccuracy::new(60)
                .expect("60 ppm is a valid local accuracy"),
        ),
        BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    )
}

fn phase(packet_start_micros: u32) -> BluetoothPeripheralConnectionRecurringPhase {
    BluetoothPeripheralConnectionRecurringPhase::from_nominal_anchor(
        crate::BluetoothSchedulerInstant::from_image(packet_start_micros),
    )
}

fn completed_event(request: LeLegacyConnectionRequest) -> LePeripheralConnectionEventCompleted {
    LePeripheralConnection::from_request(request)
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Observed)
}

fn missed_first_event(request: LeLegacyConnectionRequest) -> LePeripheralConnectionEventCompleted {
    LePeripheralConnection::from_request(request)
        .prepare_event()
        .into_submitted()
        .complete(LePeripheralConnectionEventPeerActivity::Missed)
}

#[test]
fn missed_first_event_cannot_invent_an_anchor_for_recurrence() {
    let request = request(24, 4);
    let original_phase = phase(10_000);
    let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let expected_completed = missed_first_event(request);
    let ControlFlow::Break(failure) = prepare_recurring_protocol_proposal(
        missed_first_event(request),
        original_phase,
        None,
        delta,
        epoch(0),
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        software_policy(),
    ) else {
        panic!("the first peer-selected anchor has not been observed");
    };

    assert_eq!(failure.completed, expected_completed);
    assert_eq!(failure.original_phase, original_phase);
    assert_eq!(failure.delta, delta);
    assert_eq!(
        failure.error,
        BluetoothPeripheralConnectionRecurringCandidateError::InitialAnchorUnavailable
    );
}

#[test]
fn protocol_proposal_keeps_completed_owner_provisional_while_capture_advances_full_delta() {
    let request = request(24, 4);
    let original_phase = phase(9_900);
    let actual = BluetoothPeripheralConnectionPacketStartTiming::from_scheduler_micros(10_007);
    let delta = LePeripheralConnectionEventDelta::new(3).unwrap();
    let expected_completed = completed_event(request);
    let ControlFlow::Continue(candidate) = prepare_recurring_protocol_proposal(
        completed_event(request),
        original_phase,
        Some(&actual),
        delta,
        epoch(0),
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        software_policy(),
    ) else {
        panic!("known timing authority forms a provisional proposal");
    };

    assert_eq!(candidate.event_counter(), delta.get());
    assert_eq!(candidate.proposal.delta, delta);
    assert_eq!(
        candidate.proposal.proposed_anchor.image(),
        actual
            .scheduler_instant()
            .image()
            .wrapping_add(request.timing().interval_micros() * u32::from(delta.get()))
    );
    let (completed, restored_phase, restored_delta) = candidate.cancel();
    assert_eq!(completed, expected_completed);
    assert_eq!(restored_phase, original_phase);
    assert_eq!(restored_delta, delta);
}

#[test]
fn protocol_planning_failure_returns_exact_completion_phase_and_delta() {
    let request = request(24, 4);
    let original_phase = phase(10_000);
    let delta = LePeripheralConnectionEventDelta::new(5).unwrap();
    let expected_completed = completed_event(request);
    let ControlFlow::Break(failure) = prepare_recurring_protocol_proposal(
        completed_event(request),
        original_phase,
        None,
        delta,
        epoch(0),
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        BluetoothPeripheralConnectionRecurringTimingPolicy::new(
            None,
            BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
        ),
    ) else {
        panic!("missing local SCA must reject the proposal");
    };

    assert_eq!(failure.completed, expected_completed);
    assert_eq!(failure.original_phase, original_phase);
    assert_eq!(failure.delta, delta);
    assert_eq!(
        failure.error,
        BluetoothPeripheralConnectionRecurringCandidateError::Timing(
            BluetoothPeripheralConnectionRecurringTimingError::LocalSleepClockAccuracyUnknown
        )
    );
}

#[test]
fn scheduler_rejection_can_restore_and_retry_with_a_different_typed_delta() {
    let request = request(24, 4);
    let original_phase = phase(20_000);
    let rejected_delta = LePeripheralConnectionEventDelta::new(4).unwrap();
    let ControlFlow::Continue(rejected) = prepare_recurring_protocol_proposal(
        completed_event(request),
        original_phase,
        None,
        rejected_delta,
        epoch(0),
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        software_policy(),
    ) else {
        panic!("the first scheduler candidate is representable");
    };
    assert_eq!(rejected.event_counter(), 4);
    assert_eq!(rejected.proposal.delta, rejected_delta);

    let (restored, restored_phase, restored_delta) = rejected.cancel();
    assert_eq!(restored, completed_event(request));
    assert_eq!(restored_phase, original_phase);
    assert_eq!(restored_delta, rejected_delta);
    let retry_delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let ControlFlow::Continue(retry) = prepare_recurring_protocol_proposal(
        restored,
        restored_phase,
        None,
        retry_delta,
        epoch(0),
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        software_policy(),
    ) else {
        panic!("the restored owner accepts a different retry");
    };

    assert_eq!(retry.event_counter(), 1);
    assert_eq!(retry.proposal.delta, retry_delta);
    assert_eq!(
        retry.proposal.proposed_anchor.image(),
        20_000 + request.timing().interval_micros()
    );
}
