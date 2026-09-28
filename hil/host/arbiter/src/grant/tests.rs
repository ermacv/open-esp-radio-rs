use std::{
    process::{Child, Command},
    time::Duration,
};

use super::*;
use crate::{AIR, BoardEventKind, balance::Balance, state::State};

fn request(owner: &str, work: &str) -> Request {
    Request {
        owner: owner.into(),
        work: work.into(),
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
        estimate_secs: 60,
        process,
        enqueued_unix: crate::unix_now(),
        claims: normalize(&claims),
        unknown: Default::default(),
    }
}

fn hold(arbiter: &Arbiter, ticket: Ticket) {
    arbiter
        .transaction(|state| {
            state.holders.push(Holder {
                ticket,
                token: "other".into(),
                granted_unix: crate::unix_now(),
                reason: None,
                preempted: None,
                unknown: Default::default(),
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

fn set_balance(arbiter: &Arbiter, owner: &str, minutes: i64) {
    arbiter
        .transaction(|state| {
            state.balances.insert(
                owner.into(),
                Balance {
                    balance_ms: minutes * 60_000,
                    last_active_unix_ms: crate::unix_now_ms(),
                    ..Balance::default()
                },
            );
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
fn a_waiter_with_a_higher_balance_is_served_before_an_earlier_one() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (head, head_identity) = other_process();
    queue(&arbiter, ticket(1, "head", head_identity, vec![]));
    set_balance(&arbiter, "head", 5);
    set_balance(&arbiter, "wifi", -5);
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            let grant = arbiter.acquire_within(&request("wifi", "smoke"), None);
            grant.map(drop).is_ok()
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        state(&arbiter).holders.is_empty(),
        "the head's balance is higher"
    );
    set_balance(&arbiter, "wifi", 10);
    assert!(waiter.join().unwrap(), "a higher balance is served first");
    let record = &arbiter.history().unwrap()[0];
    assert_eq!(record.owner, "wifi");
    let reason = record.reason.as_ref().unwrap();
    assert!(
        reason.over.iter().any(|over| over.owner == "head"),
        "{reason:?}"
    );
    stop(head);
}

#[test]
fn a_lease_is_charged_the_time_it_holds_and_budgets_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let grant = arbiter
        .acquire_within(&request("phy", "run"), None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    grant.mark_yielded();
    drop(grant);
    let record = &arbiter.history().unwrap()[0];
    assert_eq!(record.outcome, LeaseOutcome::YieldedToBalance);
    assert!(record.charged_ms >= 300, "{record:?}");
    let set = |name: &str| (name == "OER_HIL_SHORT").then(|| std::ffi::OsString::from("1"));
    assert_eq!(retired_variable(set), Some("OER_HIL_SHORT"));
    assert_eq!(retired_variable(|_| None), None);
    assert!(NO_BUDGETS.contains("no budget"));
}

#[test]
fn divisible_work_yields_after_its_slice_to_a_waiter_with_a_higher_balance() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let mut grant = arbiter
        .acquire_within(&on_board("esp32s31", request("phy", "series")), None)
        .unwrap();
    grant.supervise_with(true, Duration::from_secs(1), Duration::from_secs(60));
    let (child, identity) = other_process();
    queue(
        &arbiter,
        ticket(
            50,
            "wifi",
            identity,
            vec![Claim::board("esp32c5"), Claim::shared(AIR)],
        ),
    );
    set_balance(&arbiter, "wifi", 10);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!grant.yield_requested(), "a waiter on another board");
    arbiter
        .transaction(|state| {
            state.queue[0].claims = normalize(&[Claim::board("esp32s31")]);
            Ok(())
        })
        .unwrap();
    set_balance(&arbiter, "phy", 20);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!grant.yield_requested(), "the waiter's balance is lower");
    set_balance(&arbiter, "phy", -20);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(grant.yield_requested());
    grant.mark_yielded();
    drop(grant);
    stop(child);
}

#[test]
fn indivisible_work_runs_on_while_a_waiter_has_a_higher_balance() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let mut grant = arbiter
        .acquire_within(&on_board("esp32s31", request("phy", "flash")), None)
        .unwrap();
    grant.supervise_with(false, Duration::ZERO, Duration::from_secs(60));
    let (child, identity) = other_process();
    queue(
        &arbiter,
        ticket(51, "wifi", identity, vec![Claim::board("esp32s31")]),
    );
    set_balance(&arbiter, "wifi", 30);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!grant.yield_requested());
    assert_eq!(state(&arbiter).holders.len(), 1);
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
    std::fs::write(directory.path().join("state.json"), r#"{"schema":4}"#).unwrap();
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
        .acquire_within(&on_board("esp32s31", request("phy", "hung")), None)
        .unwrap();
    grant.supervise_with(false, MIN_SLICE, Duration::from_secs(1));
    // The ordinary cancellation path releases the lease.
    while oer_process::sleep(Duration::from_secs(60)).is_ok() {}
}

#[test]
fn every_lease_is_terminated_at_the_hard_limit() {
    if let Some(directory) = std::env::var_os(WATCHDOG_CHILD) {
        return hold_supervised_lease(directory);
    }
    let directory = tempfile::tempdir().unwrap();
    let (elapsed, outcome) = supervised_child(
        "grant::tests::every_lease_is_terminated_at_the_hard_limit",
        directory.path(),
        |_| {},
    );
    assert!(elapsed >= Duration::from_millis(900), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    assert_eq!(outcome, LeaseOutcome::HardLimit);
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
