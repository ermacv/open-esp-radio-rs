//! Executor-neutral quiescence for one active LE DTM Test End.
//!
//! Test End is latched only after the start response entered HCI. Before a
//! scheduler head is visible, the runner cancels recurrence and drains any
//! abandoned Controller-time request. Once a head is visible, rollback is no
//! longer claimed: exactly that event reaches `RUN`, completes the ordinary
//! head-retirement/unlink/recycle chain and then returns the graph to a
//! response-retained stopping owner. No transition stops the common scheduler
//! or copies vendor queue teardown.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerDeferredTestEnd, LeControllerResponsePending, LeControllerResponsePublication,
};

use crate::le::dtm::active::session::BluetoothDtmActiveRadio;
use crate::le::dtm::quiescence::{
    BluetoothDtmQuiescenceFault, BluetoothDtmQuiescenceFaultCause,
    BluetoothDtmQuiescenceRetryCause, BluetoothDtmQuiescenceRunner, BluetoothDtmQuiescenceStep,
    BluetoothDtmQuiescenceWait,
};
use crate::le::dtm::session::BluetoothDtmSessionStopping;
use crate::{
    BluetoothControllerIdleCommandTask, BluetoothControllerPublishedTaskService,
    BluetoothDtmActiveCompletionFaultCause, BluetoothDtmActiveCpuOwned,
    BluetoothDtmPostUnlinkWakeCell, BluetoothDtmRecurringFaultCause, BluetoothDtmSessionIdle,
    BluetoothDtmTestEndReport, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerWakeCell,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

/// A latched Test End command and the exact active graph it must quiesce.
#[must_use = "advance, wait, retry or retain the complete Test End transaction"]
pub struct BluetoothDtmStoppingRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    quiescence: BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>,
}

/// Borrowed wait source for the current stopping phase.
#[derive(Clone, Copy)]
pub enum BluetoothDtmStoppingWait<'runner> {
    /// The active scheduler event has not produced a finished-list observation.
    Scheduler(&'runner BluetoothSchedulerWakeCell),
    /// The exact unlinked graph awaits its primary mailbox event.
    PostUnlink(&'runner BluetoothDtmPostUnlinkWakeCell),
    /// An abandoned Controller-time latch needs a cooperative later recheck.
    ControllerTime,
}

/// Why the unchanged stopping owner needs an explicit later retry.
pub enum BluetoothDtmStoppingRetryCause<'cause, E> {
    /// A CPU-owned prepared merge rejected cancellation without publishing HEAD.
    CancellationRejected,
    /// HEAD is visible and the dynamic scheduler-start suffix must be retried.
    SchedulerStart(&'cause E),
}

/// One bounded Test End transition.
#[must_use = "retain every affine owner until response-ready or fail-stop"]
pub enum BluetoothDtmStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Another finite stopping transition may run immediately.
    Continue(BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>),
    /// The exact runner is parked on its borrowed wait source.
    Waiting(BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>),
    /// One unrelated finished list still belongs to the external dispatcher.
    UnrelatedList {
        /// Unchanged Test End transaction.
        runner: BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>,
        /// Exact unrelated list observation.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// A finite cancellation or scheduler-start operation retained its owner.
    Retryable(BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>),
    /// The graph is fully CPU-owned and its terminal report is stable.
    ResponseReady(BluetoothDtmTestEndReady<'runtime, S, CAPACITY>),
    /// A fail-closed lower transition retained command, HCI order and graph.
    Fault(BluetoothDtmStoppingFault<'runtime, S, CAPACITY>),
}

/// Stable Test End response beside the exact task, HCI order and reclaimed graph.
///
/// The HCI integration layer consumes this opaque owner into its endpoint-bound
/// response publication transaction. It cannot release the graph through this
/// public surface.
#[must_use = "publish the Test End response before restoring the graph"]
pub struct BluetoothDtmTestEndReady<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    task: Task<'runtime, S, CAPACITY>,
    stopping: BluetoothDtmSessionStopping,
}

impl<S, const CAPACITY: usize> BluetoothDtmTestEndReady<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Stable role-specific result used by LE Test End Command Complete.
    pub const fn report(&self) -> BluetoothDtmTestEndReport {
        self.stopping.report()
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmTestEndReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Bind the terminal response to the same HCI order epoch as the accepted
    /// Test End command.
    ///
    /// The task and reclaimed graph become the private radio owner of the
    /// generic response transaction. Neither can be released before the event
    /// is durably enqueued.
    pub fn into_response_pending(
        self,
    ) -> BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY> {
        let packet_count = self.report().reported_packet_count();
        let recovery = BluetoothDtmTestEndRecovery {
            task: self.task,
            stopping: self.stopping,
        };
        BluetoothDtmTestEndResponsePending {
            transaction: self
                .deferred
                .map_owner(|()| recovery)
                .into_ended_response(packet_count),
        }
    }
}

struct BluetoothDtmTestEndRecovery<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    stopping: BluetoothDtmSessionStopping,
}

/// Terminal Test End response retaining the task and graph across HCI backpressure.
#[must_use = "publish the exact response or retain the complete terminal owner"]
pub struct BluetoothDtmTestEndResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction:
        LeControllerResponsePending<'runtime, BluetoothDtmTestEndRecovery<'runtime, S, CAPACITY>>,
}

/// Result of one consuming Test End response publication attempt.
#[must_use = "retain backpressure, fault, restore failure or the completed task owner"]
pub enum BluetoothDtmTestEndResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The response was enqueued and the idle graph restored exactly once.
    Completed(BluetoothDtmTestEndComplete<'runtime, S, CAPACITY>),
    /// C2H capacity was unavailable; the complete owner is unchanged.
    Pending(BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>),
    /// The supplied endpoint belongs to another HCI epoch.
    EndpointMismatch(BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>),
    /// A non-capacity transport failure retained response, task and graph.
    Fault {
        pending: BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
    /// Publication succeeded, but the private idle slot rejected graph restore.
    RestoreFailed(BluetoothDtmTestEndRestoreFailure<'runtime, S, CAPACITY>),
}

/// Controller task after Test End publication and successful idle-graph restore.
#[must_use = "return the task owner to the sole HCI/controller session loop"]
pub struct BluetoothDtmTestEndComplete<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmTestEndComplete<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Return the sole idle command task after the DTM runtime is idle again.
    pub fn into_idle_command_task(
        self,
    ) -> BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY> {
        self.task
    }
}

/// Post-publication fail-stop owner for an unexpected idle-graph rejection.
///
/// The response authority is already gone, so retrying restore can never
/// publish Test End twice.
#[must_use = "retry idle restore or retain the post-publication owner"]
pub struct BluetoothDtmTestEndRestoreFailure<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    ready: LeControllerCommandReady<'runtime, ()>,
    idle: BluetoothDtmSessionIdle,
}

/// Result of retrying only the post-publication idle-graph restore.
#[must_use = "retain the completed task or the unchanged restore owner"]
pub enum BluetoothDtmTestEndRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The private DTM runtime accepted its graph and is idle again.
    Completed(BluetoothDtmTestEndComplete<'runtime, S, CAPACITY>),
    /// The slot still rejected the graph; response publication cannot repeat.
    Rejected(BluetoothDtmTestEndRestoreFailure<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmTestEndRestoreFailure<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Retry only graph restore; the already-published response cannot recur.
    pub fn retry_restore(mut self) -> BluetoothDtmTestEndRestoreStep<'runtime, S, CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => BluetoothDtmTestEndRestoreStep::Completed(BluetoothDtmTestEndComplete {
                task: BluetoothControllerIdleCommandTask::from_parts(self.task, self.ready),
            }),
            Err(idle) => {
                self.idle = idle;
                BluetoothDtmTestEndRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Whether an endpoint may publish this exact terminal response.
    pub fn matches_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.transaction.matches_endpoint(controller)
    }

    /// Wait until the matching Controller-to-Host queue may accept Test End.
    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    /// Attempt exact-once Test End publication through the matching HCI epoch.
    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothDtmTestEndResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(published) => {
                let (recovery, ready) = published.into_parts();
                let idle = recovery.stopping.response_published();
                let mut task = recovery.task;
                match task.restore_dtm_session_idle(idle) {
                    Ok(()) => BluetoothDtmTestEndResponsePublication::Completed(
                        BluetoothDtmTestEndComplete {
                            task: BluetoothControllerIdleCommandTask::from_parts(task, ready),
                        },
                    ),
                    Err(idle) => BluetoothDtmTestEndResponsePublication::RestoreFailed(
                        BluetoothDtmTestEndRestoreFailure { task, ready, idle },
                    ),
                }
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothDtmTestEndResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothDtmTestEndResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothDtmTestEndResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

/// Read-only fail-stop classification for Test End quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmStoppingFaultCause {
    /// The ordinary active-event completion chain failed closed.
    Completion(BluetoothDtmActiveCompletionFaultCause),
    /// Recurring cancellation, preparation or start failed closed.
    Recurring(BluetoothDtmRecurringFaultCause),
    /// An already published HEAD returned an impossible non-start transition.
    UnexpectedPublishedHeadTransition,
}

/// Opaque fail-stop Test End owner.
#[must_use = "retain the exact command and graph for diagnostic shutdown"]
pub struct BluetoothDtmStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothDtmStoppingFaultCause,
    _deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    _quiescence: BluetoothDtmQuiescenceFault<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothDtmStoppingFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Exact fail-closed classification without exposing retained owners.
    pub const fn cause(&self) -> BluetoothDtmStoppingFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn new(
        radio: BluetoothDtmActiveRadio<'runtime, S, CAPACITY>,
        deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    ) -> Self {
        Self {
            deferred,
            quiescence: BluetoothDtmQuiescenceRunner::new(radio),
        }
    }

    /// Borrow the exact current wait source without moving the affine runner.
    pub fn wait(&self) -> Option<BluetoothDtmStoppingWait<'_>> {
        match self.quiescence.wait() {
            Some(BluetoothDtmQuiescenceWait::Scheduler(wake)) => {
                Some(BluetoothDtmStoppingWait::Scheduler(wake))
            }
            Some(BluetoothDtmQuiescenceWait::PostUnlink(wake)) => {
                Some(BluetoothDtmStoppingWait::PostUnlink(wake))
            }
            Some(BluetoothDtmQuiescenceWait::ControllerTime) => {
                Some(BluetoothDtmStoppingWait::ControllerTime)
            }
            None => None,
        }
    }

    /// Borrow the current retry reason without exposing its retained owner.
    pub fn retry_cause(&self) -> Option<BluetoothDtmStoppingRetryCause<'_, S::Error>> {
        match self.quiescence.retry_cause() {
            Some(BluetoothDtmQuiescenceRetryCause::CancellationRejected) => {
                Some(BluetoothDtmStoppingRetryCause::CancellationRejected)
            }
            Some(BluetoothDtmQuiescenceRetryCause::SchedulerStart(error)) => {
                Some(BluetoothDtmStoppingRetryCause::SchedulerStart(error))
            }
            None => None,
        }
    }

    /// Execute exactly one cancellation, drain, start or completion transition.
    pub fn step(self) -> BluetoothDtmStoppingStep<'runtime, S, CAPACITY> {
        let Self {
            deferred,
            quiescence,
        } = self;
        match quiescence.step() {
            BluetoothDtmQuiescenceStep::Continue(quiescence) => {
                BluetoothDtmStoppingStep::Continue(Self {
                    deferred,
                    quiescence,
                })
            }
            BluetoothDtmQuiescenceStep::Waiting(quiescence) => {
                BluetoothDtmStoppingStep::Waiting(Self {
                    deferred,
                    quiescence,
                })
            }
            BluetoothDtmQuiescenceStep::UnrelatedList {
                runner: quiescence,
                observed,
            } => BluetoothDtmStoppingStep::UnrelatedList {
                runner: Self {
                    deferred,
                    quiescence,
                },
                observed,
            },
            BluetoothDtmQuiescenceStep::Retryable(quiescence) => {
                BluetoothDtmStoppingStep::Retryable(Self {
                    deferred,
                    quiescence,
                })
            }
            BluetoothDtmQuiescenceStep::CpuOwned(owner) => response_ready(deferred, owner),
            BluetoothDtmQuiescenceStep::Fault(quiescence) => {
                BluetoothDtmStoppingStep::Fault(BluetoothDtmStoppingFault {
                    cause: stopping_fault_cause(quiescence.cause()),
                    _deferred: deferred,
                    _quiescence: quiescence,
                })
            }
        }
    }
}

const fn stopping_fault_cause(
    cause: BluetoothDtmQuiescenceFaultCause,
) -> BluetoothDtmStoppingFaultCause {
    match cause {
        BluetoothDtmQuiescenceFaultCause::Completion(cause) => {
            BluetoothDtmStoppingFaultCause::Completion(cause)
        }
        BluetoothDtmQuiescenceFaultCause::Recurring(cause) => {
            BluetoothDtmStoppingFaultCause::Recurring(cause)
        }
        BluetoothDtmQuiescenceFaultCause::UnexpectedPublishedHeadTransition => {
            BluetoothDtmStoppingFaultCause::UnexpectedPublishedHeadTransition
        }
    }
}

fn response_ready<'runtime, S, const CAPACITY: usize>(
    deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    owner: BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>,
) -> BluetoothDtmStoppingStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (task, ended) = match owner {
        BluetoothDtmActiveCpuOwned::Transmitter(ready) => {
            let (task, owner) = ready.into_parts();
            (task, owner.into_test_ended())
        }
        BluetoothDtmActiveCpuOwned::Receiver(ready) => {
            let (task, owner, _status, _outcome) = ready.into_parts();
            (task, owner.into_test_ended())
        }
    };
    BluetoothDtmStoppingStep::ResponseReady(BluetoothDtmTestEndReady {
        deferred,
        task,
        stopping: BluetoothDtmSessionStopping::new(ended),
    })
}
