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

use crate::le::peripheral::connection::BluetoothPeripheralConnectionPacketStartTiming;
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothSchedulerInstant, BluetoothSchedulerRawWindow,
    BluetoothSchedulerSoftwareConfig,
};

const MICROS_PER_MILLISECOND: u32 = 1_000;
const MILLISECONDS_PER_SECOND: u32 = 1_000;

// Source-owned S31 recurring-event policy. The exact provenance of these
// physical allowances is recorded in the recurring-event section of
// verification/vendor/projects/esp32s31/analysis/bluetooth-peripheral-connection.md.
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
mod tests;
