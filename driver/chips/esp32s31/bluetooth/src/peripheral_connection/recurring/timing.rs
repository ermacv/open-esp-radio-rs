//! Pure recurring timing for one ESP32-S31 peripheral connection.
//!
//! A completed receive capture supplies the causal packet-start phase. This
//! module advances that phase only by a portable Link Layer event delta and
//! forms semantic memory inputs for a future scheduler admission attempt. It
//! neither samples `now()` nor publishes controller SRAM or MMIO.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::connection::{
    LeLegacyConnectionRequest, LePeripheralConnectionEventDelta,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionEventSpan, BluetoothPeripheralConnectionRecurringReceiveWait,
};

use crate::peripheral_connection::BluetoothPeripheralConnectionPacketStartTiming;
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothSchedulerInstant, BluetoothSchedulerRawWindow,
    BluetoothSchedulerSoftwareConfig,
};

const MICROS_PER_MILLISECOND: u32 = 1_000;
const MILLISECONDS_PER_SECOND: u32 = 1_000;

// Source-owned S31 recurring-event policy. The exact provenance of these
// physical allowances is recorded in the recurring-event section of
// verification/vendor/targets/esp32s31/analysis/bluetooth-peripheral-connection.md.
// They are deliberately not descriptor images.
const LE_1M_RECURRING_EVENT_MICROS: u32 = 5_154;
const LE_CONNECTION_COMMON_RESERVE_MICROS: u32 = 440;
const LE_RECURRING_FIXED_GUARD_MICROS: u32 = 10;
const LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS: u32 = 63;
const LE_RECURRING_SCHEDULER_BOUNDARY_GUARD_MICROS: u32 = 1;
const LE_RECURRING_RECEIVE_CPU_TIME_TAIL_MICROS: u32 = 2;
const MAX_FORWARD_MICROS: u32 = i32::MAX as u32;

/// Reviewed maximum error of the local sleep clock, in parts per million.
///
/// This value is deliberately distinct from the Central's SCA class carried
/// by `CONNECT_IND`. A future powered source must supply the local clock fact;
/// absence cannot silently select an accuracy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothPeripheralConnectionLocalSleepClockAccuracy {
    worst_case_ppm: u16,
}

impl BluetoothPeripheralConnectionLocalSleepClockAccuracy {
    pub(crate) const fn new(worst_case_ppm: u16) -> Option<Self> {
        if worst_case_ppm > 500 {
            None
        } else {
            Some(Self { worst_case_ppm })
        }
    }

    const fn worst_case_ppm(self) -> u32 {
        self.worst_case_ppm as u32
    }
}

/// Source-owned selection of the S31 window-widening implementation.
///
/// No raw controller flag crosses this boundary. Only the reviewed software
/// path with zero accumulated anchor uncertainty can currently form a plan;
/// no untyped uncertainty value can enter the calculation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothPeripheralConnectionWindowWideningMode {
    #[allow(
        dead_code,
        reason = "an explicit unknown hardware mode must fail closed if a future source supplies it"
    )]
    Unknown,
    /// Reviewed software calculation immediately after actual-anchor correction.
    ///
    /// The vendor's accumulated anchor uncertainty is zero at this boundary.
    /// A nonzero form is intentionally absent until a typed source owns it.
    SoftwareZeroAccumulatedUncertainty,
    #[allow(
        dead_code,
        reason = "automatic widening stays explicitly unsupported until a typed hardware source exists"
    )]
    Automatic,
}

/// Physical authority needed to calculate one recurring receive window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothPeripheralConnectionRecurringTimingPolicy {
    local_sleep_clock_accuracy: Option<BluetoothPeripheralConnectionLocalSleepClockAccuracy>,
    window_widening_mode: BluetoothPeripheralConnectionWindowWideningMode,
}

impl BluetoothPeripheralConnectionRecurringTimingPolicy {
    pub(crate) const fn new(
        local_sleep_clock_accuracy: Option<BluetoothPeripheralConnectionLocalSleepClockAccuracy>,
        window_widening_mode: BluetoothPeripheralConnectionWindowWideningMode,
    ) -> Self {
        Self {
            local_sleep_clock_accuracy,
            window_widening_mode,
        }
    }
}

/// Nominal connection phase and its actual-packet widening reference.
///
/// Connection intervals have an exact integer-microsecond representation, so
/// this source-owned phase carries no detached tick-conversion fraction or
/// cached widening. An actual capture becomes both the corrected nominal
/// anchor and the new widening reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the connection phase must be retained until the next timing decision"]
pub(crate) struct BluetoothPeripheralConnectionRecurringPhase {
    nominal_anchor: BluetoothSchedulerInstant,
    window_widening_reference: BluetoothSchedulerInstant,
}

impl BluetoothPeripheralConnectionRecurringPhase {
    /// Seed the first recurring phase from the planned first-event anchor.
    pub(crate) const fn from_nominal_anchor(nominal_anchor: BluetoothSchedulerInstant) -> Self {
        Self {
            nominal_anchor,
            window_widening_reference: nominal_anchor,
        }
    }

    /// Correct the phase from an actual normalized packet-start capture.
    ///
    /// The actual start becomes the new nominal anchor and widening reference.
    /// Exact-microsecond intervals and the supported zero-accumulated policy
    /// leave no fractional or widening cache to preserve across correction.
    pub(crate) const fn correct_from_normalized_packet_start(
        self,
        packet_start: &BluetoothPeripheralConnectionPacketStartTiming,
    ) -> Self {
        let actual = packet_start.scheduler_instant();
        Self {
            nominal_anchor: actual,
            window_widening_reference: actual,
        }
    }

    /// Form an unpublished recurrence candidate at exactly `delta` intervals.
    ///
    /// The caller owns skipped-event and timeout policy. Arithmetic which can
    /// no longer be ordered in the scheduler's signed wrapping half-domain is
    /// rejected instead of aliasing an old phase.
    pub(crate) fn plan(
        self,
        request: LeLegacyConnectionRequest,
        delta: LePeripheralConnectionEventDelta,
        epoch: BluetoothControllerSchedulerEpoch,
        scheduler_config: BluetoothSchedulerSoftwareConfig,
        timing_policy: BluetoothPeripheralConnectionRecurringTimingPolicy,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringEventPlan,
        BluetoothPeripheralConnectionRecurringTimingError,
    > {
        match timing_policy.window_widening_mode {
            BluetoothPeripheralConnectionWindowWideningMode::Unknown => {
                return Err(
                    BluetoothPeripheralConnectionRecurringTimingError::WindowWideningModeUnknown,
                );
            }
            BluetoothPeripheralConnectionWindowWideningMode::Automatic => {
                return Err(
                    BluetoothPeripheralConnectionRecurringTimingError::AutomaticWindowWideningUnsupported,
                );
            }
            BluetoothPeripheralConnectionWindowWideningMode::SoftwareZeroAccumulatedUncertainty => {
            }
        }
        let Some(local_sleep_clock_accuracy) = timing_policy.local_sleep_clock_accuracy else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::LocalSleepClockAccuracyUnknown,
            );
        };

        let interval_micros = request.timing().interval_micros();
        let Some(anchor_advance_micros) = interval_micros.checked_mul(delta.get() as u32) else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::AnchorAdvanceOutsideForwardHalfRange,
            );
        };
        let elapsed_before = self
            .nominal_anchor
            .image()
            .wrapping_sub(self.window_widening_reference.image());
        let Some(elapsed_since_reference_micros) =
            elapsed_before.checked_add(anchor_advance_micros)
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::AnchorAdvanceOutsideForwardHalfRange,
            );
        };
        if elapsed_before > MAX_FORWARD_MICROS
            || anchor_advance_micros > MAX_FORWARD_MICROS
            || elapsed_since_reference_micros > MAX_FORWARD_MICROS
        {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::AnchorAdvanceOutsideForwardHalfRange,
            );
        }

        let Some(total_sca_ppm) = u32::from(request.sleep_clock_accuracy().worst_case_ppm())
            .checked_add(local_sleep_clock_accuracy.worst_case_ppm())
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::WindowWideningUnrepresentable,
            );
        };
        let elapsed_millis = elapsed_since_reference_micros / MICROS_PER_MILLISECOND;
        let Some(ppm_milliseconds) = total_sca_ppm.checked_mul(elapsed_millis) else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::WindowWideningUnrepresentable,
            );
        };
        // The reviewed vendor helper truncates elapsed microseconds to whole
        // milliseconds, then truncates `(elapsed_ms * total_ppm) / 1000`.
        let calculated_window_widening_micros = ppm_milliseconds / MILLISECONDS_PER_SECOND;
        let Some(window_widening_micros) = calculated_window_widening_micros
            .checked_add(LE_RECURRING_WINDOW_WIDENING_JITTER_MICROS)
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::WindowWideningUnrepresentable,
            );
        };

        let Some(double_window_widening) = window_widening_micros.checked_mul(2) else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::ReceiveWaitUnrepresentable,
            );
        };
        let Some(receive_wait_micros) = LE_RECURRING_FIXED_GUARD_MICROS
            .checked_add(double_window_widening)
            .and_then(|total| total.checked_add(LE_RECURRING_RECEIVE_CPU_TIME_TAIL_MICROS))
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::ReceiveWaitUnrepresentable,
            );
        };
        let Some(receive_wait) =
            BluetoothPeripheralConnectionRecurringReceiveWait::new(receive_wait_micros)
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::ReceiveWaitUnrepresentable,
            );
        };

        let Some(event_span_micros) =
            interval_micros.checked_sub(LE_CONNECTION_COMMON_RESERVE_MICROS)
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::EventSpanUnrepresentable,
            );
        };
        let Some(event_span) = BluetoothPeripheralConnectionEventSpan::new(
            epoch.raw_duration_ticks_for_micros(event_span_micros),
        ) else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::EventSpanUnrepresentable,
            );
        };

        let proposed_anchor = self.nominal_anchor.wrapping_add(anchor_advance_micros);
        let Some(window_start_offset) = scheduler_config
            .preparation_lead_micros()
            .checked_add(LE_RECURRING_FIXED_GUARD_MICROS)
            .and_then(|offset| offset.checked_add(window_widening_micros))
            .and_then(|offset| offset.checked_add(LE_RECURRING_SCHEDULER_BOUNDARY_GUARD_MICROS))
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::SchedulerWindowUnrepresentable,
            );
        };
        let window_start = BluetoothSchedulerInstant::from_image(
            proposed_anchor.image().wrapping_sub(window_start_offset),
        );
        let Some(window_end_offset) =
            LE_1M_RECURRING_EVENT_MICROS.checked_add(window_widening_micros)
        else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::SchedulerWindowUnrepresentable,
            );
        };
        let window_end = BluetoothSchedulerInstant::from_image(
            proposed_anchor
                .image()
                .wrapping_sub(scheduler_config.preparation_lead_micros())
                .wrapping_add(window_end_offset),
        );
        let Some(window) = BluetoothSchedulerRawWindow::from_projected_scheduler_window(
            epoch.raw_ticks_for_micros(window_start.image()),
            epoch.raw_ticks_for_micros(window_end.image()),
        ) else {
            return Err(
                BluetoothPeripheralConnectionRecurringTimingError::SchedulerWindowUnrepresentable,
            );
        };

        Ok(BluetoothPeripheralConnectionRecurringEventPlan {
            delta,
            proposed_phase: Self {
                nominal_anchor: proposed_anchor,
                window_widening_reference: self.window_widening_reference,
            },
            proposed_anchor,
            window,
            event_span,
            receive_wait,
            window_widening_micros,
        })
    }
}

/// Complete pure input set for one future recurring scheduler admission.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the recurring plan must enter lower admission or be discarded unchanged"]
pub(crate) struct BluetoothPeripheralConnectionRecurringEventPlan {
    delta: LePeripheralConnectionEventDelta,
    proposed_phase: BluetoothPeripheralConnectionRecurringPhase,
    proposed_anchor: BluetoothSchedulerInstant,
    window: BluetoothSchedulerRawWindow,
    event_span: BluetoothPeripheralConnectionEventSpan,
    receive_wait: BluetoothPeripheralConnectionRecurringReceiveWait,
    window_widening_micros: u32,
}

impl BluetoothPeripheralConnectionRecurringEventPlan {
    #[cfg(test)]
    pub(crate) const fn delta(&self) -> LePeripheralConnectionEventDelta {
        self.delta
    }

    #[cfg(test)]
    pub(crate) const fn proposed_anchor(&self) -> BluetoothSchedulerInstant {
        self.proposed_anchor
    }

    #[cfg(test)]
    pub(crate) const fn window(&self) -> BluetoothSchedulerRawWindow {
        self.window
    }

    #[cfg(test)]
    pub(crate) const fn event_span(&self) -> BluetoothPeripheralConnectionEventSpan {
        self.event_span
    }

    #[cfg(test)]
    pub(crate) const fn receive_wait(&self) -> BluetoothPeripheralConnectionRecurringReceiveWait {
        self.receive_wait
    }

    pub(crate) const fn window_widening_micros(&self) -> u32 {
        self.window_widening_micros
    }

    /// Decompose pure derived values for a future combined LL/phase owner.
    ///
    /// This is not a commit edge. The later admission transition must retain
    /// the original phase together with the provisional Link Layer owner and
    /// install this proposed phase only when both can advance atomically.
    pub(crate) const fn into_parts(
        self,
    ) -> (
        LePeripheralConnectionEventDelta,
        BluetoothPeripheralConnectionRecurringPhase,
        BluetoothSchedulerInstant,
        BluetoothSchedulerRawWindow,
        BluetoothPeripheralConnectionEventSpan,
        BluetoothPeripheralConnectionRecurringReceiveWait,
    ) {
        (
            self.delta,
            self.proposed_phase,
            self.proposed_anchor,
            self.window,
            self.event_span,
            self.receive_wait,
        )
    }
}

/// Why pure recurring timing could not form a lossless scheduler candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionRecurringTimingError {
    WindowWideningModeUnknown,
    AutomaticWindowWideningUnsupported,
    LocalSleepClockAccuracyUnknown,
    AnchorAdvanceOutsideForwardHalfRange,
    WindowWideningUnrepresentable,
    EventSpanUnrepresentable,
    ReceiveWaitUnrepresentable,
    SchedulerWindowUnrepresentable,
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::connection::{
        LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES, LeDataChannelMap,
        LeLegacyConnectionRequest, LePeripheralConnectionEventDelta,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionEventSpan;
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{
        BluetoothPeripheralConnectionLocalSleepClockAccuracy,
        BluetoothPeripheralConnectionRecurringPhase,
        BluetoothPeripheralConnectionRecurringTimingError,
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
            LE_RECURRING_FIXED_GUARD_MICROS
                + 2 * widening
                + LE_RECURRING_RECEIVE_CPU_TIME_TAIL_MICROS
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
            Err(
                BluetoothPeripheralConnectionRecurringTimingError::AutomaticWindowWideningUnsupported
            )
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
}
