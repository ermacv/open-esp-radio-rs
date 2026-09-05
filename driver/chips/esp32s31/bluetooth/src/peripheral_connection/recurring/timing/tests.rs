use open_esp_radio_bluetooth_ll::connection::{
    LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeDataChannelMap,
    LeLegacyConnectionRequest, LePeripheralConnectionEventDelta,
};
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionEventSpan;
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::{
    BluetoothPeripheralConnectionLocalSleepClockAccuracy,
    BluetoothPeripheralConnectionRecurringPhase, BluetoothPeripheralConnectionRecurringTimingError,
    BluetoothPeripheralConnectionRecurringTimingPolicy,
    BluetoothPeripheralConnectionWindowWideningMode, LE_CONNECTION_COMMON_RESERVE_MICROS,
    LE_RECURRING_FIXED_GUARD_MICROS, LE_RECURRING_RECEIVE_CPU_TIME_TAIL_MICROS,
    LE_RECURRING_SCHEDULER_BOUNDARY_GUARD_MICROS, LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS,
};
use crate::peripheral_connection::BluetoothPeripheralConnectionPacketStartTiming;
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

#[test]
fn immediate_successor_forms_all_typed_recurring_inputs() {
    let request = request(24, 4);
    let epoch = epoch(9_000);
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
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
        BluetoothPeripheralConnectionEventSpan::new(epoch.raw_duration_ticks_for_micros(
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
fn window_widening_floors_elapsed_time_to_whole_milliseconds() {
    let request = request(6, 0);
    let policy = BluetoothPeripheralConnectionRecurringTimingPolicy::new(
        Some(BluetoothPeripheralConnectionLocalSleepClockAccuracy::new(500).unwrap()),
        BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    );
    let plan = phase(10_000)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
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
    let policy = BluetoothPeripheralConnectionRecurringTimingPolicy::new(
        Some(BluetoothPeripheralConnectionLocalSleepClockAccuracy::new(79).unwrap()),
        BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
    );
    let plan = phase(10_000)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch(0),
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
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
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
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
fn scheduler_positions_wrap_without_losing_packet_start_phase() {
    let request = request(24, 4);
    let packet_start = u32::MAX - 10_000;
    let epoch = epoch(u32::MAX - 11_000);
    let plan = phase(packet_start)
        .plan(
            request,
            LePeripheralConnectionEventDelta::new(1).unwrap(),
            epoch,
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
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
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
    let local = BluetoothPeripheralConnectionLocalSleepClockAccuracy::new(60).unwrap();

    assert_eq!(
        phase(10_000).plan(
            request,
            delta,
            epoch,
            config,
            BluetoothPeripheralConnectionRecurringTimingPolicy::new(
                None,
                BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
            ),
        ),
        Err(BluetoothPeripheralConnectionRecurringTimingError::LocalSleepClockAccuracyUnknown)
    );
    assert_eq!(
        phase(10_000).plan(
            request,
            delta,
            epoch,
            config,
            BluetoothPeripheralConnectionRecurringTimingPolicy::new(
                Some(local),
                BluetoothPeripheralConnectionWindowWideningMode::Unknown,
            ),
        ),
        Err(BluetoothPeripheralConnectionRecurringTimingError::WindowWideningModeUnknown)
    );
    assert_eq!(
        phase(10_000).plan(
            request,
            delta,
            epoch,
            config,
            BluetoothPeripheralConnectionRecurringTimingPolicy::new(
                Some(local),
                BluetoothPeripheralConnectionWindowWideningMode::Automatic,
            ),
        ),
        Err(BluetoothPeripheralConnectionRecurringTimingError::AutomaticWindowWideningUnsupported)
    );
}

#[test]
fn unrepresentable_forward_or_receive_wait_range_fails_closed() {
    let epoch = epoch(0);
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();

    assert_eq!(
        phase(10_000).plan(
            request(3_200, 0),
            LePeripheralConnectionEventDelta::new(1_000).unwrap(),
            epoch,
            config,
            software_policy(),
        ),
        Err(
            BluetoothPeripheralConnectionRecurringTimingError::AnchorAdvanceOutsideForwardHalfRange
        )
    );
    assert_eq!(
        phase(10_000).plan(
            request(24, 0),
            LePeripheralConnectionEventDelta::new(10_000).unwrap(),
            epoch,
            config,
            BluetoothPeripheralConnectionRecurringTimingPolicy::new(
                Some(BluetoothPeripheralConnectionLocalSleepClockAccuracy::new(500).unwrap()),
                BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty,
            ),
        ),
        Err(BluetoothPeripheralConnectionRecurringTimingError::ReceiveWaitUnrepresentable)
    );
}

#[test]
fn actual_packet_start_resets_the_nominal_widening_phase() {
    let request = request(24, 4);
    let epoch = epoch(0);
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
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

    let actual = BluetoothPeripheralConnectionPacketStartTiming::from_scheduler_micros(
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
