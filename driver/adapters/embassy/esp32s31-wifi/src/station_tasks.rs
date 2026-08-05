//! Finite shutdown boundary for tasks attached to one connected STA epoch.
//!
//! The radio runner returns its PAC/DMA owner from the parent future, while a
//! staged RX consumer or an application task may still retain borrowed
//! scratch or register access in an executor task. Those tasks must
//! acknowledge shutdown before connected teardown can recover the epoch.

use core::future::Future;

use embassy_time::{Duration, with_timeout};

/// Executor tasks which borrow resources from one connected station epoch.
///
/// The application chooses task placement and the concrete stop mechanism.
/// Before calling [`stop_esp32s31_connected_task_group`], it must prevent new
/// hardware publications (normally by stopping the radio runner and
/// quiescing the MAC interrupt route). `wait_stopped` must return every value
/// needed by the subsequent driver teardown.
pub trait Esp32s31ConnectedTaskGroup {
    type Stopped;

    /// Publish the idempotent cooperative stop request to every task.
    fn request_stop(&mut self);

    /// Wait until every task has released its connected-epoch borrows.
    fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_;
}

/// Result of the finite connected-task shutdown transaction.
///
/// A timeout is not a recoverable disconnect. At least one executor task may
/// still own epoch resources, so the caller must enter its platform reset
/// path rather than continue normal teardown or construct another epoch.
pub enum Esp32s31ConnectedTaskStopOutcome<S> {
    Stopped(S),
    ResetRequired { timeout: Duration },
}

impl<S> Esp32s31ConnectedTaskStopOutcome<S> {
    pub const fn is_reset_required(&self) -> bool {
        matches!(self, Self::ResetRequired { .. })
    }
}

/// Request and await release of all executor tasks attached to one epoch.
///
/// The deadline covers the complete group, not each task independently. This
/// prevents a task count from multiplying the maximum disconnect latency.
pub async fn stop_esp32s31_connected_task_group<G>(
    group: &mut G,
    timeout: Duration,
) -> Esp32s31ConnectedTaskStopOutcome<G::Stopped>
where
    G: Esp32s31ConnectedTaskGroup,
{
    group.request_stop();
    match with_timeout(timeout, group.wait_stopped()).await {
        Ok(stopped) => Esp32s31ConnectedTaskStopOutcome::Stopped(stopped),
        Err(_) => Esp32s31ConnectedTaskStopOutcome::ResetRequired { timeout },
    }
}

#[cfg(test)]
mod tests {
    use core::future::{pending, ready};

    use embassy_futures::block_on;

    use super::*;

    struct ReadyTaskGroup {
        stop_requests: usize,
        owner: Option<u32>,
    }

    impl Esp32s31ConnectedTaskGroup for ReadyTaskGroup {
        type Stopped = u32;

        fn request_stop(&mut self) {
            self.stop_requests += 1;
        }

        fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_ {
            ready(self.owner.take().expect("task owner is returned once"))
        }
    }

    #[test]
    fn one_deadline_returns_the_exact_task_owner() {
        let mut group = ReadyTaskGroup {
            stop_requests: 0,
            owner: Some(73),
        };
        let outcome = block_on(stop_esp32s31_connected_task_group(
            &mut group,
            Duration::from_millis(1),
        ));
        let Esp32s31ConnectedTaskStopOutcome::Stopped(owner) = outcome else {
            panic!("ready task group unexpectedly required reset");
        };
        assert_eq!(owner, 73);
        assert_eq!(group.stop_requests, 1);
    }

    struct StuckTaskGroup {
        stop_requested: bool,
    }

    impl Esp32s31ConnectedTaskGroup for StuckTaskGroup {
        type Stopped = ();

        fn request_stop(&mut self) {
            self.stop_requested = true;
        }

        fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_ {
            pending()
        }
    }

    #[test]
    fn missed_group_deadline_requires_platform_reset() {
        let mut group = StuckTaskGroup {
            stop_requested: false,
        };
        let outcome = block_on(stop_esp32s31_connected_task_group(
            &mut group,
            Duration::from_ticks(0),
        ));
        assert!(group.stop_requested);
        assert!(outcome.is_reset_required());
    }
}
