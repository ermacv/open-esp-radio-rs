//! Owner balances: who has waited more than they used.
//!
//! Every owner has a balance. Each lease it holds charges it the time held;
//! while it waits for a resource another lease holds, it is credited the time
//! waited, once however many requests it has queued. An owner with neither
//! accrues nothing. Balances decay with a two-hour half-life and are clamped
//! to an hour either way, so old debt fades. After each grant the mean over
//! the owners active in the last day is subtracted, keeping the sum near zero.
//! The grantable request whose owner has the highest balance is served
//! first, the earlier one on a tie; divisible work of an owner with a lower
//! balance than a waiter it blocks yields after a minimum slice.

use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};

use crate::state::{State, conflict};

/// Balances halve over this time.
pub const HALF_LIFE: Duration = Duration::from_secs(2 * 3600);
/// Balances stay within this much either way.
pub const CAP: Duration = Duration::from_secs(3600);
/// Owners active within this window share the normalization.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(24 * 3600);
/// Divisible work runs at least this long before it yields to a waiter.
pub const MIN_SLICE: Duration = Duration::from_secs(10 * 60);
/// No lease runs longer than this; its holder is then terminated.
pub const HARD_LIMIT: Duration = Duration::from_secs(3600);

/// One owner's standing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Balance {
    /// Positive: waited more than used.
    pub balance_ms: i64,
    pub last_active_unix_ms: u64,
    #[serde(flatten)]
    pub unknown: crate::Unknown,
}

/// Advance every balance from the state's last settlement to `now_ms`:
/// decay, then credit blocked waiters and charge holders, then clamp.
pub(crate) fn settle(state: &mut State, now_ms: u64) {
    let elapsed = now_ms.saturating_sub(state.settled_unix_ms);
    if state.settled_unix_ms == 0 || elapsed == 0 {
        state.settled_unix_ms = state.settled_unix_ms.max(now_ms);
        return;
    }
    let decay = 0.5_f64.powf(elapsed as f64 / HALF_LIFE.as_millis() as f64);
    for balance in state.balances.values_mut() {
        balance.balance_ms = (balance.balance_ms as f64 * decay).round() as i64;
    }
    let elapsed = elapsed as i64;
    let mut waiting = std::collections::BTreeSet::new();
    for ticket in &state.queue {
        if state
            .holders
            .iter()
            .any(|holder| conflict(&holder.ticket.claims, &ticket.claims))
        {
            waiting.insert(ticket.owner.clone());
        }
    }
    for owner in waiting {
        let balance = state.balances.entry(owner).or_default();
        balance.balance_ms += elapsed;
        balance.last_active_unix_ms = now_ms;
    }
    // A preempted lease is charged only up to its preemption.
    for holder in state
        .holders
        .iter()
        .filter(|holder| holder.preempted.is_none())
    {
        let balance = state
            .balances
            .entry(holder.ticket.owner.clone())
            .or_default();
        balance.balance_ms -= elapsed;
        balance.last_active_unix_ms = now_ms;
    }
    let cap = CAP.as_millis() as i64;
    for balance in state.balances.values_mut() {
        balance.balance_ms = balance.balance_ms.clamp(-cap, cap);
    }
    state.settled_unix_ms = now_ms;
}

/// Subtract the mean balance of the owners active within the window, and
/// forget the others.
pub(crate) fn normalize(state: &mut State, now_ms: u64) {
    let window = ACTIVE_WINDOW.as_millis() as u64;
    state
        .balances
        .retain(|_, balance| now_ms.saturating_sub(balance.last_active_unix_ms) <= window);
    if state.balances.is_empty() {
        return;
    }
    let mean = state
        .balances
        .values()
        .map(|balance| balance.balance_ms)
        .sum::<i64>()
        / state.balances.len() as i64;
    for balance in state.balances.values_mut() {
        balance.balance_ms -= mean;
    }
}

/// The balance of `owner`; zero for an owner without one.
pub(crate) fn of(balances: &BTreeMap<String, Balance>, owner: &str) -> i64 {
    balances.get(owner).map_or(0, |balance| balance.balance_ms)
}

/// Whether ticket `a` is served before ticket `b`: the higher balance, then
/// the earlier arrival.
pub(crate) fn before(state: &State, a: &crate::state::Ticket, b: &crate::state::Ticket) -> bool {
    let (balance_a, balance_b) = (of(&state.balances, &a.owner), of(&state.balances, &b.owner));
    balance_a > balance_b || (balance_a == balance_b && a.id < b.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process::ProcessIdentity,
        state::{Claim, Holder, Ticket},
    };

    const MINUTE: u64 = 60_000;

    fn ticket(id: u64, owner: &str) -> Ticket {
        Ticket {
            id,
            owner: owner.into(),
            work: String::from("work"),
            estimate_secs: 60,
            process: ProcessIdentity::current().unwrap(),
            enqueued_unix: 0,
            claims: vec![Claim::board("AA")],
            unknown: Default::default(),
        }
    }

    fn holding(owner: &str) -> Holder {
        Holder {
            ticket: ticket(1, owner),
            token: String::from("t"),
            granted_unix: 0,
            reason: None,
            preempted: None,
            unknown: Default::default(),
        }
    }

    fn state_at(now: u64) -> State {
        State {
            settled_unix_ms: now,
            ..State::default()
        }
    }

    #[test]
    fn holding_twenty_minutes_while_another_waits_puts_the_waiter_next() {
        let mut state = state_at(1);
        state.holders.push(holding("a"));
        state.queue.push(ticket(2, "b"));
        settle(&mut state, 1 + 20 * MINUTE);
        let (a, b) = (of(&state.balances, "a"), of(&state.balances, "b"));
        // Twenty minutes decay by 2^(-1/6) at most, applied to the prior zero.
        assert_eq!((a, b), (-20 * MINUTE as i64, 20 * MINUTE as i64));
        state.holders.clear();
        state.queue.push(ticket(3, "a"));
        assert!(
            before(&state, &state.queue[0], &state.queue[1]),
            "b before a"
        );
        normalize(&mut state, 1 + 20 * MINUTE);
        let sum: i64 = state.balances.values().map(|b| b.balance_ms).sum();
        assert!(sum.abs() <= 1, "normalization keeps the sum at zero: {sum}");
    }

    #[test]
    fn balances_decay_with_a_two_hour_half_life_and_are_clamped() {
        let mut state = state_at(1);
        state.balances.insert(
            String::from("a"),
            Balance {
                balance_ms: 40 * MINUTE as i64,
                last_active_unix_ms: 1,
                ..Balance::default()
            },
        );
        settle(&mut state, 1 + HALF_LIFE.as_millis() as u64);
        assert_eq!(of(&state.balances, "a"), 20 * MINUTE as i64);
        state.holders.push(holding("a"));
        settle(&mut state, 1 + HALF_LIFE.as_millis() as u64 + 200 * MINUTE);
        assert_eq!(of(&state.balances, "a"), -(CAP.as_millis() as i64));
    }

    #[test]
    fn an_owner_neither_waiting_nor_holding_accrues_nothing() {
        let mut state = state_at(1);
        state.holders.push(holding("a"));
        // A queued request on a disjoint board is not blocked.
        let mut free = ticket(2, "c");
        free.claims = vec![Claim::board("BB")];
        state.queue.push(free);
        settle(&mut state, 1 + 10 * MINUTE);
        assert_eq!(of(&state.balances, "c"), 0);
        assert!(!state.balances.contains_key("paused"));
    }

    #[test]
    fn owners_idle_for_a_day_leave_the_normalization() {
        let mut state = state_at(1);
        for (owner, active) in [("old", 1), ("new", ACTIVE_WINDOW.as_millis() as u64 + 10)] {
            state.balances.insert(
                owner.into(),
                Balance {
                    balance_ms: 5 * MINUTE as i64,
                    last_active_unix_ms: active,
                    ..Balance::default()
                },
            );
        }
        normalize(&mut state, ACTIVE_WINDOW.as_millis() as u64 + 20);
        assert!(!state.balances.contains_key("old"));
        assert_eq!(of(&state.balances, "new"), 0);
    }
}
