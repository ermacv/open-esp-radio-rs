use core::future::{pending, ready};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

struct ReadyTaskGroup {
    stop_requests: usize,
    owner: Option<u32>,
}

#[test]
fn auxiliary_group_reveals_primary_owner_only_after_both_tasks_stop() {
    let primary = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(17),
    };
    let auxiliary = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(29),
    };
    let mut group = ConnectedTaskGroupWithAuxiliary::new(primary, auxiliary);

    group.request_stop();
    assert_eq!(block_on(group.wait_stopped()), 17);
    assert_eq!(group.primary.stop_requests, 1);
    assert_eq!(group.auxiliary.stop_requests, 1);
}

impl ConnectedTaskGroup for ReadyTaskGroup {
    type Stopped = u32;

    fn request_stop(&mut self) {
        self.stop_requests += 1;
    }

    async fn wait_stopped(&mut self) -> Self::Stopped {
        ready(self.owner.take().expect("task owner is returned once")).await
    }
}

#[test]
fn one_deadline_returns_the_exact_task_owner() {
    let mut group = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(73),
    };
    let outcome = block_on(stop_connected_task_group_until(
        &mut group,
        Duration::from_millis(1),
    ));
    let ConnectedTaskStopAttempt::Stopped(owner) = outcome else {
        panic!("ready task group unexpectedly remained pending");
    };
    assert_eq!(owner, 73);
    assert_eq!(group.stop_requests, 1);
}

#[test]
fn normal_stop_waits_for_and_returns_the_exact_task_owner() {
    let mut group = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(79),
    };

    assert_eq!(block_on(stop_connected_task_group(&mut group)), 79);
    assert_eq!(group.stop_requests, 1);
}

struct StuckTaskGroup {
    stop_requested: bool,
}

impl ConnectedTaskGroup for StuckTaskGroup {
    type Stopped = ();

    fn request_stop(&mut self) {
        self.stop_requested = true;
    }

    async fn wait_stopped(&mut self) -> Self::Stopped {
        pending().await
    }
}

#[test]
fn missed_group_deadline_remains_a_pending_stop() {
    let mut group = StuckTaskGroup {
        stop_requested: false,
    };
    let outcome = block_on(stop_connected_task_group_until(
        &mut group,
        Duration::from_ticks(0),
    ));
    assert!(group.stop_requested);
    assert!(outcome.is_pending());
}

#[test]
fn split_capabilities_round_trip_and_allow_a_later_epoch() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (mut controller, endpoint) = resources.split().expect("first epoch is idle");
    controller.request_stop();
    block_on(endpoint.wait_for_stop());
    endpoint.complete(91);
    assert_eq!(block_on(controller.wait_stopped()), 91);
    assert!(resources.split().is_ok());
}

#[test]
fn unused_reservation_rolls_back_without_poisoning_the_next_epoch() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    resources
        .reserve()
        .expect("first epoch is idle")
        .abort_unused();

    assert!(resources.reserve().is_ok());
}

#[test]
fn dropped_reservation_preserves_the_fail_closed_contract() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    drop(resources.reserve().expect("first epoch is idle"));

    assert!(matches!(
        resources.reserve(),
        Err(ConnectedTaskControlError::Poisoned)
    ));
}

#[test]
fn dropped_task_endpoint_permanently_poisons_reuse() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (_controller, endpoint) = resources.split().expect("first epoch is idle");
    drop(endpoint);
    assert!(resources.is_poisoned());
    assert!(matches!(
        resources.split(),
        Err(ConnectedTaskControlError::Poisoned)
    ));
}

#[test]
fn dropped_parent_controller_permanently_poisons_reuse() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (controller, endpoint) = resources.split().expect("first epoch is idle");
    drop(controller);
    assert!(resources.is_poisoned());
    drop(endpoint);
    assert!(matches!(
        resources.split(),
        Err(ConnectedTaskControlError::Poisoned)
    ));
}
