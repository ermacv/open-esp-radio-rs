//! Connection event geometry over the backend's connection allowances.
//!
//! The Link Layer anchors every event on the central's first packet. Until a
//! packet has been received the anchor is the start of a transmit window of
//! uncertain position, which recurs at every interval. Clock drift widens the
//! receive window by the sum of both sleep-clock accuracies over the time since
//! the last received anchor (Core Vol 6 Part B 4.2.4); the backend adds its
//! jitter and guards as [`ConnectionAllowances`] describes.

use oer_bluetooth_radio::{
    ConnectionAllowances, ConnectionEventTiming, RadioDuration, RadioInstant, RadioWindow,
};

/// The anchor phase of a connection between events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Phase {
    /// Nominal anchor of the last event.
    pub(crate) anchor: RadioInstant,
    /// The last anchor a received packet established; drift accumulates from
    /// here.
    pub(crate) reference: RadioInstant,
    /// Width of the transmit window still uncertain at the anchor, zero once a
    /// packet fixed the anchor.
    pub(crate) transmit_window: u32,
}

/// One planned connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Plan {
    pub(crate) anchor: RadioInstant,
    pub(crate) window: RadioWindow,
    pub(crate) timing: ConnectionEventTiming,
    pub(crate) transmit_window: u32,
}

/// The first event, anchored at the start of its transmit window.
pub(crate) fn first(
    anchor: RadioInstant,
    transmit_window: u32,
    allowances: ConnectionAllowances,
) -> Option<Plan> {
    let guard = allowances.first_event_guard.as_micros();
    let boundary = allowances.boundary_guard.as_micros();
    let start = anchor
        .as_micros()
        .checked_sub(u64::from(guard + boundary))?;
    let duration = guard + boundary + transmit_window + allowances.first_event_length.as_micros();
    Some(Plan {
        anchor,
        window: RadioWindow::new(
            RadioInstant::from_micros(start),
            RadioDuration::from_micros(duration),
        )
        .ok()?,
        timing: ConnectionEventTiming::First {
            transmit_window: RadioDuration::from_micros(transmit_window),
            timing_guard: allowances.first_event_guard,
        },
        transmit_window,
    })
}

/// A later event at `anchor` with `transmit_window` still uncertain.
pub(crate) fn recurring(
    anchor: RadioInstant,
    reference: RadioInstant,
    transmit_window: u32,
    peer_sleep_clock_ppm: u16,
    allowances: ConnectionAllowances,
) -> Option<Plan> {
    let widening = widening(
        anchor.as_micros().checked_sub(reference.as_micros())?,
        peer_sleep_clock_ppm,
        allowances,
    )?;
    let guard = allowances.receive_guard.as_micros();
    let boundary = allowances.boundary_guard.as_micros();
    let start = anchor
        .as_micros()
        .checked_sub(u64::from(guard + widening + boundary))?;
    let duration = guard
        .checked_add(boundary)?
        .checked_add(widening.checked_mul(2)?)?
        .checked_add(transmit_window)?
        .checked_add(allowances.event_length.as_micros())?;
    let receive_wait = guard
        .checked_add(widening.checked_mul(2)?)?
        .checked_add(transmit_window)?
        .checked_add(allowances.receive_tail.as_micros())?;
    Some(Plan {
        anchor,
        window: RadioWindow::new(
            RadioInstant::from_micros(start),
            RadioDuration::from_micros(duration),
        )
        .ok()?,
        timing: ConnectionEventTiming::Recurring {
            receive_wait: RadioDuration::from_micros(receive_wait),
        },
        transmit_window,
    })
}

/// Window widening after `elapsed` microseconds: whole milliseconds times the
/// combined accuracy, truncated as the vendor computes it, plus the jitter.
fn widening(elapsed: u64, peer_ppm: u16, allowances: ConnectionAllowances) -> Option<u32> {
    let ppm = u64::from(peer_ppm) + u64::from(allowances.local_sleep_clock_ppm);
    let drift = (elapsed / 1_000).checked_mul(ppm)? / 1_000;
    u32::try_from(drift)
        .ok()?
        .checked_add(allowances.widening_jitter.as_micros())
}

#[cfg(test)]
mod tests {
    use oer_bluetooth_radio::{
        ConnectionAllowances, ConnectionEventTiming, RadioDuration, RadioInstant,
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
        let plan = first(RadioInstant::from_micros(100_000), 2_500, S31).unwrap();
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
        let reference = RadioInstant::from_micros(100_000);
        // 50 ms at 500 + 50 ppm: floor(50 * 550 / 1000) = 27 us, plus 63.
        let anchor = RadioInstant::from_micros(150_000);
        let plan = recurring(anchor, reference, 0, 50, S31).unwrap();
        let widening: u32 = 27 + 63;
        assert_eq!(
            plan.window.start().as_micros(),
            150_000 - 10 - u64::from(widening) - 1
        );
        assert_eq!(
            plan.window.end().as_micros(),
            150_000 + u64::from(widening) + 5_047
        );
        assert_eq!(
            plan.timing,
            ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(10 + 2 * widening + 2),
            }
        );
        // An uncertain transmit window stays part of every listening.
        let uncertain = recurring(anchor, reference, 1_250, 50, S31).unwrap();
        assert_eq!(
            uncertain.timing,
            ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(10 + 2 * widening + 1_250 + 2),
            }
        );
        assert_eq!(
            uncertain.window.duration().as_micros(),
            plan.window.duration().as_micros() + 1_250
        );
    }
}
