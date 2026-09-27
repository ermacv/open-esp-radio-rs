//! Grant order and waiting-time estimates over conflicting claims.
//!
//! Requests are served in arrival order among those that conflict: a ticket
//! is granted once no holder and no earlier waiting ticket conflicts with it.
//! Tickets that claim disjoint resources proceed in parallel. A short ticket
//! may pass earlier conflicting tickets, but never two grants in a row.

use crate::state::{State, Ticket, conflict};

/// Whether the ticket `id` can be granted now, and whether that grant jumps
/// ahead of earlier conflicting tickets.
pub(crate) fn grantable(state: &State, id: u64) -> Option<bool> {
    let index = state.queue.iter().position(|ticket| ticket.id == id)?;
    let ticket = &state.queue[index];
    if state
        .holders
        .iter()
        .any(|holder| conflict(&holder.ticket.claims, &ticket.claims))
    {
        return None;
    }
    let behind = state.queue[..index]
        .iter()
        .any(|earlier| conflict(&earlier.claims, &ticket.claims));
    match (behind, ticket.short && !state.jumped) {
        (false, _) => Some(false),
        (true, true) => Some(true),
        (true, false) => None,
    }
}

/// Whether a waiting ticket conflicts with the claims of holder `id`.
pub(crate) fn blocks_waiters(state: &State, id: u64) -> bool {
    state
        .holders
        .iter()
        .find(|holder| holder.ticket.id == id)
        .is_some_and(|holder| {
            state
                .queue
                .iter()
                .any(|ticket| conflict(&ticket.claims, &holder.ticket.claims))
        })
}

/// Whether a waiting ticket of at most `brief_secs` conflicts with the claims
/// of holder `id`.
pub(crate) fn blocks_brief_waiters(state: &State, id: u64, brief_secs: u64) -> bool {
    state
        .holders
        .iter()
        .find(|holder| holder.ticket.id == id)
        .is_some_and(|holder| {
            state.queue.iter().any(|ticket| {
                ticket.budget_secs <= brief_secs && conflict(&ticket.claims, &holder.ticket.claims)
            })
        })
}

/// Earlier conflicting tickets and the expected wait in seconds of every
/// queued ticket, assuming every lease uses its budget.
pub(crate) fn expected_starts(state: &State, now: u64) -> Vec<(u64, usize, u64)> {
    let remaining = |ticket: &Ticket| {
        state
            .holders
            .iter()
            .filter(|holder| conflict(&holder.ticket.claims, &ticket.claims))
            .map(|holder| (holder.granted_unix + holder.ticket.budget_secs).saturating_sub(now))
            .max()
            .unwrap_or(0)
    };
    state
        .queue
        .iter()
        .enumerate()
        .map(|(index, ticket)| {
            let ahead = state.queue[..index]
                .iter()
                .filter(|earlier| conflict(&earlier.claims, &ticket.claims))
                .collect::<Vec<_>>();
            let queued = ahead.iter().map(|earlier| earlier.budget_secs).sum::<u64>();
            (ticket.id, ahead.len(), remaining(ticket) + queued)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        budget::BudgetSource,
        process::ProcessIdentity,
        state::{AIR, Claim, Holder},
    };

    pub(crate) fn ticket(id: u64, short: bool, budget_secs: u64, claims: Vec<Claim>) -> Ticket {
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
            claims,
        }
    }

    fn board(name: &str) -> Vec<Claim> {
        vec![Claim::board(name), Claim::shared(AIR)]
    }

    fn holder(ticket: Ticket) -> Holder {
        Holder {
            ticket,
            token: "t".into(),
            granted_unix: 1000,
            over_budget: false,
        }
    }

    #[test]
    fn disjoint_requests_proceed_while_conflicting_ones_keep_arrival_order() {
        let state = State {
            holders: vec![holder(ticket(1, false, 300, board("esp32s31")))],
            queue: vec![
                ticket(2, false, 60, board("esp32s31")),
                ticket(3, false, 60, board("esp32c5")),
                ticket(
                    4,
                    false,
                    60,
                    vec![Claim::board("esp32c5"), Claim::exclusive(AIR)],
                ),
            ],
            ..State::default()
        };
        assert_eq!(grantable(&state, 2), None, "esp32s31 is held");
        assert_eq!(grantable(&state, 3), Some(false), "esp32c5 is free");
        assert_eq!(
            grantable(&state, 4),
            None,
            "exclusive air waits for the holder"
        );
        assert!(blocks_waiters(&state, 1));
    }

    #[test]
    fn a_short_ticket_passes_earlier_conflicting_ones_once() {
        let mut state = State {
            queue: vec![
                ticket(1, false, 600, board("esp32s31")),
                ticket(2, true, 60, board("esp32s31")),
            ],
            holders: vec![],
            ..State::default()
        };
        // A free board goes to the earliest ticket.
        assert_eq!(grantable(&state, 1), Some(false));
        assert_eq!(grantable(&state, 2), Some(true));
        state.jumped = true;
        assert_eq!(grantable(&state, 2), None);
    }

    #[test]
    fn expected_starts_count_only_conflicting_work() {
        let state = State {
            holders: vec![holder(ticket(9, false, 300, board("esp32s31")))],
            queue: vec![
                ticket(1, false, 600, board("esp32s31")),
                ticket(2, false, 60, board("esp32c5")),
                ticket(3, false, 60, board("esp32s31")),
            ],
            ..State::default()
        };
        assert_eq!(
            expected_starts(&state, 1100),
            [(1, 0, 200), (2, 0, 0), (3, 1, 800)]
        );
    }
}
