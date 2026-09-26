//! Placement of radio events without overlapping reservations.
//!
//! Every event reserves its air window plus the radio's preparation lead
//! before it. A role proposes the earliest start it accepts and the latest
//! start it still accepts; the arbiter moves the start past every busy
//! reservation. A proposal that cannot start by its latest start is not
//! placed, and the role skips that event.

use oer_bluetooth_radio::{RadioDuration, RadioInstant, RadioWindow};

/// One event a role wants to schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) earliest: RadioInstant,
    pub(crate) latest: RadioInstant,
    pub(crate) duration: RadioDuration,
}

/// The earliest start in `[earliest, latest]` whose reservation
/// `[start - lead, start + duration)` overlaps no busy reservation.
pub(crate) fn place(
    proposal: Proposal,
    lead: RadioDuration,
    busy: &[Option<RadioWindow>],
) -> Option<RadioInstant> {
    let mut start = proposal.earliest;
    // Each pass either settles or moves past one busy reservation.
    for _ in 0..=busy.len() {
        let reservation = reservation(start, proposal.duration, lead)?;
        match busy
            .iter()
            .flatten()
            .filter(|window| window.overlaps(reservation))
            .map(|window| window.end())
            .max()
        {
            None => return (start <= proposal.latest).then_some(start),
            Some(end) => start = end.checked_add(lead)?,
        }
    }
    None
}

/// The reservation of an air window starting at `start`.
pub(crate) fn reservation(
    start: RadioInstant,
    duration: RadioDuration,
    lead: RadioDuration,
) -> Option<RadioWindow> {
    let begin = start.as_micros().checked_sub(u64::from(lead.as_micros()))?;
    RadioWindow::new(
        RadioInstant::from_micros(begin),
        RadioDuration::from_micros(duration.as_micros().checked_add(lead.as_micros())?),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use oer_bluetooth_radio::{RadioDuration, RadioInstant, RadioWindow};

    use super::{Proposal, place, reservation};

    const LEAD: RadioDuration = RadioDuration::from_micros(100);

    fn proposal(earliest: u64, latest: u64, duration: u32) -> Proposal {
        Proposal {
            earliest: RadioInstant::from_micros(earliest),
            latest: RadioInstant::from_micros(latest),
            duration: RadioDuration::from_micros(duration),
        }
    }

    fn busy(start: u64, duration: u32) -> Option<RadioWindow> {
        reservation(
            RadioInstant::from_micros(start),
            RadioDuration::from_micros(duration),
            LEAD,
        )
    }

    #[test]
    fn a_free_timeline_keeps_the_earliest_start() {
        assert_eq!(
            place(proposal(1_000, 2_000, 500), LEAD, &[None, busy(5_000, 100)]),
            Some(RadioInstant::from_micros(1_000))
        );
    }

    #[test]
    fn a_start_moves_past_every_busy_reservation_including_the_lead() {
        // Busy 1_000..1_500 (reservation 900..1_500); then 1_700..1_800.
        let busy = [busy(1_000, 500), busy(1_700, 100)];
        assert_eq!(
            place(proposal(1_000, 5_000, 200), LEAD, &busy),
            Some(RadioInstant::from_micros(1_900))
        );
        assert_eq!(place(proposal(1_000, 1_800, 200), LEAD, &busy), None);
    }
}
