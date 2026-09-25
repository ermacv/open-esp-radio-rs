use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample,
    le::peripheral::connection::PeripheralConnectionPacketStartTiming,
    scheduler::SchedulerSoftwareConfig,
};

use oer_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeDataChannelMap,
    LeLegacyConnectionRequest, LePeripheralConnection, LePeripheralConnectionEventDelta,
    LePeripheralConnectionEventPeerActivity,
};

use oer_esp32s31_bluetooth_memory::PeripheralConnectionEventSpan;

use super::{
    LE_CONNECTION_COMMON_RESERVE_MICROS, LE_RECURRING_FIXED_GUARD_MICROS,
    LE_RECURRING_RECEIVE_CPU_TIME_TAIL_MICROS, LE_RECURRING_SCHEDULER_BOUNDARY_GUARD_MICROS,
    LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS, PeripheralConnectionLocalSleepClockAccuracy,
    PeripheralConnectionRecurringPhase, PeripheralConnectionRecurringTimingError,
    PeripheralConnectionRecurringTimingPolicy, PeripheralConnectionWindowWideningMode,
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

fn epoch(micros_anchor: u32) -> ControllerSchedulerEpoch {
    ControllerSchedulerEpoch::new(
        ControllerTimeSample::for_validation(100),
        micros_anchor,
        crate::controller_hal::reviewed_standalone_time_scale(),
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

#[test]
fn unknown_anchor_retains_win_size_until_a_real_capture() {
    let request = request(24, 4);
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let mut unknown = PeripheralConnectionRecurringPhase::from_nominal_anchor(
        crate::SchedulerInstant::from_image(u32::MAX - 10_000),
    );
    for _ in 0..5 {
        let wide = unknown
            .plan(request, delta, epoch, config, software_policy())
            .unwrap();
        let actual = PeripheralConnectionPacketStartTiming::from_scheduler_micros(
            unknown.nominal_anchor.image(),
        );
        let corrected = unknown
            .correct_from_normalized_packet_start(&actual)
            .plan(request, delta, epoch, config, software_policy())
            .unwrap();
        // Compare to the receive uncertainty rather than descriptor encodings.
        assert!(
            wide.receive_wait().total_micros() >= 2_500 + corrected.receive_wait().total_micros()
        );
        assert!(
            wide.window().end().wrapping_sub(wide.window().start())
                > corrected
                    .window()
                    .end()
                    .wrapping_sub(corrected.window().start())
        );
        unknown = wide.proposed_phase;
    }
}

#[test]
fn immediate_successor_forms_all_typed_recurring_inputs() {
    let request = request(24, 4);
    let epoch = epoch(9_000);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let plan = phase(10_000)
        .plan(request, delta, epoch, config, software_policy())
        .expect("known software widening forms a plan");

    let calculated_widening = ((30_000 / 1_000) * (75 + 60)) / 1_000;
    let widening = calculated_widening + LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS;
    let proposed_anchor = 40_000u32;
    let expected_start = proposed_anchor
        .wrapping_sub(config.preparation_lead_micros())
        .wrapping_sub(LE_RECURRING_FIXED_GUARD_MICROS)
        .wrapping_sub(widening)
        .wrapping_sub(LE_RECURRING_SCHEDULER_BOUNDARY_GUARD_MICROS);
    let expected_end = proposed_anchor
        .wrapping_sub(config.preparation_lead_micros())
        .wrapping_add(5_154)
        .wrapping_add(widening);

    assert_eq!(plan.delta(), delta);
    assert_eq!(plan.proposed_anchor().image(), proposed_anchor);
    assert_eq!(
        plan.window().start(),
        epoch.raw_ticks_for_micros(expected_start)
    );
    assert_eq!(
        plan.window().end(),
        epoch.raw_ticks_for_micros(expected_end)
    );
    assert_eq!(plan.window_widening_micros(), widening);
    assert_eq!(
        plan.event_span(),
        PeripheralConnectionEventSpan::new(epoch.raw_duration_ticks_for_micros(
            request.timing().interval_micros() - LE_CONNECTION_COMMON_RESERVE_MICROS,
        ))
        .unwrap()
    );
    assert_eq!(
        plan.receive_wait().total_micros(),
        LE_RECURRING_FIXED_GUARD_MICROS + 2 * widening + LE_RECURRING_RECEIVE_CPU_TIME_TAIL_MICROS
    );
}

#[test]
fn connection_update_instant_uses_old_interval_offset_and_new_window() {
    let previous = request(24, 4);
    let updated = request(40, 4);
    let mut completed = LePeripheralConnection::from_request(
        previous,
        oer_bluetooth_ll::connection::LeChannelSelectionAlgorithm::AlgorithmTwo,
    )
    .prepare_event()
    .into_submitted()
    .complete(LePeripheralConnectionEventPeerActivity::Observed);
    completed
        .schedule_connection_update(updated.timing(), 1)
        .unwrap();
    let provisional =
        completed.prepare_recurring_event(LePeripheralConnectionEventDelta::new(1).unwrap());
    let transition = provisional.connection_timing_transition().unwrap();
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let plan = phase(10_000)
        .plan_connection_update(
            previous,
            transition,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch,
            config,
            software_policy(),
        )
        .unwrap();
    let expected_anchor = 10_000
        + previous.timing().interval_micros()
        + u32::from(updated.timing().window_offset_units()) * 1_250;
    assert_eq!(plan.proposed_anchor().image(), expected_anchor);
    assert_eq!(
        plan.event_span(),
        PeripheralConnectionEventSpan::new(epoch.raw_duration_ticks_for_micros(
            updated.timing().interval_micros() - LE_CONNECTION_COMMON_RESERVE_MICROS,
        ))
        .unwrap()
    );
    assert!(
        plan.receive_wait().total_micros()
            >= u32::from(updated.timing().window_size_units()) * 1_250
    );

    let (_, proposed_phase, _, _, _, _) = plan.into_parts();
    let following = proposed_phase
        .plan(
            updated,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch,
            config,
            software_policy(),
        )
        .unwrap();
    assert_eq!(
        following.proposed_anchor().image(),
        expected_anchor + updated.timing().interval_micros()
    );
    assert!(
        following.receive_wait().total_micros()
            >= u32::from(updated.timing().window_size_units()) * 1_250
    );
}

#[test]
fn window_widening_floors_elapsed_time_to_whole_milliseconds() {
    let request = request(6, 0);
    let policy = PeripheralConnectionRecurringTimingPolicy::new(
        Some(PeripheralConnectionLocalSleepClockAccuracy::new(500).unwrap()),
        PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    );
    let plan = phase(10_000)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch(0),
            SchedulerSoftwareConfig::reviewed_standalone(),
            policy,
        )
        .unwrap();

    // floor(7_500 / 1_000) * (500 + 500) / 1_000 is 7. A ceiling
    // at the elapsed-time division would produce 8 instead.
    assert_eq!(
        plan.window_widening_micros(),
        LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS + 7
    );
}

#[test]
fn window_widening_floors_the_ppm_product() {
    let request = request(8, 7);
    let policy = PeripheralConnectionRecurringTimingPolicy::new(
        Some(PeripheralConnectionLocalSleepClockAccuracy::new(79).unwrap()),
        PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    );
    let plan = phase(10_000)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch(0),
            SchedulerSoftwareConfig::reviewed_standalone(),
            policy,
        )
        .unwrap();

    // 10 * (20 + 79) / 1_000 is zero. A ceiling at the PPM-product
    // division would add one microsecond.
    assert_eq!(
        plan.window_widening_micros(),
        LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS
    );
}

#[test]
fn skipped_events_advance_nominal_phase_and_widening_by_the_same_delta() {
    let request = request(24, 4);
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let delta_four = LePeripheralConnectionEventDelta::new(4).unwrap();
    let direct = phase(10_000)
        .plan(request, delta_four, epoch, config, software_policy())
        .unwrap();
    let delta_two = LePeripheralConnectionEventDelta::new(2).unwrap();
    let first_half = phase(10_000)
        .plan(request, delta_two, epoch, config, software_policy())
        .unwrap();
    let (_, proposed_phase, _, _, _, _) = first_half.into_parts();
    let second_half = proposed_phase
        .plan(request, delta_two, epoch, config, software_policy())
        .unwrap();

    assert_eq!(direct.proposed_anchor(), second_half.proposed_anchor());
    assert_eq!(
        direct.window_widening_micros(),
        second_half.window_widening_micros()
    );
    assert_eq!(direct.window(), second_half.window());
    assert_eq!(direct.receive_wait(), second_half.receive_wait());
}

#[test]
fn maintenance_skip_at_shortest_interval_reserves_the_actual_successor_window() {
    use crate::le::peripheral::{
        deadlines::Deadlines,
        maintenance::{PeripheralMaintenanceBlocked, PeripheralMaintenanceBudget},
    };
    use core::num::NonZeroU32;

    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let policy = PeripheralConnectionRecurringTimingPolicy::new(
        Some(PeripheralConnectionLocalSleepClockAccuracy::new(500).unwrap()),
        PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    );
    let plan = phase(10_000)
        .plan(
            request(6, 0),
            LePeripheralConnectionEventDelta::from_skipped(1).unwrap(),
            epoch,
            config,
            policy,
        )
        .unwrap();
    let start = epoch.project_peripheral_event_start(plan.window());
    let end = epoch.project_peripheral_event_end(plan.window());
    let instant = crate::SchedulerInstant::from_image;
    let n = |v| NonZeroU32::new(v).unwrap();
    let admit = |execution, restoration, acquisition_delay: u64| {
        PeripheralMaintenanceBudget::new(n(execution), n(restoration), n(2_000))
            .unwrap()
            .admit(
                instant(10_000),
                instant(start),
                instant(end),
                100_000,
                100_000 + acquisition_delay,
                config.late_start_guard_micros(),
                Deadlines {
                    supervision: None,
                    procedure: None,
                    termination: None,
                },
            )
    };

    // Even sampling at the previous anchor is optimistic: event completion
    // consumes time too. One skip supplies two intervals, but preparation,
    // clock widening and the late-start guard all precede the next anchor.
    assert_eq!(plan.proposed_anchor().image(), 25_000);
    assert_eq!(start - 10_000 - config.late_start_guard_micros(), 14_764);
    assert_eq!(
        admit(10_000, 5_000, 0).unwrap_err(),
        PeripheralMaintenanceBlocked::WindowTooShort,
    );
    let accepted = admit(13_000, 1_500, 263).unwrap();
    assert_eq!(accepted.restoration.expires_at_micros(), 114_763);
    // Neither equality nor time spent acquiring the Controller sample is free.
    assert_eq!(
        admit(13_000, 1_500, 264).unwrap_err(),
        PeripheralMaintenanceBlocked::WindowTooShort,
    );
}

#[test]
fn scheduler_positions_wrap_without_losing_packet_start_phase() {
    let request = request(24, 4);
    let packet_start = u32::MAX - 10_000;
    let epoch = epoch(u32::MAX - 11_000);
    let plan = phase(packet_start)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch,
            SchedulerSoftwareConfig::reviewed_standalone(),
            software_policy(),
        )
        .unwrap();

    assert_eq!(
        plan.proposed_anchor().image(),
        packet_start.wrapping_add(request.timing().interval_micros())
    );
    assert!(plan.window().duration() > 0);
}

#[test]
fn missing_sca_or_software_widening_authority_fails_closed() {
    let request = request(24, 4);
    let delta = LePeripheralConnectionEventDelta::new(1).unwrap();
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let local = PeripheralConnectionLocalSleepClockAccuracy::new(60).unwrap();

    assert_eq!(
        phase(10_000).plan(
            request,
            delta,
            epoch,
            config,
            PeripheralConnectionRecurringTimingPolicy::new(
                None,
                PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
            ),
        ),
        Err(PeripheralConnectionRecurringTimingError::LocalSleepClockAccuracyUnknown)
    );
    assert_eq!(
        phase(10_000).plan(
            request,
            delta,
            epoch,
            config,
            PeripheralConnectionRecurringTimingPolicy::new(
                Some(local),
                PeripheralConnectionWindowWideningMode::Unknown,
            ),
        ),
        Err(PeripheralConnectionRecurringTimingError::WindowWideningModeUnknown)
    );
    assert_eq!(
        phase(10_000).plan(
            request,
            delta,
            epoch,
            config,
            PeripheralConnectionRecurringTimingPolicy::new(
                Some(local),
                PeripheralConnectionWindowWideningMode::Automatic,
            ),
        ),
        Err(PeripheralConnectionRecurringTimingError::AutomaticWindowWideningUnsupported)
    );
}

#[test]
fn unrepresentable_forward_or_receive_wait_range_fails_closed() {
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();

    assert_eq!(
        phase(10_000).plan(
            request(3_200, 0),
            LePeripheralConnectionEventDelta::new(1_000).unwrap(),
            epoch,
            config,
            software_policy(),
        ),
        Err(PeripheralConnectionRecurringTimingError::AnchorAdvanceOutsideForwardHalfRange)
    );
    assert_eq!(
        phase(10_000).plan(
            request(24, 0),
            LePeripheralConnectionEventDelta::new(10_000).unwrap(),
            epoch,
            config,
            PeripheralConnectionRecurringTimingPolicy::new(
                Some(PeripheralConnectionLocalSleepClockAccuracy::new(500).unwrap()),
                PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
            ),
        ),
        Err(PeripheralConnectionRecurringTimingError::ReceiveWaitUnrepresentable)
    );
}

#[test]
fn actual_packet_start_resets_the_nominal_widening_phase() {
    let request = request(24, 4);
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let widened = phase(10_000)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(10).unwrap(),
            epoch,
            config,
            software_policy(),
        )
        .unwrap();
    assert!(widened.window_widening_micros() > LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS);
    let (_, proposed_phase, proposed_anchor, _, _, _) = widened.into_parts();

    let actual = PeripheralConnectionPacketStartTiming::from_scheduler_micros(
        proposed_anchor.image().wrapping_add(7),
    );
    let corrected = proposed_phase
        .correct_from_normalized_packet_start(&actual)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch,
            config,
            software_policy(),
        )
        .unwrap();
    let immediate = phase(actual.scheduler_instant().image())
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch,
            config,
            software_policy(),
        )
        .unwrap();

    assert_eq!(
        corrected.proposed_anchor().image(),
        actual
            .scheduler_instant()
            .image()
            .wrapping_add(request.timing().interval_micros())
    );
    assert_eq!(
        corrected.window_widening_micros(),
        immediate.window_widening_micros()
    );
}

#[test]
fn full_maintenance_budget_at_shortest_interval_needs_multiple_omissions() {
    use crate::le::peripheral::{
        deadlines::Deadlines,
        maintenance::{PeripheralMaintenanceBlocked, PeripheralMaintenanceBudget},
        supervision::PeripheralSupervisionDeadline,
    };
    use core::num::NonZeroU32;
    let epoch = epoch(0);
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let policy = PeripheralConnectionRecurringTimingPolicy::new(
        Some(PeripheralConnectionLocalSleepClockAccuracy::new(500).unwrap()),
        PeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    );
    let n = |v| NonZeroU32::new(v).unwrap();
    let budget = PeripheralMaintenanceBudget::new(n(20_000), n(5_000), n(2_000)).unwrap();
    for anchor in [10_000, u32::MAX - 10_000] {
        let instant = crate::SchedulerInstant::from_image;
        let now = instant(anchor.wrapping_add(2_000)); // real event service consumes airtime
        let mut limits = Deadlines {
            supervision: None,
            procedure: None,
            termination: None,
        };
        for skipped in 0..=3 {
            let plan = phase(anchor)
                .plan(
                    request(6, 0),
                    LePeripheralConnectionEventDelta::from_skipped(skipped).unwrap(),
                    epoch,
                    config,
                    policy,
                )
                .unwrap();
            let start = instant(epoch.project_peripheral_event_start(plan.window()));
            let end = instant(epoch.project_peripheral_event_end(plan.window()));
            let admit = |limits| {
                budget.admit(
                    now,
                    start,
                    end,
                    100_000,
                    100_500,
                    config.late_start_guard_micros(),
                    limits,
                )
            };
            if skipped < 3 {
                assert_eq!(
                    admit(limits).unwrap_err(),
                    PeripheralMaintenanceBlocked::WindowTooShort
                );
            } else {
                let window = admit(limits).unwrap();
                assert_eq!(window.execution.expires_at_micros(), 120_500);
                assert_eq!(window.restoration.expires_at_micros(), 125_500);
                // Extra omissions may not borrow the protocol reserve.
                limits.supervision = Some(PeripheralSupervisionDeadline::new(
                    now,
                    end.image().wrapping_sub(now.image()) + 2_000,
                ));
                assert_eq!(
                    admit(limits).unwrap_err(),
                    PeripheralMaintenanceBlocked::ProtocolDeadline
                );
            }
        }
    }
}
