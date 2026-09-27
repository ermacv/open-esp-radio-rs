use std::{
    process::{Child, Command},
    time::Duration,
};

use super::*;
use crate::{AIR, BoardEventKind, budget::BudgetSource, state::State};

fn request(owner: &str, work: &str) -> Request {
    Request {
        owner: owner.into(),
        work: work.into(),
        budget: Some(Duration::from_secs(60)),
        short: false,
        scenarios: Vec::new(),
        claims: Vec::new(),
    }
}

fn on_board(board: &str, request: Request) -> Request {
    Request {
        claims: vec![Claim::board(board), Claim::shared(AIR)],
        ..request
    }
}

/// A live process that stands for another requester.
fn other_process() -> (Child, ProcessIdentity) {
    let child = Command::new("sleep").arg("60").spawn().unwrap();
    let identity = ProcessIdentity::of(child.id()).unwrap();
    (child, identity)
}

fn stop(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

fn ticket(id: u64, owner: &str, process: ProcessIdentity, claims: Vec<Claim>) -> Ticket {
    Ticket {
        id,
        owner: owner.into(),
        work: format!("work {id}"),
        budget_secs: 60,
        budget_source: BudgetSource::Explicit,
        short: false,
        process,
        enqueued_unix: crate::unix_now(),
        claims: normalize(&claims),
    }
}

fn hold(arbiter: &Arbiter, ticket: Ticket) {
    arbiter
        .transaction(|state| {
            state.holders.push(Holder {
                ticket,
                token: "other".into(),
                granted_unix: crate::unix_now(),
                over_budget: false,
            });
            Ok(())
        })
        .unwrap();
}

fn queue(arbiter: &Arbiter, ticket: Ticket) {
    arbiter
        .transaction(|state| {
            state.next_id = state.next_id.max(ticket.id + 1);
            state.queue.push(ticket);
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
    assert_eq!(state(&arbiter).holders[0].token, token);
    assert_eq!(state(&arbiter).holders[0].ticket.claims, [Claim::stand()]);
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
    assert!(state(&arbiter).holders.is_empty());
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
fn a_nested_command_may_use_only_what_the_enclosing_lease_holds() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let grant = arbiter
        .acquire_within(&on_board("esp32s31", request("phy", "series")), None)
        .unwrap();
    let token = grant.environment()[0].1.clone();
    let inside = on_board("esp32s31", request("phy", "run a"));
    assert!(
        arbiter
            .acquire_within(&inside, Some(token.clone()))
            .unwrap()
            .is_nested()
    );
    let other_board = on_board("esp32c5", request("phy", "run b"));
    let error = arbiter
        .acquire_within(&other_board, Some(token))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("board:esp32c5"), "{error}");
}

#[test]
fn leases_on_different_boards_are_held_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (child, identity) = other_process();
    hold(
        &arbiter,
        ticket(
            7,
            "wifi",
            identity,
            vec![Claim::board("esp32s31"), Claim::shared(AIR)],
        ),
    );
    let grant = arbiter
        .acquire_within(&on_board("esp32c5", request("802154", "peer")), None)
        .unwrap();
    assert_eq!(state(&arbiter).holders.len(), 2);
    drop(grant);
    // Exclusive air waits for every radio user.
    let rf = Request {
        claims: vec![Claim::board("esp32c5"), Claim::exclusive(AIR)],
        ..request("phy", "rf")
    };
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            arbiter
                .acquire_within(&rf, None)
                .map(|grant| grant.is_nested())
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(state(&arbiter).queue.len(), 1);
    stop(child);
    assert!(!waiter.join().unwrap().unwrap());
}

#[test]
fn a_waiter_is_granted_when_the_holder_dies() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (holder, identity) = other_process();
    hold(&arbiter, ticket(7, "phy", identity, vec![]));
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            let grant = arbiter.acquire_within(&request("bt", "run b"), None);
            grant.map(|grant| grant.is_nested())
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(state(&arbiter).queue.len(), 1, "the request waits");
    stop(holder);
    assert!(!waiter.join().unwrap().unwrap());
    let history = arbiter.history().unwrap();
    assert_eq!(history[0].id, 7);
    assert_eq!(history[0].outcome, LeaseOutcome::Abandoned);
    assert_eq!(history[1].owner, "bt");
}

#[test]
fn waiting_follows_arrival_order_and_dead_waiters_are_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (head, head_identity) = other_process();
    let (dead, dead_identity) = other_process();
    queue(&arbiter, ticket(1, "head", head_identity, vec![]));
    queue(&arbiter, ticket(2, "dead", dead_identity, vec![]));
    stop(dead);
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
    assert!(waiting.holders.is_empty());
    stop(head);
    assert!(waiter.join().unwrap());
}

#[test]
fn a_short_request_is_granted_ahead_of_the_head_and_must_be_short() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (head, head_identity) = other_process();
    queue(&arbiter, ticket(1, "head", head_identity, vec![]));
    let short = Request {
        short: true,
        ..request("wifi", "smoke")
    };
    let grant = arbiter.acquire_within(&short, None).unwrap();
    assert_eq!(state(&arbiter).queue.len(), 1);
    assert!(state(&arbiter).jumped);
    drop(grant);
    let long = Request {
        budget: Some(Duration::from_secs(600)),
        ..short
    };
    assert!(arbiter.acquire_within(&long, None).is_err());
    stop(head);
}

#[test]
fn over_budget_and_termination_are_recorded() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let grant = arbiter
        .acquire_within(&request("phy", "run"), None)
        .unwrap();
    grant.warn_over_budget();
    assert!(state(&arbiter).holders[0].over_budget);
    grant.mark_budget_exceeded();
    drop(grant);
    let yielded = arbiter
        .acquire_within(&request("phy", "run"), None)
        .unwrap();
    yielded.mark_yielded();
    drop(yielded);
    let outcomes = arbiter
        .history()
        .unwrap()
        .iter()
        .map(|record| record.outcome)
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes,
        [LeaseOutcome::BudgetExceeded, LeaseOutcome::Yielded]
    );
}

#[test]
fn divisible_work_is_asked_to_yield_only_while_it_blocks_a_waiter() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    // The supervisor terminates this test process at twice the budget, so
    // every assertion completes within four seconds of the grant.
    let mut grant = arbiter
        .acquire_within(
            &Request {
                budget: Some(Duration::from_secs(2)),
                ..on_board("esp32s31", request("phy", "series"))
            },
            None,
        )
        .unwrap();
    grant.supervise_self(true);
    let granted = Instant::now();
    let (child, identity) = other_process();
    queue(
        &arbiter,
        ticket(
            50,
            "esp32c5",
            identity,
            vec![Claim::board("esp32c5"), Claim::shared(AIR)],
        ),
    );
    std::thread::sleep(Duration::from_millis(2300).saturating_sub(granted.elapsed()));
    assert!(!grant.yield_requested(), "a waiter on another board");
    assert!(!grant.blocks_waiters());
    arbiter
        .transaction(|state| {
            state.queue[0].claims = normalize(&[Claim::board("esp32s31")]);
            Ok(())
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(3300).saturating_sub(granted.elapsed()));
    assert!(grant.blocks_waiters());
    assert!(grant.yield_requested());
    grant.mark_yielded();
    drop(grant);
    assert!(granted.elapsed() < Duration::from_secs(4));
    stop(child);
}

#[test]
fn divisible_work_within_its_budget_yields_only_to_brief_waiters() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let mut grant = arbiter
        .acquire_within(
            &Request {
                budget: Some(Duration::from_secs(600)),
                ..on_board("esp32s31", request("phy", "long series"))
            },
            None,
        )
        .unwrap();
    grant.supervise_self(true);
    let (child, identity) = other_process();
    // A long request on the same board waits for the series' budget.
    queue(
        &arbiter,
        Ticket {
            budget_secs: 1800,
            ..ticket(60, "wifi", identity, vec![Claim::board("esp32s31")])
        },
    );
    std::thread::sleep(Duration::from_secs(6));
    assert!(grant.blocks_waiters());
    assert!(
        !grant.yield_requested(),
        "a long waiter waits for the budget"
    );
    // A brief one is let in at the next boundary.
    arbiter
        .transaction(|state| {
            state.queue[0].budget_secs = 180;
            Ok(())
        })
        .unwrap();
    std::thread::sleep(Duration::from_secs(6));
    assert!(grant.yield_requested());
    grant.mark_yielded();
    drop(grant);
    stop(child);
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
    let (holder, identity) = other_process();
    hold(
        &arbiter,
        ticket(4, "802154", identity, vec![Claim::board("esp32c5")]),
    );
    let status = arbiter.status().unwrap();
    assert_eq!(status.holders[0].owner, "802154");
    assert_eq!(status.holders[0].claims, "board:esp32c5");
    assert_eq!(status.startup_artifacts.len(), 1);
    assert!(
        status
            .devices
            .iter()
            .all(|device| device.firmware.is_none())
    );
    let text = status.to_string();
    assert!(text.contains("802154"), "{text}");
    stop(holder);
    assert!(arbiter.status().unwrap().holders.is_empty());
}

#[test]
fn a_newer_state_schema_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("state.json"), r#"{"schema":3}"#).unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    assert!(arbiter.status().is_err());
}

/// Set in the child copy of the test binary that holds a supervised lease.
const WATCHDOG_CHILD: &str = "OER_HIL_ARBITER_TEST_WATCHDOG_CHILD";

/// Run `test` in a child copy of this binary holding a one-second
/// indivisible lease, run `granted` once it holds the lease, and return the
/// elapsed time since the grant and the lease's outcome.
fn supervised_child(
    test: &str,
    directory: &std::path::Path,
    granted: impl FnOnce(&Arbiter),
) -> (Duration, LeaseOutcome) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--test-threads=1"])
        .env(WATCHDOG_CHILD, directory)
        .spawn()
        .unwrap();
    let arbiter = Arbiter::at(directory).unwrap();
    while state(&arbiter).holders.is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    granted(&arbiter);
    assert!(child.wait().unwrap().success());
    let history = arbiter.history().unwrap();
    let lease = history
        .iter()
        .find(|record| record.owner == "phy")
        .expect("the child's lease");
    (started.elapsed(), lease.outcome)
}

fn hold_supervised_lease(directory: std::ffi::OsString) {
    let _signals = oer_process::install_signal_handlers().unwrap();
    let arbiter = Arbiter::at(std::path::PathBuf::from(directory)).unwrap();
    let mut grant = arbiter
        .acquire_within(
            &Request {
                budget: Some(Duration::from_secs(1)),
                ..on_board("esp32s31", request("phy", "hung"))
            },
            None,
        )
        .unwrap();
    grant.supervise_self(false);
    // The ordinary cancellation path releases the lease.
    while oer_process::sleep(Duration::from_secs(60)).is_ok() {}
}

#[test]
fn the_watchdog_cancels_its_process_at_twice_the_budget() {
    if let Some(directory) = std::env::var_os(WATCHDOG_CHILD) {
        return hold_supervised_lease(directory);
    }
    let directory = tempfile::tempdir().unwrap();
    let (elapsed, outcome) = supervised_child(
        "grant::tests::the_watchdog_cancels_its_process_at_twice_the_budget",
        directory.path(),
        |_| {},
    );
    assert!(elapsed >= Duration::from_millis(1900), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    assert_eq!(outcome, LeaseOutcome::BudgetExceeded);
}

#[test]
fn indivisible_work_is_preempted_at_its_budget_when_others_wait() {
    if let Some(directory) = std::env::var_os(WATCHDOG_CHILD) {
        return hold_supervised_lease(directory);
    }
    let directory = tempfile::tempdir().unwrap();
    let (waiter, identity) = other_process();
    let (elapsed, outcome) = supervised_child(
        "grant::tests::indivisible_work_is_preempted_at_its_budget_when_others_wait",
        directory.path(),
        |arbiter| {
            queue(
                arbiter,
                ticket(1000, "wifi", identity, vec![Claim::board("esp32s31")]),
            )
        },
    );
    assert!(elapsed >= Duration::from_millis(900), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(outcome, LeaseOutcome::Preempted);
    stop(waiter);
}

#[test]
fn a_new_request_waits_for_live_leases_of_the_previous_schema() {
    let directory = tempfile::tempdir().unwrap();
    let (legacy, identity) = other_process();
    let legacy_ticket = serde_json::json!({
        "id": 1, "owner": "wifi", "work": "run", "budget_secs": 60,
        "budget_source": {"kind": "explicit"}, "short": false,
        "process": identity, "enqueued_unix": 1
    });
    std::fs::write(
        directory.path().join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "next_id": 2, "queue": [],
            "holder": {"ticket": legacy_ticket, "token": "t", "granted_unix": 1, "over_budget": false},
            "head_next": false
        }))
        .unwrap(),
    )
    .unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            arbiter
                .acquire_within(&on_board("esp32c5", request("802154", "peer")), None)
                .is_ok()
        })
    };
    std::thread::sleep(Duration::from_millis(1500));
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("state.json")).unwrap())
            .unwrap();
    assert_eq!(raw["schema"], 1, "the old lease keeps its state");
    stop(legacy);
    assert!(waiter.join().unwrap());
    assert_eq!(state(&arbiter).schema, crate::state::STATE_SCHEMA);
}
