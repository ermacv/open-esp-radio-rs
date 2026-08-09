//! Chip-independent finite shutdown for tasks attached to a Wi-Fi epoch.
//!
//! A radio runner can return while an executor task still owns protocol
//! scratch or queue leases. This module provides a split ownership contract:
//! the parent receives only the stop/wait capability, while the task receives
//! only the request/return capability. A dropped task endpoint poisons the
//! resources, so a replacement epoch cannot erase an unreturned owner.

use core::{
    future::Future,
    sync::atomic::{AtomicU8, Ordering},
};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};
use embassy_time::{Duration, with_timeout};

const IDLE: u8 = 0;
const ACTIVE: u8 = 1;
const POISONED: u8 = 2;

/// Executor tasks which borrow resources from one connected Wi-Fi epoch.
pub trait ConnectedTaskGroup {
    type Stopped;

    /// Publish the idempotent cooperative stop request to every task.
    fn request_stop(&mut self);

    /// Wait until every task has released its epoch resources.
    #[allow(async_fn_in_trait)]
    async fn wait_stopped(&mut self) -> Self::Stopped;
}

/// One owner-bearing connected task plus an auxiliary task which must stop
/// before the epoch can release its radio resources.
///
/// The primary task returns the owner needed by the next driver epoch. The
/// auxiliary task may return bookkeeping of its own, but that value is kept
/// private: completion of both endpoints is required before the primary owner
/// is revealed. This is useful for application/HIL services which share an
/// Embassy network epoch without giving those services access to DMA or PAC
/// ownership.
pub struct ConnectedTaskGroupWithAuxiliary<P, A> {
    primary: P,
    auxiliary: A,
}

impl<P, A> ConnectedTaskGroupWithAuxiliary<P, A> {
    pub const fn new(primary: P, auxiliary: A) -> Self {
        Self { primary, auxiliary }
    }
}

impl<P, A> ConnectedTaskGroup for ConnectedTaskGroupWithAuxiliary<P, A>
where
    P: ConnectedTaskGroup,
    A: ConnectedTaskGroup,
{
    type Stopped = P::Stopped;

    fn request_stop(&mut self) {
        self.primary.request_stop();
        self.auxiliary.request_stop();
    }

    async fn wait_stopped(&mut self) -> Self::Stopped {
        let primary = self.primary.wait_stopped().await;
        let _auxiliary = self.auxiliary.wait_stopped().await;
        primary
    }
}

/// Why task-control resources cannot create another epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedTaskControlError {
    Active,
    Poisoned,
}

/// Reusable signal storage for one sequential executor task.
///
/// Only a completed task endpoint followed by consumption of its returned
/// owner restores `Idle`. There is deliberately no safe reset operation.
pub struct ConnectedTaskControlResources<M: RawMutex, S> {
    stop: Signal<M, ()>,
    stopped: Signal<M, S>,
    state: AtomicU8,
}

pub type ConnectedTaskEndpoints<'resources, M, S> = (
    ConnectedTaskController<'resources, M, S>,
    ConnectedTaskEndpoint<'resources, M, S>,
);

/// A task-control epoch reserved before any driver owner is decomposed.
///
/// Keeping both endpoints together permits an infallible rollback while no
/// executor task has observed either capability. Once `into_endpoints` is
/// called, ordinary controller/endpoint shutdown rules apply.
pub struct ConnectedTaskReservation<'resources, M: RawMutex, S> {
    controller: ConnectedTaskController<'resources, M, S>,
    endpoint: ConnectedTaskEndpoint<'resources, M, S>,
}

impl<'resources, M: RawMutex, S> ConnectedTaskReservation<'resources, M, S> {
    pub fn into_endpoints(self) -> ConnectedTaskEndpoints<'resources, M, S> {
        (self.controller, self.endpoint)
    }

    /// Roll back a reservation which was never exposed to an executor task.
    pub fn abort_unused(mut self) {
        self.controller.completed = true;
        self.endpoint.completed = true;
        self.controller
            .resources
            .state
            .store(IDLE, Ordering::Release);
    }
}

impl<M: RawMutex, S> ConnectedTaskControlResources<M, S> {
    pub const fn new() -> Self {
        Self {
            stop: Signal::new(),
            stopped: Signal::new(),
            state: AtomicU8::new(IDLE),
        }
    }

    /// Create exactly one parent/task capability pair for a fresh epoch.
    pub fn split(&self) -> Result<ConnectedTaskEndpoints<'_, M, S>, ConnectedTaskControlError> {
        match self
            .state
            .compare_exchange(IDLE, ACTIVE, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok((
                ConnectedTaskController {
                    resources: self,
                    completed: false,
                },
                ConnectedTaskEndpoint {
                    resources: self,
                    completed: false,
                },
            )),
            Err(ACTIVE) => Err(ConnectedTaskControlError::Active),
            Err(_) => Err(ConnectedTaskControlError::Poisoned),
        }
    }

    /// Reserve both sides of a future task epoch before moving any related
    /// DMA, IRQ or protocol owner.
    pub fn reserve(&self) -> Result<ConnectedTaskReservation<'_, M, S>, ConnectedTaskControlError> {
        let (controller, endpoint) = self.split()?;
        Ok(ConnectedTaskReservation {
            controller,
            endpoint,
        })
    }

    pub fn is_poisoned(&self) -> bool {
        self.state.load(Ordering::Acquire) == POISONED
    }
}

impl<M: RawMutex, S> Default for ConnectedTaskControlResources<M, S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Parent-side capability: request stop and recover the returned owner.
pub struct ConnectedTaskController<'resources, M: RawMutex, S> {
    resources: &'resources ConnectedTaskControlResources<M, S>,
    completed: bool,
}

impl<M: RawMutex, S> ConnectedTaskGroup for ConnectedTaskController<'_, M, S> {
    type Stopped = S;

    fn request_stop(&mut self) {
        if !self.completed {
            self.resources.stop.signal(());
        }
    }

    async fn wait_stopped(&mut self) -> Self::Stopped {
        let stopped = self.resources.stopped.wait().await;
        // Only `ConnectedTaskEndpoint::complete` can publish this value.
        // Its destructor has therefore already been disarmed.
        self.completed = true;
        self.resources.state.store(IDLE, Ordering::Release);
        stopped
    }
}

impl<M: RawMutex, S> Drop for ConnectedTaskController<'_, M, S> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.resources.state.compare_exchange(
                ACTIVE,
                POISONED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

/// Task-side capability: observe stop and return the exact stopped owner.
pub struct ConnectedTaskEndpoint<'resources, M: RawMutex, S> {
    resources: &'resources ConnectedTaskControlResources<M, S>,
    completed: bool,
}

impl<M: RawMutex, S> ConnectedTaskEndpoint<'_, M, S> {
    pub fn wait_for_stop(&self) -> impl Future<Output = ()> + '_ {
        self.resources.stop.wait()
    }

    /// Consume the task endpoint and publish its exact stopped owner.
    pub fn complete(mut self, stopped: S) {
        self.completed = true;
        self.resources.stopped.signal(stopped);
    }
}

impl<M: RawMutex, S> Drop for ConnectedTaskEndpoint<'_, M, S> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.resources.state.compare_exchange(
                ACTIVE,
                POISONED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

/// Result of one deadline-bounded observation of task shutdown.
///
/// `Pending` does not poison the task group and does not imply that radio
/// reset is required. The caller may keep waiting or observe the same stopping
/// transaction again; the task owners remain inside the group until
/// [`ConnectedTaskGroup::wait_stopped`] returns them.
pub enum ConnectedTaskStopAttempt<S> {
    Stopped(S),
    Pending { waited: Duration },
}

impl<S> ConnectedTaskStopAttempt<S> {
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

/// Request and await release of all executor tasks attached to one epoch.
///
/// This is the normal ownership transition. It has no internal deadline:
/// returning is the proof that every attached task released its exact owner.
pub async fn stop_connected_task_group<G>(group: &mut G) -> G::Stopped
where
    G: ConnectedTaskGroup,
{
    group.request_stop();
    group.wait_stopped().await
}

/// Request task shutdown and observe it for at most `timeout`.
///
/// The deadline covers the complete group, not each task independently.
/// Expiration is only a liveness observation; it never converts a still-owned
/// task graph into a quarantined hardware frontier.
pub async fn stop_connected_task_group_until<G>(
    group: &mut G,
    timeout: Duration,
) -> ConnectedTaskStopAttempt<G::Stopped>
where
    G: ConnectedTaskGroup,
{
    group.request_stop();
    match with_timeout(timeout, group.wait_stopped()).await {
        Ok(stopped) => ConnectedTaskStopAttempt::Stopped(stopped),
        Err(_) => ConnectedTaskStopAttempt::Pending { waited: timeout },
    }
}

#[cfg(test)]
mod tests {
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
}
