use std::{
    process::{Child, Command},
    time::Duration,
};

use super::*;
use crate::{BoardEventKind, budget::BudgetSource, state::State};

fn request(owner: &str, work: &str) -> Request {
    Request {
        owner: owner.into(),
        work: work.into(),
        budget: Some(Duration::from_secs(60)),
        short: false,
    }
}

/// A live process that stands for another requester.
fn other_process() -> (Child, ProcessIdentity) {
    let child = Command::new("sleep").arg("60").spawn().unwrap();
    let identity = ProcessIdentity::of(child.id()).unwrap();
    (child, identity)
}

fn ticket(id: u64, owner: &str, process: ProcessIdentity, short: bool) -> Ticket {
    Ticket {
        id,
        owner: owner.into(),
        work: format!("work {id}"),
        budget_secs: 60,
        budget_source: BudgetSource::Explicit,
        short,
        process,
        enqueued_unix: crate::unix_now(),
    }
}

fn hold(arbiter: &Arbiter, ticket: Ticket) {
    arbiter
        .transaction(|state| {
            state.holder = Some(Holder {
                ticket,
                token: "other".into(),
                granted_unix: crate::unix_now(),
                over_budget: false,
            });
            Ok(())
        })
        .unwrap();
}

fn state(arbiter: &Arbiter) -> State {
    arbiter.transaction(|state| Ok(state.clone())).unwrap()
}

#[test]
fn a_free_stand_is_granted_and_released_into_history() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let grant = arbiter
        .acquire_within(&request("wifi", "run a"), None)
        .unwrap();
    assert!(!grant.is_nested());
    let token = grant.environment()[0].1.clone();
    assert_eq!(state(&arbiter).holder.unwrap().token, token);
    // The holding process and commands carrying its token join the lease.
    assert!(
        arbiter
            .acquire_within(&request("wifi", "inner"), None)
            .unwrap()
            .is_nested()
    );
    assert!(
        arbiter
            .acquire_within(&request("wifi", "inner"), Some(token.clone()))
            .unwrap()
            .is_nested()
    );
    drop(grant);
    assert!(state(&arbiter).holder.is_none());
    let history = arbiter.history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, LeaseOutcome::Released);
    assert_eq!(history[0].work, "run a");
    // A token of a finished lease is refused rather than silently queued.
    assert!(
        arbiter
            .acquire_within(&request("wifi", "late"), Some(token))
            .is_err()
    );
}

#[test]
fn a_waiter_is_granted_when_the_holder_dies() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (mut holder, identity) = other_process();
    hold(&arbiter, ticket(7, "phy", identity, false));
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            let grant = arbiter.acquire_within(&request("bt", "run b"), None);
            grant.map(|grant| grant.is_nested())
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(state(&arbiter).queue.len(), 1, "the request waits");
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(!waiter.join().unwrap().unwrap());
    let history = arbiter.history().unwrap();
    assert_eq!(history[0].id, 7);
    assert_eq!(history[0].outcome, LeaseOutcome::Abandoned);
    assert_eq!(history[1].owner, "bt");
}

#[test]
fn waiting_follows_fifo_and_dead_waiters_are_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (mut head, head_identity) = other_process();
    let (mut dead, dead_identity) = other_process();
    arbiter
        .transaction(|state| {
            state.queue.push(ticket(1, "head", head_identity, false));
            state.queue.push(ticket(2, "dead", dead_identity, false));
            state.next_id = 3;
            Ok(())
        })
        .unwrap();
    dead.kill().unwrap();
    dead.wait().unwrap();
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || arbiter.acquire_within(&request("bt", "run"), None).is_ok())
    };
    std::thread::sleep(Duration::from_millis(700));
    let waiting = state(&arbiter);
    assert_eq!(
        waiting.queue.iter().map(|t| t.id).collect::<Vec<_>>(),
        [1, 3],
        "the free stand stays reserved for the live head"
    );
    assert!(waiting.holder.is_none());
    head.kill().unwrap();
    head.wait().unwrap();
    assert!(waiter.join().unwrap());
}

#[test]
fn a_short_request_is_granted_ahead_of_the_head_and_must_be_short() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (mut head, head_identity) = other_process();
    arbiter
        .transaction(|state| {
            state.queue.push(ticket(1, "head", head_identity, false));
            state.next_id = 2;
            Ok(())
        })
        .unwrap();
    let short = Request {
        short: true,
        ..request("wifi", "smoke")
    };
    let grant = arbiter.acquire_within(&short, None).unwrap();
    assert_eq!(state(&arbiter).queue.len(), 1);
    assert!(state(&arbiter).head_next);
    drop(grant);
    let long = Request {
        budget: Some(Duration::from_secs(600)),
        ..short
    };
    assert!(arbiter.acquire_within(&long, None).is_err());
    head.kill().unwrap();
    head.wait().unwrap();
}

#[test]
fn over_budget_and_termination_are_recorded() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let grant = arbiter
        .acquire_within(&request("phy", "run"), None)
        .unwrap();
    grant.warn_over_budget();
    assert!(state(&arbiter).holder.unwrap().over_budget);
    grant.mark_budget_exceeded();
    drop(grant);
    assert_eq!(
        arbiter.history().unwrap()[0].outcome,
        LeaseOutcome::BudgetExceeded
    );
}

#[test]
fn status_and_board_report_the_latest_state() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    arbiter
        .record_board(
            None,
            BoardEventKind::StartupArtifactWritten {
                path: "/tmp/a".into(),
                sha256: "ab".into(),
                disposition: "Created".into(),
            },
        )
        .unwrap();
    let (mut holder, identity) = other_process();
    hold(&arbiter, ticket(4, "802154", identity, false));
    let status = arbiter.status().unwrap();
    assert_eq!(status.holder.as_ref().unwrap().owner, "802154");
    assert!(status.startup_artifact.is_some());
    assert!(
        status
            .devices
            .iter()
            .all(|device| device.firmware.is_none())
    );
    let text = status.to_string();
    assert!(text.contains("802154"), "{text}");
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(arbiter.status().unwrap().holder.is_none());
}

#[test]
fn a_newer_state_schema_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("state.json"), r#"{"schema":2}"#).unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    assert!(arbiter.status().is_err());
}

/// Set in the child copy of the test binary that holds a supervised lease.
const WATCHDOG_CHILD: &str = "OER_HIL_ARBITER_TEST_WATCHDOG_CHILD";

#[test]
fn the_watchdog_cancels_its_process_at_twice_the_budget() {
    if let Some(directory) = std::env::var_os(WATCHDOG_CHILD) {
        let _signals = oer_process::install_signal_handlers().unwrap();
        let arbiter = Arbiter::at(std::path::PathBuf::from(directory)).unwrap();
        let mut grant = arbiter
            .acquire_within(
                &Request {
                    budget: Some(Duration::from_secs(1)),
                    ..request("phy", "hung")
                },
                None,
            )
            .unwrap();
        grant.terminate_self_on_overrun();
        // The ordinary cancellation path releases the lease.
        while oer_process::sleep(Duration::from_secs(60)).is_ok() {}
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "grant::tests::the_watchdog_cancels_its_process_at_twice_the_budget",
            "--test-threads=1",
        ])
        .env(WATCHDOG_CHILD, directory.path())
        .status()
        .unwrap();
    assert!(status.success());
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let history = arbiter.history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, LeaseOutcome::BudgetExceeded);
}
