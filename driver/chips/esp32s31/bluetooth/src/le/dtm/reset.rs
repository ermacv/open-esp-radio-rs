//! Reset-specific ownership around the shared active-DTM quiescence machine.
//!
//! Hardware quiescence, graph reclaim and restoration of the runtime idle slot
//! all complete before the retained portable Reset may mutate bootstrap state.
//! HCI publication then retains the already-idle task across endpoint mismatch,
//! backpressure and transport failure.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerResetBarrier,
    LeControllerResponsePending, LeControllerResponsePublication,
};

use crate::le::dtm::active::session::BluetoothDtmActiveRadio;
use crate::le::dtm::quiescence::{
    BluetoothDtmQuiescenceFault, BluetoothDtmQuiescenceFaultCause,
    BluetoothDtmQuiescenceRetryCause, BluetoothDtmQuiescenceRunner, BluetoothDtmQuiescenceStep,
    BluetoothDtmQuiescenceWait,
};
use crate::le::dtm::reset_order::{BluetoothDtmRestoredReset, BluetoothDtmRestoredResetCompletion};
use crate::{
    BluetoothControllerIdleCommandTask, BluetoothControllerPublishedTaskService,
    BluetoothDtmActiveCompletionFaultCause, BluetoothDtmActiveCpuOwned,
    BluetoothDtmPostUnlinkWakeCell, BluetoothDtmRecurringFaultCause, BluetoothDtmSessionIdle,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerWakeCell,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

/// Accepted Reset and exact active graph advancing through neutral quiescence.
#[must_use = "advance, wait, retry or retain the complete Reset transaction"]
pub struct BluetoothDtmResetStoppingRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    barrier: LeControllerResetBarrier<'runtime, ()>,
    quiescence: BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>,
}

/// Borrowed wait source for the current Reset-quiescence phase.
#[derive(Clone, Copy)]
pub enum BluetoothDtmResetStoppingWait<'runner> {
    Scheduler(&'runner BluetoothSchedulerWakeCell),
    PostUnlink(&'runner BluetoothDtmPostUnlinkWakeCell),
    ControllerTime,
}

/// Why Reset quiescence retained its exact owner for an explicit retry.
pub enum BluetoothDtmResetStoppingRetryCause<'cause, E> {
    CancellationRejected,
    SchedulerStart(&'cause E),
}

/// Read-only fail-stop classification for Reset quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmResetStoppingFaultCause {
    Completion(BluetoothDtmActiveCompletionFaultCause),
    Recurring(BluetoothDtmRecurringFaultCause),
    UnexpectedPublishedHeadTransition,
}

/// Opaque fail-stop owner retaining Reset/order and the exact active graph.
#[must_use = "retain the complete Reset quiescence owner for diagnostic shutdown"]
pub struct BluetoothDtmResetStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothDtmResetStoppingFaultCause,
    _barrier: LeControllerResetBarrier<'runtime, ()>,
    _quiescence: BluetoothDtmQuiescenceFault<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothDtmResetStoppingFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothDtmResetStoppingFaultCause {
        self.cause
    }
}

/// One bounded Reset-quiescence transition.
#[must_use = "retain every affine owner until idle restore or fail-stop"]
pub enum BluetoothDtmResetStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>),
    Waiting(BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>),
    UnrelatedList {
        runner: BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Retryable(BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>),
    /// Radio is quiescent and the exact graph is restored in the task idle slot.
    CompletionReady(BluetoothDtmResetCompletionReady<'runtime, S, CAPACITY>),
    /// Radio is quiescent, but the exact graph has not yet re-entered its idle slot.
    RestoreFailed(BluetoothDtmResetRestoreFailure<'runtime, S, CAPACITY>),
    Fault(BluetoothDtmResetStoppingFault<'runtime, S, CAPACITY>),
}

/// Quiesced Reset owner after successful graph restore and before bootstrap mutation.
#[must_use = "complete Reset through the matching combined Controller endpoint"]
pub struct BluetoothDtmResetCompletionReady<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    restored: BluetoothDtmRestoredReset<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Quiesced Reset owner whose exact graph was rejected by the runtime idle slot.
#[must_use = "retry idle restore without applying or publishing Reset"]
pub struct BluetoothDtmResetRestoreFailure<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    idle: BluetoothDtmSessionIdle,
    barrier: LeControllerResetBarrier<'runtime, ()>,
}

/// Result of retrying only the pre-Reset idle graph restore.
#[must_use = "retain either the completion-ready owner or unchanged restore failure"]
pub enum BluetoothDtmResetRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    CompletionReady(BluetoothDtmResetCompletionReady<'runtime, S, CAPACITY>),
    Rejected(BluetoothDtmResetRestoreFailure<'runtime, S, CAPACITY>),
}

/// Result of applying a quiesced Reset through one combined Controller endpoint.
#[must_use = "publish the response or retain the exact endpoint mismatch"]
pub enum BluetoothDtmResetCompletionStart<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothDtmResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothDtmResetCompletionReady<'runtime, S, CAPACITY>),
}

/// Applied Reset whose Command Complete retains the already-idle task.
#[must_use = "publish the exact Reset response or retain the complete owner"]
pub struct BluetoothDtmResetResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Result of one consuming Reset response publication attempt.
#[must_use = "retain backpressure, mismatch, fault or the completed idle task"]
pub enum BluetoothDtmResetResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completed(BluetoothDtmResetComplete<'runtime, S, CAPACITY>),
    Pending(BluetoothDtmResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothDtmResetResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothDtmResetResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Sole Controller task after Reset response publication with DTM already idle.
#[must_use = "return the task owner to the sole Controller session loop"]
pub struct BluetoothDtmResetComplete<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmResetComplete<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn into_idle_command_task(
        self,
    ) -> BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY> {
        self.task
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn new(
        radio: BluetoothDtmActiveRadio<'runtime, S, CAPACITY>,
        barrier: LeControllerResetBarrier<'runtime, ()>,
    ) -> Self {
        Self {
            barrier,
            quiescence: BluetoothDtmQuiescenceRunner::new(radio),
        }
    }

    pub fn wait(&self) -> Option<BluetoothDtmResetStoppingWait<'_>> {
        match self.quiescence.wait() {
            Some(BluetoothDtmQuiescenceWait::Scheduler(wake)) => {
                Some(BluetoothDtmResetStoppingWait::Scheduler(wake))
            }
            Some(BluetoothDtmQuiescenceWait::PostUnlink(wake)) => {
                Some(BluetoothDtmResetStoppingWait::PostUnlink(wake))
            }
            Some(BluetoothDtmQuiescenceWait::ControllerTime) => {
                Some(BluetoothDtmResetStoppingWait::ControllerTime)
            }
            None => None,
        }
    }

    pub fn retry_cause(&self) -> Option<BluetoothDtmResetStoppingRetryCause<'_, S::Error>> {
        match self.quiescence.retry_cause() {
            Some(BluetoothDtmQuiescenceRetryCause::CancellationRejected) => {
                Some(BluetoothDtmResetStoppingRetryCause::CancellationRejected)
            }
            Some(BluetoothDtmQuiescenceRetryCause::SchedulerStart(error)) => {
                Some(BluetoothDtmResetStoppingRetryCause::SchedulerStart(error))
            }
            None => None,
        }
    }

    pub fn step(self) -> BluetoothDtmResetStoppingStep<'runtime, S, CAPACITY> {
        let Self {
            barrier,
            quiescence,
        } = self;
        match quiescence.step() {
            BluetoothDtmQuiescenceStep::Continue(quiescence) => {
                BluetoothDtmResetStoppingStep::Continue(Self {
                    barrier,
                    quiescence,
                })
            }
            BluetoothDtmQuiescenceStep::Waiting(quiescence) => {
                BluetoothDtmResetStoppingStep::Waiting(Self {
                    barrier,
                    quiescence,
                })
            }
            BluetoothDtmQuiescenceStep::UnrelatedList {
                runner: quiescence,
                observed,
            } => BluetoothDtmResetStoppingStep::UnrelatedList {
                runner: Self {
                    barrier,
                    quiescence,
                },
                observed,
            },
            BluetoothDtmQuiescenceStep::Retryable(quiescence) => {
                BluetoothDtmResetStoppingStep::Retryable(Self {
                    barrier,
                    quiescence,
                })
            }
            BluetoothDtmQuiescenceStep::CpuOwned(owner) => restore_quiesced(barrier, owner),
            BluetoothDtmQuiescenceStep::Fault(quiescence) => {
                BluetoothDtmResetStoppingStep::Fault(BluetoothDtmResetStoppingFault {
                    cause: reset_fault_cause(quiescence.cause()),
                    _barrier: barrier,
                    _quiescence: quiescence,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmResetRestoreFailure<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn retry_restore(mut self) -> BluetoothDtmResetRestoreStep<'runtime, S, CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => {
                BluetoothDtmResetRestoreStep::CompletionReady(BluetoothDtmResetCompletionReady {
                    restored: BluetoothDtmRestoredReset::new(
                        self.barrier.map_owner(|()| self.task),
                    ),
                })
            }
            Err(idle) => {
                self.idle = idle;
                BluetoothDtmResetRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmResetCompletionReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    ) -> BluetoothDtmResetCompletionStart<'runtime, S, CAPACITY> {
        match self.restored.complete(controller) {
            BluetoothDtmRestoredResetCompletion::ResponsePending(transaction) => {
                BluetoothDtmResetCompletionStart::ResponsePending(
                    BluetoothDtmResetResponsePending { transaction },
                )
            }
            BluetoothDtmRestoredResetCompletion::EndpointMismatch(restored) => {
                BluetoothDtmResetCompletionStart::EndpointMismatch(Self { restored })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmResetResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
    ) -> BluetoothDtmResetResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(published) => {
                BluetoothDtmResetResponsePublication::Completed(BluetoothDtmResetComplete {
                    task: BluetoothControllerIdleCommandTask::from_ready(published),
                })
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothDtmResetResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothDtmResetResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothDtmResetResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

fn restore_quiesced<'runtime, S, const CAPACITY: usize>(
    barrier: LeControllerResetBarrier<'runtime, ()>,
    owner: BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>,
) -> BluetoothDtmResetStoppingStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (mut task, quiesced) = match owner {
        BluetoothDtmActiveCpuOwned::Transmitter(ready) => {
            let (task, owner) = ready.into_parts();
            (task, owner.into_quiesced())
        }
        BluetoothDtmActiveCpuOwned::Receiver(ready) => {
            let (task, owner, _status, _outcome) = ready.into_parts();
            (task, owner.into_quiesced())
        }
    };
    let idle = BluetoothDtmSessionIdle::from_quiesced(quiesced);
    match task.restore_dtm_session_idle(idle) {
        Ok(()) => {
            BluetoothDtmResetStoppingStep::CompletionReady(BluetoothDtmResetCompletionReady {
                restored: BluetoothDtmRestoredReset::new(barrier.map_owner(|()| task)),
            })
        }
        Err(idle) => {
            BluetoothDtmResetStoppingStep::RestoreFailed(BluetoothDtmResetRestoreFailure {
                task,
                idle,
                barrier,
            })
        }
    }
}

const fn reset_fault_cause(
    cause: BluetoothDtmQuiescenceFaultCause,
) -> BluetoothDtmResetStoppingFaultCause {
    match cause {
        BluetoothDtmQuiescenceFaultCause::Completion(cause) => {
            BluetoothDtmResetStoppingFaultCause::Completion(cause)
        }
        BluetoothDtmQuiescenceFaultCause::Recurring(cause) => {
            BluetoothDtmResetStoppingFaultCause::Recurring(cause)
        }
        BluetoothDtmQuiescenceFaultCause::UnexpectedPublishedHeadTransition => {
            BluetoothDtmResetStoppingFaultCause::UnexpectedPublishedHeadTransition
        }
    }
}
