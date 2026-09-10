use core::ops::ControlFlow;

use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample,
    le::peripheral::connection::{
        PeripheralConnectionLocalSleepClockAccuracy, PeripheralConnectionPacketStartTiming,
        PeripheralConnectionRecurringPhase, PeripheralConnectionRecurringTimingError,
        PeripheralConnectionRecurringTimingPolicy, PeripheralConnectionWindowWideningMode,
    },
    scheduler::SchedulerSoftwareConfig,
};

use oer_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeDataChannelMap,
    LeLegacyConnectionRequest, LePeripheralConnection, LePeripheralConnectionEventCompleted,
    LePeripheralConnectionEventDelta, LePeripheralConnectionEventPeerActivity,
};

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{PeripheralConnectionRecurringCandidateError, prepare_recurring_protocol_proposal};

fn request(interval_units: u16, central_sca: u8) -> LeLegacyConnectionRequest {
    request_with_channel_selection(interval_units, central_sca, true)
}

fn request_with_channel_selection(
    interval_units: u16,
    central_sca: u8,
    channel_selection_two: bool,
) -> LeLegacyConnectionRequest {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = if channel_selection_two { 0x25 } else { 0x05 };
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

fn epoch(micros_anchor: u32) -> ControllerSchedulerEpoch {
    ControllerSchedulerEpoch::new(
        ControllerTimeSample::for_validation(100),
        micros_anchor,
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
    )
}

fn software_policy() -> PeripheralConnectionRecurringTimingPolicy {
    PeripheralConnectionRecurringTimingPolicy::new(
        Some(
            PeripheralConnectionLocalSleepClockAccuracy::new(60)
                .expect("60 ppm is a valid local accuracy"),
        ),
        PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    )
}

fn phase(packet_start_micros: u32) -> PeripheralConnectionRecurringPhase {
    PeripheralConnectionRecurringPhase::from_nominal_anchor(crate::SchedulerInstant::from_image(
        packet_start_micros,
    ))
    .correct_from_normalized_packet_start(
        &PeripheralConnectionPacketStartTiming::from_scheduler_micros(packet_start_micros),
    )
}

fn completed_event(request: LeLegacyConnectionRequest) -> LePeripheralConnectionEventCompleted {
    LePeripheralConnection::from_request(
        request,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    )
    .prepare_event()
    .into_submitted()
    .complete(LePeripheralConnectionEventPeerActivity::Observed)
}

fn missed_first_event(request: LeLegacyConnectionRequest) -> LePeripheralConnectionEventCompleted {
    LePeripheralConnection::from_request(
        request,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    )
    .prepare_event()
    .into_submitted()
    .complete(LePeripheralConnectionEventPeerActivity::Missed)
}

#[test]
fn six_unanswered_events_preserve_the_window_and_end_establishment() {
    for algorithm_two in [false, true] {
        let request = request_with_channel_selection(24, 4, algorithm_two);
        let mut current_phase = PeripheralConnectionRecurringPhase::from_nominal_anchor(
            crate::SchedulerInstant::from_image(u32::MAX - 20_000),
        );
        let mut completed = missed_first_event(request);
        for counter in 0..6 {
            assert_eq!(completed.event_counter(), counter);
            assert_eq!(completed.establishment_failed(), counter == 5);
            let result = prepare_recurring_protocol_proposal(
                completed,
                current_phase,
                None,
                LePeripheralConnectionEventDelta::new(1).unwrap(),
                epoch(0),
                SchedulerSoftwareConfig::reviewed_standalone(),
                software_policy(),
            );
            if counter == 5 {
                let ControlFlow::Break(failure) = result else {
                    panic!("failed establishment must not publish a seventh event");
                };
                assert_eq!(
                    failure.error,
                    PeripheralConnectionRecurringCandidateError::EstablishmentFailed
                );
                assert_eq!(failure.original_phase, current_phase);
                assert_eq!(failure.completed.event_counter(), 5);
                break;
            }
            let ControlFlow::Continue(candidate) = result else {
                panic!("an unanswered initial event must retain the whole transmit window");
            };
            assert_eq!(candidate.event_counter(), counter + 1);
            assert!(candidate.proposal.receive_wait.total_micros() >= 2_500);
            // Cancellation preserves the exact phase and LL completion; retry
            // advances neither the counter nor the widening reference twice.
            let proposal = candidate.proposal;
            let (restored, restored_phase, delta) = candidate.cancel();
            assert_eq!(restored_phase, current_phase);
            assert_eq!(restored.event_counter(), counter);
            let ControlFlow::Continue(retry) = prepare_recurring_protocol_proposal(
                restored,
                restored_phase,
                None,
                delta,
                epoch(0),
                SchedulerSoftwareConfig::reviewed_standalone(),
                software_policy(),
            ) else {
                panic!("the unchanged candidate remains admissible")
            };
            assert_eq!(retry.proposal, proposal);
            current_phase = retry.proposal.proposed_phase;
            completed = retry
                .provisional
                .commit()
                .into_submitted()
                .complete(LePeripheralConnectionEventPeerActivity::Missed);
        }
    }
}

#[test]
fn establishment_cannot_skip_a_receive_window() {
    let completed = missed_first_event(request(24, 4));
    let original_phase = PeripheralConnectionRecurringPhase::from_nominal_anchor(
        crate::SchedulerInstant::from_image(10_000),
    );
    let ControlFlow::Break(failure) = prepare_recurring_protocol_proposal(
        completed,
        original_phase,
        None,
        LePeripheralConnectionEventDelta::new(2).unwrap(),
        epoch(0),
        SchedulerSoftwareConfig::reviewed_standalone(),
        software_policy(),
    ) else {
        panic!("establishment must listen in every event")
    };
    assert_eq!(
        failure.error,
        PeripheralConnectionRecurringCandidateError::EstablishmentEventSkipped
    );
    assert_eq!(failure.completed.event_counter(), 0);
    assert_eq!(failure.original_phase, original_phase);
}

#[test]
fn protocol_proposal_keeps_completed_owner_provisional_while_capture_advances_full_delta() {
    let request = request(24, 4);
    let original_phase = phase(9_900);
    let actual = PeripheralConnectionPacketStartTiming::from_scheduler_micros(10_007);
    let delta = LePeripheralConnectionEventDelta::new(3).unwrap();
    let expected_completed = completed_event(request);
    let ControlFlow::Continue(candidate) = prepare_recurring_protocol_proposal(
        completed_event(request),
        original_phase,
        Some(&actual),
        delta,
        epoch(0),
        SchedulerSoftwareConfig::reviewed_standalone(),
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
        SchedulerSoftwareConfig::reviewed_standalone(),
        PeripheralConnectionRecurringTimingPolicy::new(
            None,
            PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
        ),
    ) else {
        panic!("missing local SCA must reject the proposal");
    };

    assert_eq!(failure.completed, expected_completed);
    assert_eq!(failure.original_phase, original_phase);
    assert_eq!(failure.delta, delta);
    assert_eq!(
        failure.error,
        PeripheralConnectionRecurringCandidateError::Timing(
            PeripheralConnectionRecurringTimingError::LocalSleepClockAccuracyUnknown
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
        SchedulerSoftwareConfig::reviewed_standalone(),
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
        SchedulerSoftwareConfig::reviewed_standalone(),
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

#[test]
fn recurring_trace_preserves_channel_and_phase_across_counter_and_clock_wrap() {
    // CSA#1 with all channels enabled and hop 5. The trace spans a full
    // event-counter wrap; cancellation must not consume a hop or interval.
    const CHANNEL_CYCLE: [u8; 37] = [
        5, 10, 15, 20, 25, 30, 35, 3, 8, 13, 18, 23, 28, 33, 1, 6, 11, 16, 21, 26, 31, 36, 4, 9,
        14, 19, 24, 29, 34, 2, 7, 12, 17, 22, 27, 32, 0,
    ];
    let request = request_with_channel_selection(24, 4, false);
    let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let mut packet_start = u32::MAX - 40_000;
    let epoch = epoch(packet_start.wrapping_sub(1_000));
    let mut retained_phase = phase(packet_start);
    let mut completed = completed_event(request);

    for ordinal in 1..=u32::from(u16::MAX) + 3 {
        let actual = PeripheralConnectionPacketStartTiming::from_scheduler_micros(packet_start);
        let ControlFlow::Continue(mut candidate) = prepare_recurring_protocol_proposal(
            completed,
            retained_phase,
            Some(&actual),
            delta,
            epoch,
            config,
            software_policy(),
        ) else {
            panic!("a freshly observed contiguous anchor must remain representable");
        };
        if ordinal % 257 == 0 {
            let (restored, original_phase, restored_delta) = candidate.cancel();
            assert_eq!(original_phase, retained_phase);
            assert_eq!(restored.event_counter(), (ordinal - 1) as u16);
            assert_eq!(restored_delta, delta);
            let ControlFlow::Continue(retried) = prepare_recurring_protocol_proposal(
                restored,
                original_phase,
                Some(&actual),
                restored_delta,
                epoch,
                config,
                software_policy(),
            ) else {
                panic!("retrying the exact cancelled candidate must preserve admission inputs");
            };
            candidate = retried;
        }
        packet_start = packet_start.wrapping_add(request.timing().interval_micros());
        assert_eq!(candidate.event_counter(), ordinal as u16);
        assert_eq!(
            candidate.provisional.channel().get(),
            CHANNEL_CYCLE[ordinal as usize % 37]
        );
        assert_eq!(candidate.proposal.proposed_anchor.image(), packet_start);
        retained_phase = candidate.proposal.proposed_phase;
        completed = candidate
            .provisional
            .commit()
            .into_submitted()
            .complete(LePeripheralConnectionEventPeerActivity::Observed);
        assert_eq!(completed.event_counter(), ordinal as u16);
    }
}

#[test]
fn missed_events_keep_the_actual_widening_reference_until_a_new_capture() {
    let request = request(24, 4);
    let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let first_capture = PeripheralConnectionPacketStartTiming::from_scheduler_micros(10_007);
    let mut retained_phase = phase(9_900);
    let mut completed = completed_event(request);
    let mut first = true;

    // These windows are all measured from the actual first packet at 10_007,
    // even though the nominal anchor advances after each missed event.
    for (counter, anchor, widening) in [(1, 40_007, 67), (2, 70_007, 71), (3, 100_007, 75)] {
        let ControlFlow::Continue(candidate) = prepare_recurring_protocol_proposal(
            completed,
            retained_phase,
            first.then_some(&first_capture),
            delta,
            epoch,
            config,
            software_policy(),
        ) else {
            panic!("bounded missed events retain an established anchor");
        };
        first = false;
        assert_eq!(candidate.event_counter(), counter);
        assert_eq!(candidate.proposal.proposed_anchor.image(), anchor);
        assert_eq!(candidate.proposal.window_widening_micros, widening);
        retained_phase = candidate.proposal.proposed_phase;
        completed = candidate
            .provisional
            .commit()
            .into_submitted()
            .complete(if counter == 3 {
                LePeripheralConnectionEventPeerActivity::Observed
            } else {
                LePeripheralConnectionEventPeerActivity::Missed
            });
        assert_eq!(
            completed.connection_state().establishment_event_counter(),
            Some(0)
        );
        assert_eq!(
            completed
                .connection_state()
                .supervision_anchor_event_counter(),
            Some(if counter == 3 { 3 } else { 0 })
        );
    }

    let recovered_capture = PeripheralConnectionPacketStartTiming::from_scheduler_micros(100_014);
    let ControlFlow::Continue(candidate) = prepare_recurring_protocol_proposal(
        completed,
        retained_phase,
        Some(&recovered_capture),
        delta,
        epoch,
        config,
        software_policy(),
    ) else {
        panic!("a new capture must correct both nominal phase and widening reference");
    };
    assert_eq!(candidate.event_counter(), 4);
    assert_eq!(candidate.proposal.proposed_anchor.image(), 130_014);
    assert_eq!(candidate.proposal.window_widening_micros, 67);
}
