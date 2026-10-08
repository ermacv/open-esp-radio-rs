//! Connection event geometry over the backend's connection allowances.
//!
//! The Link Layer anchors every event on the central's first packet. Until a
//! packet has been received the anchor is the start of a transmit window of
//! uncertain position, which recurs at every interval. Clock drift widens the
//! receive window by the sum of both sleep-clock accuracies over the time since
//! the last received anchor (Core Vol 6 Part B 4.2.4); the backend adds its
//! jitter and guards as [`ConnectionAllowances`] describes.

use oer_bluetooth_radio::{
    ConnectionAllowances, ConnectionEventTiming, LeInstant, LeWindow, RadioDuration, TimingError,
};

/// The anchor phase of a connection between events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Phase {
    /// Nominal anchor of the last event.
    pub(crate) anchor: LeInstant,
    /// The last anchor a received packet established; drift accumulates from
    /// here.
    pub(crate) reference: LeInstant,
    /// Width of the transmit window still uncertain at the anchor, zero once a
    /// packet fixed the anchor.
    pub(crate) transmit_window: RadioDuration,
}

/// One planned connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Plan {
    pub(crate) anchor: LeInstant,
    pub(crate) window: LeWindow,
    pub(crate) timing: ConnectionEventTiming,
    pub(crate) transmit_window: RadioDuration,
}

/// The first event, anchored at the start of its transmit window.
// CAPABILITY: bluetooth-first-peripheral-connection-window
pub(crate) fn first(
    anchor: LeInstant,
    transmit_window: RadioDuration,
    allowances: ConnectionAllowances,
) -> Result<Plan, crate::PlanningError> {
    let lead = allowances
        .first_event_guard
        .checked_add(allowances.boundary_guard)
        .ok_or(error(
            crate::PlanningCalculation::WindowGeometry,
            TimingError::DurationOverflow,
        ))?;
    let start = anchor.checked_sub(lead).ok_or(error(
        crate::PlanningCalculation::WindowGeometry,
        TimingError::BeforeEpoch,
    ))?;
    let duration = lead
        .checked_add(transmit_window)
        .and_then(|span| span.checked_add(allowances.first_event_length))
        .ok_or(error(
            crate::PlanningCalculation::WindowGeometry,
            TimingError::DurationOverflow,
        ))?;
    Ok(Plan {
        anchor,
        window: LeWindow::new(start, duration).map_err(|cause| {
            error(
                crate::PlanningCalculation::WindowGeometry,
                TimingError::Window(cause),
            )
        })?,
        timing: ConnectionEventTiming::First {
            transmit_window,
            timing_guard: allowances.first_event_guard,
        },
        transmit_window,
    })
}

/// A later event with typed elapsed drift and uncertain transmit window.
// CAPABILITY: bluetooth-recurring-peripheral-events, bluetooth-sleep-clock-accuracy-window-widening
pub(crate) fn recurring(
    anchor: LeInstant,
    reference: LeInstant,
    transmit_window: RadioDuration,
    peer_sleep_clock_ppm: u16,
    allowances: ConnectionAllowances,
) -> Result<Plan, crate::PlanningError> {
    let elapsed = anchor.checked_duration_since(reference).ok_or(error(
        crate::PlanningCalculation::Elapsed,
        TimingError::ReversedTime,
    ))?;
    let widening = widening(elapsed, peer_sleep_clock_ppm, allowances)?;
    let lead = allowances
        .receive_guard
        .checked_add(widening)
        .and_then(|span| span.checked_add(allowances.boundary_guard))
        .ok_or(error(
            crate::PlanningCalculation::WindowGeometry,
            TimingError::DurationOverflow,
        ))?;
    let start = anchor.checked_sub(lead).ok_or(error(
        crate::PlanningCalculation::WindowGeometry,
        TimingError::BeforeEpoch,
    ))?;
    let twice = widening.checked_mul(2).ok_or(error(
        crate::PlanningCalculation::WindowGeometry,
        TimingError::DurationOverflow,
    ))?;
    let duration = allowances
        .receive_guard
        .checked_add(allowances.boundary_guard)
        .and_then(|span| span.checked_add(twice))
        .and_then(|span| span.checked_add(transmit_window))
        .and_then(|span| span.checked_add(allowances.event_length))
        .ok_or(error(
            crate::PlanningCalculation::WindowGeometry,
            TimingError::DurationOverflow,
        ))?;
    let receive_wait = allowances
        .receive_guard
        .checked_add(twice)
        .and_then(|span| span.checked_add(transmit_window))
        .and_then(|span| span.checked_add(allowances.receive_tail))
        .ok_or(error(
            crate::PlanningCalculation::WindowGeometry,
            TimingError::DurationOverflow,
        ))?;
    Ok(Plan {
        anchor,
        window: LeWindow::new(start, duration).map_err(|cause| {
            error(
                crate::PlanningCalculation::WindowGeometry,
                TimingError::Window(cause),
            )
        })?,
        timing: ConnectionEventTiming::Recurring {
            receive_wait,
            widening,
        },
        transmit_window,
    })
}

/// Whole milliseconds times the combined accuracy, truncated as the vendor
/// computes it, then jitter. The u64 span times two u16 ppm coefficients fits
/// u128; do not reject representable final drift through intermediate overflow.
fn widening(
    elapsed: RadioDuration,
    peer_ppm: u16,
    allowances: ConnectionAllowances,
) -> Result<RadioDuration, crate::PlanningError> {
    let ppm = u128::from(peer_ppm) + u128::from(allowances.local_sleep_clock_ppm);
    let drift = (u128::from(elapsed.as_micros()) / 1_000) * ppm / 1_000;
    let drift = u64::try_from(drift).map_err(|_| {
        error(
            crate::PlanningCalculation::Widening,
            TimingError::DurationOverflow,
        )
    })?;
    RadioDuration::from_micros(drift)
        .checked_add(allowances.widening_jitter)
        .ok_or(error(
            crate::PlanningCalculation::Widening,
            TimingError::DurationOverflow,
        ))
}

fn error(calculation: crate::PlanningCalculation, cause: TimingError) -> crate::PlanningError {
    crate::PlanningError::timing(
        crate::PlanningRole::Peripheral,
        crate::PlanningOperation::Event,
        calculation,
        cause,
    )
}

#[cfg(test)]
mod tests {
    use oer_bluetooth_radio::{
        ConnectionAllowances, ConnectionEventTiming, LeInstant, RadioDuration,
    };

    use super::{first, recurring};

    const S31: ConnectionAllowances = ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: RadioDuration::from_micros(1),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5_047),
        first_event_length: RadioDuration::from_micros(5_155),
    };

    #[test]
    fn the_first_event_listens_across_its_transmit_window() {
        let plan = first(
            LeInstant::from_micros(100_000),
            RadioDuration::from_micros(2_500),
            S31,
        )
        .unwrap();
        assert_eq!(plan.window.start().as_micros(), 100_000 - 17);
        assert_eq!(plan.window.end().as_micros(), 100_000 + 2_500 + 5_155);
        assert_eq!(
            plan.timing,
            ConnectionEventTiming::First {
                transmit_window: RadioDuration::from_micros(2_500),
                timing_guard: RadioDuration::from_micros(16),
            }
        );
    }

    #[test]
    fn recurring_events_widen_with_elapsed_time_and_both_clocks() {
        let reference = LeInstant::from_micros(100_000);
        // 50 ms at 500 + 50 ppm: floor(50 * 550 / 1000) = 27 us, plus 63.
        let anchor = LeInstant::from_micros(150_000);
        let plan = recurring(anchor, reference, RadioDuration::from_micros(0), 50, S31).unwrap();
        let widening: u64 = 27 + 63;
        assert_eq!(plan.window.start().as_micros(), 150_000 - 10 - widening - 1);
        assert_eq!(plan.window.end().as_micros(), 150_000 + widening + 5_047);
        assert_eq!(
            plan.timing,
            ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(10 + 2 * widening + 2),
                widening: RadioDuration::from_micros(widening),
            }
        );
        // An uncertain transmit window stays part of every listening.
        let uncertain = recurring(
            anchor,
            reference,
            RadioDuration::from_micros(1_250),
            50,
            S31,
        )
        .unwrap();
        assert_eq!(
            uncertain.timing,
            ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(10 + 2 * widening + 1_250 + 2),
                widening: RadioDuration::from_micros(widening),
            }
        );
        assert_eq!(
            uncertain.window.duration().as_micros(),
            plan.window.duration().as_micros() + 1_250
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use crate::{PlanningCalculation, PlanningCause};

    const ALLOWANCES: ConnectionAllowances = ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: RadioDuration::from_micros(1),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5_047),
        first_event_length: RadioDuration::from_micros(5_155),
    };

    #[test]
    fn widening_accepts_large_spans_without_intermediate_narrowing() {
        let elapsed = RadioDuration::from_micros(u64::MAX);
        let wide = widening(elapsed, u16::MAX, ALLOWANCES).unwrap();
        let expected = (u128::from(u64::MAX) / 1_000) * (u128::from(u16::MAX) + 500) / 1_000 + 63;
        assert_eq!(u128::from(wide.as_micros()), expected);
        assert!(wide.as_micros() > u64::from(u32::MAX));
        let overflow = ConnectionAllowances {
            widening_jitter: elapsed,
            ..ALLOWANCES
        };
        assert!(matches!(
            widening(RadioDuration::from_micros(1_000_000), 50, overflow),
            Err(crate::PlanningError {
                calculation: PlanningCalculation::Widening,
                cause: PlanningCause::Timing(TimingError::DurationOverflow),
                ..
            })
        ));
    }

    #[test]
    fn first_and_recurring_geometry_refuse_epoch_boundaries_and_reversal() {
        let zero = RadioDuration::from_micros(0);
        assert!(matches!(
            first(LeInstant::from_micros(16), zero, ALLOWANCES),
            Err(crate::PlanningError {
                cause: PlanningCause::Timing(TimingError::BeforeEpoch),
                ..
            })
        ));
        assert_eq!(
            first(LeInstant::from_micros(u64::MAX - 5_155), zero, ALLOWANCES)
                .unwrap()
                .window
                .end(),
            LeInstant::from_micros(u64::MAX)
        );
        assert!(first(LeInstant::from_micros(u64::MAX - 5_154), zero, ALLOWANCES).is_err());
        assert!(matches!(
            recurring(
                LeInstant::from_micros(100),
                LeInstant::from_micros(101),
                zero,
                50,
                ALLOWANCES
            ),
            Err(crate::PlanningError {
                cause: PlanningCause::Timing(TimingError::ReversedTime),
                ..
            })
        ));
    }
}
