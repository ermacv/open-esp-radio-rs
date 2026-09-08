//! Executor-neutral quiescence for one active LE DTM Test End.
//!
//! Test End is latched only after the start response entered HCI. Before a
//! scheduler head is visible, the runner cancels recurrence and drains any
//! abandoned Controller-time request. Once a head is visible, rollback is no
//! longer claimed: exactly that event reaches `RUN`, enters common scheduler
//! stop if still running, and completes exact head retirement/unlink/recycle.
//! The returned graph remains owned across HCI response backpressure.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerIdleCommandTask, ControllerPublishedTaskService, SchedulerRunInterruptStorage,
    },
    interrupt::SchedulerWakeCell,
    le::dtm::{
        DtmActiveCompletionFaultCause, DtmActiveCpuOwned, DtmPostUnlinkWakeCell,
        DtmRecurringFaultCause, DtmSessionIdle, DtmTestEndReport,
        active::session::DtmActiveRadio,
        quiescence::{
            DtmQuiescenceFault, DtmQuiescenceFaultCause, DtmQuiescenceRetryCause,
            DtmQuiescenceRunner, DtmQuiescenceStep, DtmQuiescenceWait,
        },
        session::DtmSessionStopping,
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerDeferredTestEnd, LeControllerResponsePending, LeControllerResponsePublication,
};

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;

/// A latched Test End command and the exact active graph it must quiesce.
#[must_use = "advance, wait, retry or retain the complete Test End transaction"]
pub struct DtmStoppingRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    quiescence: DtmQuiescenceRunner<'runtime, S, CAPACITY>,
}

/// Borrowed wait source for the current stopping phase.
#[derive(Clone, Copy)]
pub enum BluetoothDtmStoppingWait<'runner> {
    /// The active scheduler event has not produced a finished-list observation.
    Scheduler(&'runner SchedulerWakeCell),
    /// The exact unlinked graph awaits its primary mailbox event.
    PostUnlink(&'runner DtmPostUnlinkWakeCell),
    /// An abandoned Controller-time latch needs a cooperative later recheck.
    ControllerTime,
}

/// Why the unchanged stopping owner needs an explicit later retry.
pub enum DtmStoppingRetryCause<'cause, E> {
    /// A CPU-owned prepared merge rejected cancellation without publishing HEAD.
    CancellationRejected,
    /// HEAD is visible and the dynamic scheduler-start suffix must be retried.
    SchedulerStart(&'cause E),
}

/// One bounded Test End transition.
#[must_use = "retain every affine owner until response-ready or fail-stop"]
pub enum DtmStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// Another finite stopping transition may run immediately.
    Continue(DtmStoppingRunner<'runtime, S, CAPACITY>),
    /// The exact runner is parked on its borrowed wait source.
    Waiting(DtmStoppingRunner<'runtime, S, CAPACITY>),
    /// One unrelated finished list still belongs to the external dispatcher.
    UnrelatedList {
        /// Unchanged Test End transaction.
        runner: DtmStoppingRunner<'runtime, S, CAPACITY>,
        /// Exact unrelated list observation.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// A finite cancellation or scheduler-start operation retained its owner.
    Retryable(DtmStoppingRunner<'runtime, S, CAPACITY>),
    /// The graph is fully CPU-owned and its terminal report is stable.
    ResponseReady(DtmTestEndReady<'runtime, S, CAPACITY>),
    /// A fail-closed lower transition retained command, HCI order and graph.
    Fault(DtmStoppingFault<'runtime, S, CAPACITY>),
}

/// Stable Test End response beside the exact task, HCI order and reclaimed graph.
///
/// The HCI integration layer consumes this opaque owner into its endpoint-bound
/// response publication transaction. It cannot release the graph through this
/// public surface.
#[must_use = "publish the Test End response before restoring the graph"]
pub struct DtmTestEndReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    task: Task<'runtime, S, CAPACITY>,
    stopping: DtmSessionStopping,
}

impl<S, const CAPACITY: usize> DtmTestEndReady<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Stable role-specific result used by LE Test End Command Complete.
    pub const fn report(&self) -> DtmTestEndReport {
        self.stopping.report()
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmTestEndReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Bind the terminal response to the same HCI order epoch as the accepted
    /// Test End command.
    ///
    /// The task and reclaimed graph become the private radio owner of the
    /// generic response transaction. Neither can be released before the event
    /// is durably enqueued.
    pub fn into_response_pending(self) -> DtmTestEndResponsePending<'runtime, S, CAPACITY> {
        let packet_count = self.report().reported_packet_count();
        let recovery = DtmTestEndRecovery {
            task: self.task,
            stopping: self.stopping,
        };
        DtmTestEndResponsePending {
            transaction: self
                .deferred
                .map_owner(|()| recovery)
                .into_ended_response(packet_count),
        }
    }
}

struct DtmTestEndRecovery<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    stopping: DtmSessionStopping,
}

/// Terminal Test End response retaining the task and graph across HCI backpressure.
#[must_use = "publish the exact response or retain the complete terminal owner"]
pub struct DtmTestEndResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, DtmTestEndRecovery<'runtime, S, CAPACITY>>,
}

/// Result of one consuming Test End response publication attempt.
#[must_use = "retain backpressure, fault, restore failure or the completed task owner"]
pub enum DtmTestEndResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// The response was enqueued and the idle graph restored exactly once.
    Completed(DtmTestEndComplete<'runtime, S, CAPACITY>),
    /// C2H capacity was unavailable; the complete owner is unchanged.
    Pending(DtmTestEndResponsePending<'runtime, S, CAPACITY>),
    /// The supplied endpoint belongs to another HCI epoch.
    EndpointMismatch(DtmTestEndResponsePending<'runtime, S, CAPACITY>),
    /// A non-capacity transport failure retained response, task and graph.
    Fault {
        pending: DtmTestEndResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
    /// Publication succeeded, but the private idle slot rejected graph restore.
    RestoreFailed(DtmTestEndRestoreFailure<'runtime, S, CAPACITY>),
}

/// Controller task after Test End publication and successful idle-graph restore.
#[must_use = "return the task owner to the sole HCI/controller session loop"]
pub struct DtmTestEndComplete<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerIdleCommandTask<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> DtmTestEndComplete<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Return the sole idle command task after the DTM runtime is idle again.
    pub fn into_idle_command_task(self) -> ControllerIdleCommandTask<'runtime, S, CAPACITY> {
        self.task
    }
}

/// Post-publication fail-stop owner for an unexpected idle-graph rejection.
///
/// The response authority is already gone, so retrying restore can never
/// publish Test End twice.
#[must_use = "retry idle restore or retain the post-publication owner"]
pub struct DtmTestEndRestoreFailure<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    ready: LeControllerCommandReady<'runtime, ()>,
    idle: DtmSessionIdle,
}

/// Result of retrying only the post-publication idle-graph restore.
#[must_use = "retain the completed task or the unchanged restore owner"]
pub enum DtmTestEndRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// The private DTM runtime accepted its graph and is idle again.
    Completed(DtmTestEndComplete<'runtime, S, CAPACITY>),
    /// The slot still rejected the graph; response publication cannot repeat.
    Rejected(DtmTestEndRestoreFailure<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize> DtmTestEndRestoreFailure<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Retry only graph restore; the already-published response cannot recur.
    pub fn retry_restore(mut self) -> DtmTestEndRestoreStep<'runtime, S, CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => DtmTestEndRestoreStep::Completed(DtmTestEndComplete {
                task: ControllerIdleCommandTask::from_parts(self.task, self.ready),
            }),
            Err(idle) => {
                self.idle = idle;
                DtmTestEndRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmTestEndResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
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
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
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
    ) -> DtmTestEndResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(published) => {
                let (recovery, ready) = published.into_parts();
                let idle = recovery.stopping.response_published();
                let mut task = recovery.task;
                match task.restore_dtm_session_idle(idle) {
                    Ok(()) => DtmTestEndResponsePublication::Completed(DtmTestEndComplete {
                        task: ControllerIdleCommandTask::from_parts(task, ready),
                    }),
                    Err(idle) => {
                        DtmTestEndResponsePublication::RestoreFailed(DtmTestEndRestoreFailure {
                            task,
                            ready,
                            idle,
                        })
                    }
                }
            }
            LeControllerResponsePublication::Pending(transaction) => {
                DtmTestEndResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                DtmTestEndResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => DtmTestEndResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

/// Read-only fail-stop classification for Test End quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmStoppingFaultCause {
    /// The retained common quiescence budget elapsed; no owner was released.
    DeadlineExpired,
    /// The ordinary active-event completion chain failed closed.
    Completion(DtmActiveCompletionFaultCause),
    /// Recurring cancellation, preparation or start failed closed.
    Recurring(DtmRecurringFaultCause),
    /// An already published HEAD returned an impossible non-start transition.
    UnexpectedPublishedHeadTransition,
}

/// Opaque fail-stop Test End owner.
#[must_use = "retain the exact command and graph for diagnostic shutdown"]
pub struct DtmStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: DtmStoppingFaultCause,
    _deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    _quiescence: DtmQuiescenceFault<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> DtmStoppingFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Exact fail-closed classification without exposing retained owners.
    pub const fn cause(&self) -> DtmStoppingFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmStoppingRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn new(
        radio: DtmActiveRadio<'runtime, S, CAPACITY>,
        deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    ) -> Self {
        Self {
            deferred,
            quiescence: DtmQuiescenceRunner::new(radio),
        }
    }

    /// Borrow the exact current wait source without moving the affine runner.
    pub fn wait(&self) -> Option<BluetoothDtmStoppingWait<'_>> {
        match self.quiescence.wait() {
            Some(DtmQuiescenceWait::PostUnlink(wake)) => {
                Some(BluetoothDtmStoppingWait::PostUnlink(wake))
            }
            Some(DtmQuiescenceWait::ControllerTime) => {
                Some(BluetoothDtmStoppingWait::ControllerTime)
            }
            None => None,
        }
    }

    /// Borrow the current retry reason without exposing its retained owner.
    pub fn retry_cause(&self) -> Option<DtmStoppingRetryCause<'_, S::Error>> {
        match self.quiescence.retry_cause() {
            Some(DtmQuiescenceRetryCause::CancellationRejected) => {
                Some(DtmStoppingRetryCause::CancellationRejected)
            }
            Some(DtmQuiescenceRetryCause::SchedulerStart(error)) => {
                Some(DtmStoppingRetryCause::SchedulerStart(error))
            }
            None => None,
        }
    }

    /// Execute exactly one cancellation, drain, start or completion transition.
    pub fn step(self) -> DtmStoppingStep<'runtime, S, CAPACITY> {
        let Self {
            deferred,
            quiescence,
        } = self;
        match quiescence.step() {
            DtmQuiescenceStep::Continue(quiescence) => DtmStoppingStep::Continue(Self {
                deferred,
                quiescence,
            }),
            DtmQuiescenceStep::Waiting(quiescence) => DtmStoppingStep::Waiting(Self {
                deferred,
                quiescence,
            }),
            DtmQuiescenceStep::UnrelatedList {
                runner: quiescence,
                observed,
            } => DtmStoppingStep::UnrelatedList {
                runner: Self {
                    deferred,
                    quiescence,
                },
                observed,
            },
            DtmQuiescenceStep::Retryable(quiescence) => DtmStoppingStep::Retryable(Self {
                deferred,
                quiescence,
            }),
            DtmQuiescenceStep::CpuOwned(owner) => response_ready(deferred, owner),
            DtmQuiescenceStep::Fault(quiescence) => DtmStoppingStep::Fault(DtmStoppingFault {
                cause: stopping_fault_cause(quiescence.cause()),
                _deferred: deferred,
                _quiescence: quiescence,
            }),
        }
    }
}

const fn stopping_fault_cause(cause: DtmQuiescenceFaultCause) -> DtmStoppingFaultCause {
    match cause {
        DtmQuiescenceFaultCause::DeadlineExpired => DtmStoppingFaultCause::DeadlineExpired,
        DtmQuiescenceFaultCause::Completion(cause) => DtmStoppingFaultCause::Completion(cause),
        DtmQuiescenceFaultCause::Recurring(cause) => DtmStoppingFaultCause::Recurring(cause),
        DtmQuiescenceFaultCause::UnexpectedPublishedHeadTransition => {
            DtmStoppingFaultCause::UnexpectedPublishedHeadTransition
        }
    }
}

fn response_ready<'runtime, S, const CAPACITY: usize>(
    deferred: LeControllerDeferredTestEnd<'runtime, ()>,
    owner: DtmActiveCpuOwned<'runtime, S, CAPACITY>,
) -> DtmStoppingStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (task, ended) = match owner {
        DtmActiveCpuOwned::Transmitter(ready) => {
            let (task, owner) = ready.into_parts();
            (task, owner.into_test_ended())
        }
        DtmActiveCpuOwned::Receiver(ready) => {
            let (task, owner, _status, _outcome) = ready.into_parts();
            (task, owner.into_test_ended())
        }
    };
    DtmStoppingStep::ResponseReady(DtmTestEndReady {
        deferred,
        task,
        stopping: DtmSessionStopping::new(ended),
    })
}
