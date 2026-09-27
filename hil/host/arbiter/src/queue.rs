//! Grant order and waiting-time estimates.

use crate::state::{State, Ticket};

/// The ticket granted next, and whether it jumps ahead of the head. A short
/// ticket may jump once; the head is served before any further jump.
pub(crate) fn next_index(queue: &[Ticket], head_next: bool) -> Option<(usize, bool)> {
    if queue.is_empty() {
        return None;
    }
    if !head_next && let Some(index) = queue.iter().skip(1).position(|ticket| ticket.short) {
        return Some((index + 1, true));
    }
    Some((0, false))
}

/// Seconds until each queued ticket is expected to be granted, in grant order,
/// assuming every lease uses its budget.
pub(crate) fn expected_starts(state: &State, now: u64) -> Vec<(u64, u64)> {
    let mut offset = state.holder.as_ref().map_or(0, |holder| {
        (holder.granted_unix + holder.ticket.budget_secs).saturating_sub(now)
    });
    let mut queue = state.queue.clone();
    let mut head_next = state.head_next;
    let mut starts = Vec::with_capacity(queue.len());
    while let Some((index, jumped)) = next_index(&queue, head_next) {
        let ticket = queue.remove(index);
        starts.push((ticket.id, offset));
        offset += ticket.budget_secs;
        head_next = jumped;
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{budget::BudgetSource, process::ProcessIdentity, state::Holder};

    pub(crate) fn ticket(id: u64, short: bool, budget_secs: u64) -> Ticket {
        Ticket {
            id,
            owner: format!("owner-{id}"),
            work: format!("work-{id}"),
            budget_secs,
            budget_source: BudgetSource::Explicit,
            short,
            process: ProcessIdentity {
                pid: 1,
                start_ticks: 1,
            },
            enqueued_unix: id,
        }
    }

    #[test]
    fn queue_is_fifo_except_one_short_jump_in_a_row() {
        let queue = [
            ticket(1, false, 60),
            ticket(2, true, 60),
            ticket(3, true, 60),
        ];
        assert_eq!(next_index(&queue, false), Some((1, true)));
        assert_eq!(next_index(&queue, true), Some((0, false)));
        assert_eq!(next_index(&queue[..1], false), Some((0, false)));
        assert_eq!(next_index(&[ticket(1, true, 60)], false), Some((0, false)));
        assert_eq!(next_index(&[], false), None);
    }

    #[test]
    fn expected_starts_follow_grant_order_after_the_holder() {
        let state = State {
            queue: vec![
                ticket(1, false, 600),
                ticket(2, true, 60),
                ticket(3, true, 60),
            ],
            holder: Some(Holder {
                ticket: ticket(9, false, 300),
                token: "t".into(),
                granted_unix: 1000,
                over_budget: false,
            }),
            ..State::default()
        };
        assert_eq!(
            expected_starts(&state, 1100),
            [(2, 200), (1, 260), (3, 860)]
        );
    }
}
