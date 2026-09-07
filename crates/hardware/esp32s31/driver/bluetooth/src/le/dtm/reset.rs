//! Reset-specific ownership around the shared active-DTM quiescence machine.
//!
//! Hardware quiescence, graph reclaim and restoration of the runtime idle slot
//! all complete before the retained portable Reset may mutate bootstrap state.
//! HCI publication then retains the already-idle task across endpoint mismatch,
//! backpressure and transport failure.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerIdleCommandTask, ControllerPublishedTaskService, SchedulerRunInterruptStorage,
    },
    interrupt::SchedulerWakeCell,
    le::dtm::{
        DtmActiveCompletionFaultCause, DtmActiveCpuOwned, DtmPostUnlinkWakeCell,
        DtmRecurringFaultCause, DtmSessionIdle,
        active::session::DtmActiveRadio,
        quiescence::{
            DtmQuiescenceFault, DtmQuiescenceFaultCause, DtmQuiescenceRetryCause,
            DtmQuiescenceRunner, DtmQuiescenceStep, DtmQuiescenceWait,
        },
        reset_order::{DtmRestoredReset, DtmRestoredResetCompletion},
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerResetBarrier,
    LeControllerResponsePending, LeControllerResponsePublication,
};

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;

/// Accepted Reset and exact active graph advancing through neutral quiescence.
#[must_use = "advance, wait, retry or retain the complete Reset transaction"]
pub struct DtmResetStoppingRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    barrier: LeControllerResetBarrier<'runtime, ()>,
    quiescence: DtmQuiescenceRunner<'runtime, S, CAPACITY>,
}

/// Borrowed wait source for the current Reset-quiescence phase.
#[derive(Clone, Copy)]
pub enum DtmResetStoppingWait<'runner> {
    Scheduler(&'runner SchedulerWakeCell),
    PostUnlink(&'runner DtmPostUnlinkWakeCell),
    ControllerTime,
}

/// Why Reset quiescence retained its exact owner for an explicit retry.
pub enum DtmResetStoppingRetryCause<'cause, E> {
    CancellationRejected,
    SchedulerStart(&'cause E),
}

/// Read-only fail-stop classification for Reset quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmResetStoppingFaultCause {
    Completion(DtmActiveCompletionFaultCause),
    Recurring(DtmRecurringFaultCause),
    UnexpectedPublishedHeadTransition,
}

/// Opaque fail-stop owner retaining Reset/order and the exact active graph.
#[must_use = "retain the complete Reset quiescence owner for diagnostic shutdown"]
pub struct DtmResetStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: DtmResetStoppingFaultCause,
    _barrier: LeControllerResetBarrier<'runtime, ()>,
    _quiescence: DtmQuiescenceFault<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> DtmResetStoppingFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> DtmResetStoppingFaultCause {
        self.cause
    }
}

/// One bounded Reset-quiescence transition.
#[must_use = "retain every affine owner until idle restore or fail-stop"]
pub enum DtmResetStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(DtmResetStoppingRunner<'runtime, S, CAPACITY>),
    Waiting(DtmResetStoppingRunner<'runtime, S, CAPACITY>),
    UnrelatedList {
        runner: DtmResetStoppingRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Retryable(DtmResetStoppingRunner<'runtime, S, CAPACITY>),
    /// Radio is quiescent and the exact graph is restored in the task idle slot.
    CompletionReady(DtmResetCompletionReady<'runtime, S, CAPACITY>),
    /// Radio is quiescent, but the exact graph has not yet re-entered its idle slot.
    RestoreFailed(DtmResetRestoreFailure<'runtime, S, CAPACITY>),
    Fault(DtmResetStoppingFault<'runtime, S, CAPACITY>),
}

/// Quiesced Reset owner after successful graph restore and before bootstrap mutation.
#[must_use = "complete Reset through the matching combined Controller endpoint"]
pub struct DtmResetCompletionReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    restored: DtmRestoredReset<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Quiesced Reset owner whose exact graph was rejected by the runtime idle slot.
#[must_use = "retry idle restore without applying or publishing Reset"]
pub struct DtmResetRestoreFailure<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    idle: DtmSessionIdle,
    barrier: LeControllerResetBarrier<'runtime, ()>,
}

/// Result of retrying only the pre-Reset idle graph restore.
#[must_use = "retain either the completion-ready owner or unchanged restore failure"]
pub enum DtmResetRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    CompletionReady(DtmResetCompletionReady<'runtime, S, CAPACITY>),
    Rejected(DtmResetRestoreFailure<'runtime, S, CAPACITY>),
}

/// Result of applying a quiesced Reset through one combined Controller endpoint.
#[must_use = "publish the response or retain the exact endpoint mismatch"]
pub enum DtmResetCompletionStart<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(DtmResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(DtmResetCompletionReady<'runtime, S, CAPACITY>),
}

/// Applied Reset whose Command Complete retains the already-idle task.
#[must_use = "publish the exact Reset response or retain the complete owner"]
pub struct DtmResetResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Result of one consuming Reset response publication attempt.
#[must_use = "retain backpressure, mismatch, fault or the completed idle task"]
pub enum DtmResetResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completed(DtmResetComplete<'runtime, S, CAPACITY>),
    Pending(DtmResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(DtmResetResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: DtmResetResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Sole Controller task after Reset response publication with DTM already idle.
#[must_use = "return the task owner to the sole Controller session loop"]
pub struct DtmResetComplete<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerIdleCommandTask<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> DtmResetComplete<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn into_idle_command_task(self) -> ControllerIdleCommandTask<'runtime, S, CAPACITY> {
        self.task
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmResetStoppingRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn new(
        radio: DtmActiveRadio<'runtime, S, CAPACITY>,
        barrier: LeControllerResetBarrier<'runtime, ()>,
    ) -> Self {
        Self {
            barrier,
            quiescence: DtmQuiescenceRunner::new(radio),
        }
    }

    pub fn wait(&self) -> Option<DtmResetStoppingWait<'_>> {
        match self.quiescence.wait() {
            Some(DtmQuiescenceWait::Scheduler(wake)) => Some(DtmResetStoppingWait::Scheduler(wake)),
            Some(DtmQuiescenceWait::PostUnlink(wake)) => {
                Some(DtmResetStoppingWait::PostUnlink(wake))
            }
            Some(DtmQuiescenceWait::ControllerTime) => Some(DtmResetStoppingWait::ControllerTime),
            None => None,
        }
    }

    pub fn retry_cause(&self) -> Option<DtmResetStoppingRetryCause<'_, S::Error>> {
        match self.quiescence.retry_cause() {
            Some(DtmQuiescenceRetryCause::CancellationRejected) => {
                Some(DtmResetStoppingRetryCause::CancellationRejected)
            }
            Some(DtmQuiescenceRetryCause::SchedulerStart(error)) => {
                Some(DtmResetStoppingRetryCause::SchedulerStart(error))
            }
            None => None,
        }
    }

    pub fn step(self) -> DtmResetStoppingStep<'runtime, S, CAPACITY> {
        let Self {
            barrier,
            quiescence,
        } = self;
        match quiescence.step() {
            DtmQuiescenceStep::Continue(quiescence) => DtmResetStoppingStep::Continue(Self {
                barrier,
                quiescence,
            }),
            DtmQuiescenceStep::Waiting(quiescence) => DtmResetStoppingStep::Waiting(Self {
                barrier,
                quiescence,
            }),
            DtmQuiescenceStep::UnrelatedList {
                runner: quiescence,
                observed,
            } => DtmResetStoppingStep::UnrelatedList {
                runner: Self {
                    barrier,
                    quiescence,
                },
                observed,
            },
            DtmQuiescenceStep::Retryable(quiescence) => DtmResetStoppingStep::Retryable(Self {
                barrier,
                quiescence,
            }),
            DtmQuiescenceStep::CpuOwned(owner) => restore_quiesced(barrier, owner),
            DtmQuiescenceStep::Fault(quiescence) => {
                DtmResetStoppingStep::Fault(DtmResetStoppingFault {
                    cause: reset_fault_cause(quiescence.cause()),
                    _barrier: barrier,
                    _quiescence: quiescence,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmResetRestoreFailure<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn retry_restore(mut self) -> DtmResetRestoreStep<'runtime, S, CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => DtmResetRestoreStep::CompletionReady(DtmResetCompletionReady {
                restored: DtmRestoredReset::new(self.barrier.map_owner(|()| self.task)),
            }),
            Err(idle) => {
                self.idle = idle;
                DtmResetRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmResetCompletionReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
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
        self.restored.matches_endpoint(controller)
    }

    /// Apply Reset only after quiescence and idle restoration proved readiness.
    pub fn complete<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> DtmResetCompletionStart<'runtime, S, CAPACITY> {
        match self.restored.complete(controller) {
            DtmRestoredResetCompletion::ResponsePending(transaction) => {
                DtmResetCompletionStart::ResponsePending(DtmResetResponsePending { transaction })
            }
            DtmRestoredResetCompletion::EndpointMismatch(restored) => {
                DtmResetCompletionStart::EndpointMismatch(Self { restored })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmResetResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
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

    /// Wait until the matching Controller-to-Host queue may accept Reset completion.
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
    ) -> DtmResetResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(published) => {
                DtmResetResponsePublication::Completed(DtmResetComplete {
                    task: ControllerIdleCommandTask::from_ready(published),
                })
            }
            LeControllerResponsePublication::Pending(transaction) => {
                DtmResetResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                DtmResetResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => DtmResetResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

fn restore_quiesced<'runtime, S, const CAPACITY: usize>(
    barrier: LeControllerResetBarrier<'runtime, ()>,
    owner: DtmActiveCpuOwned<'runtime, S, CAPACITY>,
) -> DtmResetStoppingStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (mut task, quiesced) = match owner {
        DtmActiveCpuOwned::Transmitter(ready) => {
            let (task, owner) = ready.into_parts();
            (task, owner.into_quiesced())
        }
        DtmActiveCpuOwned::Receiver(ready) => {
            let (task, owner, _status, _outcome) = ready.into_parts();
            (task, owner.into_quiesced())
        }
    };
    let idle = DtmSessionIdle::from_quiesced(quiesced);
    match task.restore_dtm_session_idle(idle) {
        Ok(()) => DtmResetStoppingStep::CompletionReady(DtmResetCompletionReady {
            restored: DtmRestoredReset::new(barrier.map_owner(|()| task)),
        }),
        Err(idle) => DtmResetStoppingStep::RestoreFailed(DtmResetRestoreFailure {
            task,
            idle,
            barrier,
        }),
    }
}

const fn reset_fault_cause(cause: DtmQuiescenceFaultCause) -> DtmResetStoppingFaultCause {
    match cause {
        DtmQuiescenceFaultCause::Completion(cause) => DtmResetStoppingFaultCause::Completion(cause),
        DtmQuiescenceFaultCause::Recurring(cause) => DtmResetStoppingFaultCause::Recurring(cause),
        DtmQuiescenceFaultCause::UnexpectedPublishedHeadTransition => {
            DtmResetStoppingFaultCause::UnexpectedPublishedHeadTransition
        }
    }
}
