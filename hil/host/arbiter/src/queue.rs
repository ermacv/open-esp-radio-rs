//! Grant order and waiting-time estimates over conflicting claims.
//!
//! Requests that conflict are served by their owners' balances
//! ([`crate::balance`]), the earlier request on a tie: a ticket is granted
//! once no holder and no waiting ticket served before it conflicts with it.
//! Tickets that claim disjoint resources proceed in parallel.

use std::time::Duration;

use crate::{
    balance,
    state::{State, Ticket, conflict},
};

/// The waiting tickets served before `ticket` that conflict with it, in
/// service order.
fn ahead<'a>(state: &'a State, ticket: &Ticket) -> Vec<&'a Ticket> {
    let mut ahead = state
        .queue
        .iter()
        .filter(|other| {
            other.id != ticket.id
                && balance::before(state, other, ticket)
                && conflict(&other.claims, &ticket.claims)
        })
        .collect::<Vec<_>>();
    ahead.sort_by(|a, b| {
        if balance::before(state, a, b) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    ahead
}

/// Whether the ticket `id` can be granted now.
pub(crate) fn grantable(state: &State, id: u64) -> bool {
    let Some(ticket) = state.queue.iter().find(|ticket| ticket.id == id) else {
        return false;
    };
    !state
        .holders
        .iter()
        .any(|holder| conflict(&holder.ticket.claims, &ticket.claims))
        && ahead(state, ticket).is_empty()
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

/// The owner and balance of the waiting ticket that holder `id` blocks and
/// whose owner's balance exceeds the holder's, when that holder has held for
/// at least `slice`: divisible work then yields at its next boundary.
pub(crate) fn outranked_by(
    state: &State,
    id: u64,
    now_unix: u64,
    slice: Duration,
) -> Option<(String, i64)> {
    let holder = state.holders.iter().find(|holder| holder.ticket.id == id)?;
    if now_unix.saturating_sub(holder.granted_unix) < slice.as_secs() {
        return None;
    }
    let own = balance::of(&state.balances, &holder.ticket.owner);
    state
        .queue
        .iter()
        .filter(|ticket| {
            ticket.owner != holder.ticket.owner && conflict(&ticket.claims, &holder.ticket.claims)
        })
        .map(|ticket| {
            (
                ticket.owner.clone(),
                balance::of(&state.balances, &ticket.owner),
            )
        })
        .filter(|(_, balance)| *balance > own)
        .max_by_key(|(_, balance)| *balance)
}

/// The conflicting tickets served earlier and the expected wait in seconds of
/// every queued ticket, from the estimates of the leases ahead of it.
pub(crate) fn expected_starts(state: &State, now: u64) -> Vec<(u64, usize, u64)> {
    let remaining = |ticket: &Ticket| {
        state
            .holders
            .iter()
            .filter(|holder| conflict(&holder.ticket.claims, &ticket.claims))
            .map(|holder| (holder.granted_unix + holder.ticket.estimate_secs).saturating_sub(now))
            .max()
            .unwrap_or(0)
    };
    state
        .queue
        .iter()
        .map(|ticket| {
            let ahead = ahead(state, ticket);
            let queued = ahead
                .iter()
                .map(|earlier| earlier.estimate_secs)
                .sum::<u64>();
            (ticket.id, ahead.len(), remaining(ticket) + queued)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        balance::Balance,
        process::ProcessIdentity,
        state::{AIR, Claim, Holder},
    };

    pub(crate) fn ticket(id: u64, estimate_secs: u64, claims: Vec<Claim>) -> Ticket {
        Ticket {
            id,
            owner: format!("owner-{id}"),
            work: format!("work-{id}"),
            estimate_secs,
            process: ProcessIdentity {
                pid: 1,
                start_ticks: 1,
            },
            enqueued_unix: id,
            claims,
            unknown: Default::default(),
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
            reason: None,
            unknown: Default::default(),
        }
    }

    fn balance(state: &mut State, owner: &str, minutes: i64) {
        state.balances.insert(
            owner.into(),
            Balance {
                balance_ms: minutes * 60_000,
                ..Balance::default()
            },
        );
    }

    #[test]
    fn disjoint_requests_proceed_while_conflicting_ones_wait() {
        let state = State {
            holders: vec![holder(ticket(1, 300, board("esp32s31")))],
            queue: vec![
                ticket(2, 60, board("esp32s31")),
                ticket(3, 60, board("esp32c5")),
                ticket(4, 60, vec![Claim::board("esp32c5"), Claim::exclusive(AIR)]),
            ],
            ..State::default()
        };
        assert!(!grantable(&state, 2), "esp32s31 is held");
        assert!(grantable(&state, 3), "esp32c5 is free");
        assert!(!grantable(&state, 4), "exclusive air waits for the holder");
        assert!(blocks_waiters(&state, 1));
    }

    #[test]
    fn the_owner_with_the_higher_balance_is_served_first() {
        let mut state = State {
            queue: vec![
                ticket(1, 600, board("esp32s31")),
                ticket(2, 60, board("esp32s31")),
            ],
            ..State::default()
        };
        // Equal balances keep arrival order.
        assert!(grantable(&state, 1));
        assert!(!grantable(&state, 2));
        balance(&mut state, "owner-2", 5);
        assert!(!grantable(&state, 1));
        assert!(grantable(&state, 2));
    }

    #[test]
    fn a_holder_is_outranked_only_after_its_slice_by_a_higher_balance() {
        let mut state = State {
            holders: vec![holder(ticket(1, 300, board("esp32s31")))],
            queue: vec![ticket(2, 60, board("esp32s31"))],
            ..State::default()
        };
        balance(&mut state, "owner-1", -3);
        balance(&mut state, "owner-2", 4);
        let slice = Duration::from_secs(600);
        assert_eq!(outranked_by(&state, 1, 1000 + 599, slice), None);
        assert_eq!(
            outranked_by(&state, 1, 1000 + 600, slice),
            Some((String::from("owner-2"), 4 * 60_000))
        );
        balance(&mut state, "owner-2", -5);
        assert_eq!(outranked_by(&state, 1, 1000 + 600, slice), None);
    }

    #[test]
    fn expected_starts_count_only_conflicting_work_served_earlier() {
        let state = State {
            holders: vec![holder(ticket(9, 300, board("esp32s31")))],
            queue: vec![
                ticket(1, 600, board("esp32s31")),
                ticket(2, 60, board("esp32c5")),
                ticket(3, 60, board("esp32s31")),
            ],
            ..State::default()
        };
        assert_eq!(
            expected_starts(&state, 1100),
            [(1, 0, 200), (2, 0, 0), (3, 1, 800)]
        );
    }
}
