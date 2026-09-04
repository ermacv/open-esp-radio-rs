//! Bounded successor scheduling for active legacy advertising.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciActiveLegacyAdvertisingCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerResponsePending, LeControllerResponsePublication,
};
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;

use crate::controller_start::{
    BluetoothLegacyAdvertisingRecurringCandidateFailure,
    BluetoothLegacyAdvertisingRecurringSequenceCompletion,
};
use crate::legacy_advertising_active::BluetoothLegacyAdvertisingStopOrder;
use crate::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginError,
    BluetoothControllerSchedulerCurrentError, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothLegacyAdvertisingActiveResponsePending,
    BluetoothLegacyAdvertisingActiveSession, BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingEmptySchedulerMergePrepared, BluetoothLegacyAdvertisingEventCpuOwned,
    BluetoothLegacyAdvertisingEventPrepared, BluetoothLegacyAdvertisingNextEventScheduled,
    BluetoothLegacyAdvertisingRecurringEventCandidate,
    BluetoothLegacyAdvertisingRecurringEventPreparationError,
    BluetoothLegacyAdvertisingRecurringPreSequence,
    BluetoothLegacyAdvertisingRecurringPreparationError,
    BluetoothLegacyAdvertisingRecurringPreparationFailure,
    BluetoothLegacyAdvertisingResetCompletionReady,
    BluetoothLegacyAdvertisingSchedulerHeadPublished, BluetoothLegacyAdvertisingStopping,
    BluetoothSchedulerEmptyListMergeError, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerRunInterruptStorage,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;
type Order<'runtime> = LeControllerCommandReady<'runtime, ()>;

enum BluetoothLegacyAdvertisingRecurringOrder<'runtime> {
    Ready(Order<'runtime>),
    ResponsePending(LeControllerResponsePending<'runtime, ()>),
    Stopping(BluetoothLegacyAdvertisingStopOrder<'runtime>),
    Detached,
}

struct BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, const CAPACITY: usize> {
    task: Option<Task<'runtime, S, CAPACITY>>,
    order: BluetoothLegacyAdvertisingRecurringOrder<'runtime>,
    previous_scheduler_item_address: BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

enum BluetoothLegacyAdvertisingRecurringPhase<'runtime, S, const CAPACITY: usize> {
    Scheduled(BluetoothLegacyAdvertisingNextEventScheduled<'static>),
    CandidatePreparationFailure(BluetoothLegacyAdvertisingRecurringPreparationFailure<'static>),
    Candidate(BluetoothLegacyAdvertisingRecurringEventCandidate<'static>),
    SequenceBegin(BluetoothLegacyAdvertisingRecurringPreSequence<'static>),
    SequenceWait {
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, CAPACITY>,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    },
    Merge(BluetoothLegacyAdvertisingEventPrepared<'static>),
    Merged(BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>),
    Head(BluetoothLegacyAdvertisingSchedulerHeadPublished<'static>),
}

/// One successor from a completed event through the next scheduler `RUN`.
#[must_use = "drive, retry, or retain the exact recurring advertising owner"]
pub struct BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    axes: Option<BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>>,
    phase: BluetoothLegacyAdvertisingRecurringPhase<'runtime, S, CAPACITY>,
}

/// Result of attaching the caller's fresh advertising delay.
#[must_use = "retain the recurring runner or sequence-exhausted CPU owner"]
pub enum BluetoothLegacyAdvertisingRecurringStart<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Runner(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    SequenceExhausted(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
}

/// One finite recurring transition.
#[must_use = "retain the runner, wait, running graph, retry, or fail-stop owner"]
pub enum BluetoothLegacyAdvertisingRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    WaitControllerTime(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Running(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    RunningResponsePending(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    RunningStopping(BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    Retryable(BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// HCI order currently retained beside a recurring radio graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRecurringOrderState {
    CommandReady,
    ResponsePending,
    Stopping,
}

/// One readiness observation for the recurring HCI order axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRecurringOrderProgress {
    Command,
    Response,
}

/// Opaque owner for an impossible endpoint mismatch after recurring intake.
#[must_use = "retain the complete command, recurring radio graph and order"]
pub struct BluetoothLegacyAdvertisingRecurringCommandMismatch<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
    >,
}

/// Typed route for a command accepted while preparing the next event.
#[must_use = "continue, publish, stop, or retain the exact mismatch owner"]
pub enum BluetoothLegacyAdvertisingRecurringCommandRoute<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyAdvertisingRecurringCommandMismatch<'runtime, 'command, S, CAPACITY>,
    ),
}

/// One non-blocking command intake while a successor event is being prepared.
#[must_use = "route a command or retain the exact recurring runner"]
pub enum BluetoothLegacyAdvertisingRecurringCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Routed {
        route: BluetoothLegacyAdvertisingRecurringCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// Result of publishing a recurring response without pausing radio progress.
#[must_use = "retain the recurring runner and exact HCI response order"]
pub enum BluetoothLegacyAdvertisingRecurringResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Pending(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Fault {
        runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Entry selected by a retained Disable or Reset before the successor `RUN`.
#[must_use = "restore the cancelled graph or finish the already-published successor"]
pub enum BluetoothLegacyAdvertisingRecurringStopBegin<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Restore(BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>),
    Published(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// Cancelled unpublished successor retained until runtime restore is accepted.
#[must_use = "drain Controller time if required and restore the exact graph"]
pub struct BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    cancelled: crate::BluetoothLegacyAdvertisingCancelled<'static>,
    order: BluetoothLegacyAdvertisingStopOrder<'runtime>,
    controller_time_drain_required: bool,
}

/// One exact unpublished-successor stop/restore transition.
#[must_use = "retain the wait, response order, rejected restore, or fail-stop owner"]
pub enum BluetoothLegacyAdvertisingRecurringStopRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>),
    DisableResponse(BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    ResetCompletion(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
    Rejected(BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingRecurringStopFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner for an abandoned Controller-time drain during recurrence cancellation.
#[must_use = "retain every cancelled radio and HCI owner for shutdown diagnostics"]
pub struct BluetoothLegacyAdvertisingRecurringStopFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _task: Task<'runtime, S, CAPACITY>,
    _cancelled: crate::BluetoothLegacyAdvertisingCancelled<'static>,
    _order: BluetoothLegacyAdvertisingStopOrder<'runtime>,
    error: BluetoothControllerSchedulerCurrentError,
}

impl<S, const CAPACITY: usize> BluetoothLegacyAdvertisingRecurringStopFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn error(&self) -> BluetoothControllerSchedulerCurrentError {
        self.error
    }
}

/// Finite reason a lossless recurring phase asks its supervisor to retry.
#[derive(Debug)]
pub enum BluetoothLegacyAdvertisingRecurringRetryCause<E> {
    Preparation(BluetoothLegacyAdvertisingRecurringPreparationError),
    Event(BluetoothLegacyAdvertisingRecurringEventPreparationError),
    ControllerTimeBegin(BluetoothControllerSchedulerCurrentBeginError),
    EmptyList(BluetoothSchedulerEmptyListMergeError),
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    SchedulerStart(E),
}

/// Exact retryable recurring phase.
#[must_use = "inspect and retry the unchanged recurring runner"]
pub struct BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyAdvertisingRecurringRetryCause<S::Error>,
    runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &BluetoothLegacyAdvertisingRecurringRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }
}

/// Non-retryable recurring ownership failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRecurringFaultCause {
    SchedulerEpochUnavailable,
    ControllerTime(BluetoothControllerSchedulerCurrentError),
    SchedulerMergeCancellationRejected,
}

#[allow(
    dead_code,
    reason = "the fail-stop owner intentionally retains every recurrence input"
)]
enum BluetoothLegacyAdvertisingRecurringFaultOwner<'runtime, S, const CAPACITY: usize> {
    Scheduled {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        scheduled: BluetoothLegacyAdvertisingNextEventScheduled<'static>,
    },
    SequenceBegin {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    },
    SequenceRecheck {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    },
    Merged {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        merged: BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    },
}

/// Opaque fail-stop owner for a recurring event.
#[must_use = "retain the exact failed recurrence for diagnostic shutdown"]
pub struct BluetoothLegacyAdvertisingRecurringFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyAdvertisingRecurringFaultCause,
    _owner: BluetoothLegacyAdvertisingRecurringFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothLegacyAdvertisingRecurringFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyAdvertisingRecurringFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Attach one source-owned random delay without hiding entropy policy.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> BluetoothLegacyAdvertisingRecurringStart<'runtime, S, CAPACITY> {
        let (task, order, address, index, completed) = self.into_parts();
        match completed.schedule_next(delay) {
            Ok(scheduled) => BluetoothLegacyAdvertisingRecurringStart::Runner(
                BluetoothLegacyAdvertisingRecurringRunner {
                    axes: Some(BluetoothLegacyAdvertisingRecurringAxes {
                        task: Some(task),
                        order: BluetoothLegacyAdvertisingRecurringOrder::Ready(order),
                        previous_scheduler_item_address: address,
                        hardware_list_index: index,
                    }),
                    phase: BluetoothLegacyAdvertisingRecurringPhase::Scheduled(scheduled),
                },
            ),
            Err(failure) => BluetoothLegacyAdvertisingRecurringStart::SequenceExhausted(
                BluetoothLegacyAdvertisingEventCpuOwned::from_parts(
                    task,
                    order,
                    address,
                    index,
                    failure.into_completed(),
                ),
            ),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_parts(
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        phase: BluetoothLegacyAdvertisingRecurringPhase<'runtime, S, CAPACITY>,
    ) -> Self {
        Self {
            axes: Some(axes),
            phase,
        }
    }

    fn retryable(
        self,
        cause: BluetoothLegacyAdvertisingRecurringRetryCause<S::Error>,
    ) -> BluetoothLegacyAdvertisingRecurringRunnerStep<'runtime, S, CAPACITY> {
        BluetoothLegacyAdvertisingRecurringRunnerStep::Retryable(
            BluetoothLegacyAdvertisingRecurringRetry {
                cause,
                runner: self,
            },
        )
    }

    /// Current independently progressing HCI order beside the radio graph.
    pub fn order_state(&self) -> BluetoothLegacyAdvertisingRecurringOrderState {
        let axes = self
            .axes
            .as_ref()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            BluetoothLegacyAdvertisingRecurringOrder::Ready(_) => {
                BluetoothLegacyAdvertisingRecurringOrderState::CommandReady
            }
            BluetoothLegacyAdvertisingRecurringOrder::ResponsePending(_) => {
                BluetoothLegacyAdvertisingRecurringOrderState::ResponsePending
            }
            BluetoothLegacyAdvertisingRecurringOrder::Stopping(_) => {
                BluetoothLegacyAdvertisingRecurringOrderState::Stopping
            }
            BluetoothLegacyAdvertisingRecurringOrder::Detached => {
                unreachable!("a stored recurring runner cannot have detached HCI order")
            }
        }
    }

    /// Wait for the currently attached command or response order to progress.
    pub async fn wait_order_progress<
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
    ) -> Result<
        BluetoothLegacyAdvertisingRecurringOrderProgress,
        open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch,
    > {
        let axes = self
            .axes
            .as_ref()
            .expect("a recurring runner retains its exact Controller axes");
        match &axes.order {
            BluetoothLegacyAdvertisingRecurringOrder::Ready(order) => {
                controller.wait_command_available(order).await?;
                Ok(BluetoothLegacyAdvertisingRecurringOrderProgress::Command)
            }
            BluetoothLegacyAdvertisingRecurringOrder::ResponsePending(response) => {
                controller.wait_response_capacity(response).await?;
                Ok(BluetoothLegacyAdvertisingRecurringOrderProgress::Response)
            }
            BluetoothLegacyAdvertisingRecurringOrder::Stopping(_) => {
                unreachable!("a stopping recurrence has no independently progressing HCI wait")
            }
            BluetoothLegacyAdvertisingRecurringOrder::Detached => {
                unreachable!("a stored recurring runner cannot have detached HCI order")
            }
        }
    }

    fn detach_ready_order(mut self) -> (Self, Order<'runtime>) {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        let order = core::mem::replace(
            &mut axes.order,
            BluetoothLegacyAdvertisingRecurringOrder::Detached,
        );
        match order {
            BluetoothLegacyAdvertisingRecurringOrder::Ready(order) => (self, order),
            _ => unreachable!("only a command-ready recurrence can accept another command"),
        }
    }

    fn attach_ready_order(mut self, order: Order<'runtime>) -> Self {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            BluetoothLegacyAdvertisingRecurringOrder::Detached => {
                axes.order = BluetoothLegacyAdvertisingRecurringOrder::Ready(order);
                self
            }
            _ => unreachable!("a recurring runner cannot acquire a second HCI order"),
        }
    }

    fn attach_response(mut self, response: LeControllerResponsePending<'runtime, ()>) -> Self {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            BluetoothLegacyAdvertisingRecurringOrder::Detached => {
                axes.order = BluetoothLegacyAdvertisingRecurringOrder::ResponsePending(response);
                self
            }
            _ => unreachable!("a recurring runner cannot acquire a second HCI order"),
        }
    }

    fn attach_stopping(mut self, order: BluetoothLegacyAdvertisingStopOrder<'runtime>) -> Self {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            BluetoothLegacyAdvertisingRecurringOrder::Detached => {
                axes.order = BluetoothLegacyAdvertisingRecurringOrder::Stopping(order);
                self
            }
            _ => unreachable!("a recurring runner cannot acquire a second HCI order"),
        }
    }

    /// Consume and route at most one command without advancing recurrence.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            'command,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        buffer: &'buffer mut [u8],
    ) -> BluetoothLegacyAdvertisingRecurringCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY>
    {
        let (runner, order) = self.detach_ready_order();
        let ready = order.map_owner(|()| runner);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_advertising_classified_command(command) {
                        HciActiveLegacyAdvertisingCommandRoute::ResponsePending(response) => {
                            let (runner, response) = response.into_parts();
                            BluetoothLegacyAdvertisingRecurringCommandRoute::Continue(
                                runner.attach_response(response),
                            )
                        }
                        HciActiveLegacyAdvertisingCommandRoute::Disable(deferred) => {
                            let (runner, deferred) = deferred.into_parts();
                            BluetoothLegacyAdvertisingRecurringCommandRoute::Continue(
                                runner.attach_stopping(
                                    BluetoothLegacyAdvertisingStopOrder::Disable(deferred),
                                ),
                            )
                        }
                        HciActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) => {
                            let (runner, barrier) = barrier.into_parts();
                            BluetoothLegacyAdvertisingRecurringCommandRoute::Continue(
                                runner.attach_stopping(BluetoothLegacyAdvertisingStopOrder::Reset(
                                    barrier,
                                )),
                            )
                        }
                        HciActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command) => {
                            BluetoothLegacyAdvertisingRecurringCommandRoute::EndpointMismatch(
                                BluetoothLegacyAdvertisingRecurringCommandMismatch {
                                    _command: command,
                                },
                            )
                        }
                    };
                BluetoothLegacyAdvertisingRecurringCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (runner, order) = ready.into_parts();
                BluetoothLegacyAdvertisingRecurringCommandIntake::Empty {
                    runner: runner.attach_ready_order(order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (runner, order) = ready.into_parts();
                BluetoothLegacyAdvertisingRecurringCommandIntake::EndpointMismatch {
                    runner: runner.attach_ready_order(order),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => {
                let (runner, order) = ready.into_parts();
                BluetoothLegacyAdvertisingRecurringCommandIntake::Channel {
                    runner: runner.attach_ready_order(order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (runner, order) = ready.into_parts();
                BluetoothLegacyAdvertisingRecurringCommandIntake::NonCommand {
                    runner: runner.attach_ready_order(order),
                    frame,
                }
            }
        }
    }

    /// Publish a pending command response while retaining the recurrence.
    pub fn try_publish_response<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        mut self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothLegacyAdvertisingRecurringResponsePublication<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        let response = core::mem::replace(
            &mut axes.order,
            BluetoothLegacyAdvertisingRecurringOrder::Detached,
        );
        let BluetoothLegacyAdvertisingRecurringOrder::ResponsePending(response) = response else {
            unreachable!("only a response-pending recurrence can publish a response")
        };
        match response.map_owner(|()| self).try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (runner, order) = ready.into_parts();
                BluetoothLegacyAdvertisingRecurringResponsePublication::Published(
                    runner.attach_ready_order(order),
                )
            }
            LeControllerResponsePublication::Pending(response) => {
                let (runner, response) = response.into_parts();
                BluetoothLegacyAdvertisingRecurringResponsePublication::Pending(
                    runner.attach_response(response),
                )
            }
            LeControllerResponsePublication::EndpointMismatch(response) => {
                let (runner, response) = response.into_parts();
                BluetoothLegacyAdvertisingRecurringResponsePublication::EndpointMismatch(
                    runner.attach_response(response),
                )
            }
            LeControllerResponsePublication::Fault {
                pending: response,
                error,
            } => {
                let (runner, response) = response.into_parts();
                BluetoothLegacyAdvertisingRecurringResponsePublication::Fault {
                    runner: runner.attach_response(response),
                    error,
                }
            }
        }
    }

    fn stop_restore(
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        cancelled: crate::BluetoothLegacyAdvertisingCancelled<'static>,
        controller_time_drain_required: bool,
    ) -> BluetoothLegacyAdvertisingRecurringStopBegin<'runtime, S, CAPACITY> {
        let BluetoothLegacyAdvertisingRecurringAxes {
            task,
            order,
            previous_scheduler_item_address: _,
            hardware_list_index: _,
        } = axes;
        let BluetoothLegacyAdvertisingRecurringOrder::Stopping(order) = order else {
            unreachable!("only a retained stop order can cancel a recurrence")
        };
        BluetoothLegacyAdvertisingRecurringStopBegin::Restore(
            BluetoothLegacyAdvertisingRecurringStopRestore {
                task: task.expect("a cancelled recurrence retains its task service"),
                cancelled,
                order,
                controller_time_drain_required,
            },
        )
    }

    /// Cancel every unpublished successor phase; a published HEAD must finish once.
    pub fn begin_stopping(
        mut self,
    ) -> BluetoothLegacyAdvertisingRecurringStopBegin<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .take()
            .expect("a recurring runner retains its exact Controller axes");
        assert!(matches!(
            axes.order,
            BluetoothLegacyAdvertisingRecurringOrder::Stopping(_)
        ));
        match self.phase {
            BluetoothLegacyAdvertisingRecurringPhase::Scheduled(scheduled) => {
                Self::stop_restore(axes, scheduled.cancel(), false)
            }
            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure) => {
                Self::stop_restore(axes, failure.cancel(), false)
            }
            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate) => {
                Self::stop_restore(axes, candidate.cancel().into_parts().0, false)
            }
            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted) => {
                let mut axes = axes;
                let cancelled = axes
                    .task
                    .as_mut()
                    .expect("an admitted recurrence retains its task service")
                    .cancel_legacy_advertising_recurring_pre_sequence(admitted);
                Self::stop_restore(axes, cancelled, false)
            }
            BluetoothLegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted } => {
                let BluetoothLegacyAdvertisingRecurringAxes {
                    task: _,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                match pending.cancel() {
                    Ok(epoch) => {
                        let mut task = epoch.into_task_service();
                        let cancelled =
                            task.cancel_legacy_advertising_recurring_pre_sequence(admitted);
                        Self::stop_restore(
                            BluetoothLegacyAdvertisingRecurringAxes {
                                task: Some(task),
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            cancelled,
                            true,
                        )
                    }
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringFaultCause::ControllerTime(
                            failure.error(),
                        );
                        let task = failure.into_parts().0.into_task_service();
                        BluetoothLegacyAdvertisingRecurringStopBegin::Fault(
                            BluetoothLegacyAdvertisingRecurringFault {
                                cause,
                                _owner:
                                    BluetoothLegacyAdvertisingRecurringFaultOwner::SequenceRecheck {
                                        axes: BluetoothLegacyAdvertisingRecurringAxes {
                                            task: Some(task),
                                            order,
                                            previous_scheduler_item_address,
                                            hardware_list_index,
                                        },
                                        admitted,
                                    },
                            },
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Merge(prepared) => {
                let mut axes = axes;
                let cancelled = axes
                    .task
                    .as_mut()
                    .expect("a prepared recurrence retains its task service")
                    .cancel_legacy_advertising_recurring_prepared(prepared);
                Self::stop_restore(axes, cancelled, false)
            }
            BluetoothLegacyAdvertisingRecurringPhase::Merged(merged) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merged recurrence retains its task service")
                    .cancel_legacy_advertising_recurring_merge(merged)
                {
                    Ok(cancelled) => Self::stop_restore(axes, cancelled, false),
                    Err(merged) => BluetoothLegacyAdvertisingRecurringStopBegin::Fault(
                        BluetoothLegacyAdvertisingRecurringFault {
                            cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerMergeCancellationRejected,
                            _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::Merged {
                                axes,
                                merged,
                            },
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Head(head) => {
                BluetoothLegacyAdvertisingRecurringStopBegin::Published(Self::from_parts(
                    axes,
                    BluetoothLegacyAdvertisingRecurringPhase::Head(head),
                ))
            }
        }
    }

    /// Execute exactly one recurrence edge.
    pub fn step(mut self) -> BluetoothLegacyAdvertisingRecurringRunnerStep<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .take()
            .expect("a recurring runner retains its exact Controller axes");
        match self.phase {
            BluetoothLegacyAdvertisingRecurringPhase::Scheduled(scheduled) => {
                match axes
                    .task
                    .as_ref()
                    .expect("a scheduled recurrence retains its task service")
                    .prepare_legacy_advertising_recurring_candidate(scheduled)
                {
                    Ok(candidate) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate),
                        ),
                    ),
                    Err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation(
                        failure,
                    )) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Preparation(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(
                                failure,
                            ),
                        )
                        .retryable(cause)
                    }
                    Err(
                        BluetoothLegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                            scheduled,
                        ),
                    ) => BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                        BluetoothLegacyAdvertisingRecurringFault {
                            cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::Scheduled {
                                axes,
                                scheduled,
                            },
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure) => {
                match axes
                    .task
                    .as_ref()
                    .expect("a preparation retry retains its task service")
                    .retry_legacy_advertising_recurring_candidate(failure)
                {
                    Ok(candidate) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate),
                        ),
                    ),
                    Err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation(
                        failure,
                    )) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Preparation(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(
                                failure,
                            ),
                        )
                        .retryable(cause)
                    }
                    Err(
                        BluetoothLegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                            scheduled,
                        ),
                    ) => BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                        BluetoothLegacyAdvertisingRecurringFault {
                            cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::Scheduled {
                                axes,
                                scheduled,
                            },
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a candidate recurrence retains its task service")
                    .admit_legacy_advertising_recurring_candidate(candidate)
                {
                    Ok(admitted) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted),
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Event(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Candidate(
                                failure.into_candidate(),
                            ),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted) => {
                let BluetoothLegacyAdvertisingRecurringAxes {
                    task,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                let task = task.expect("a sequence-begin recurrence retains its task service");
                let epoch = match task.retain_scheduler_epoch() {
                    Ok(epoch) => epoch,
                    Err(unavailable) => {
                        return BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                            BluetoothLegacyAdvertisingRecurringFault {
                                cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                                _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::SequenceBegin {
                                    axes: BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(unavailable.into_task_service()),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    admitted,
                                },
                            },
                        );
                    }
                };
                match epoch.begin_fresh_scheduler_current() {
                    Ok(pending) => BluetoothLegacyAdvertisingRecurringRunnerStep::WaitControllerTime(
                        Self::from_parts(
                            BluetoothLegacyAdvertisingRecurringAxes {
                                task: None,
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            BluetoothLegacyAdvertisingRecurringPhase::SequenceWait {
                                pending,
                                admitted,
                            },
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::ControllerTimeBegin(
                            failure.error(),
                        );
                        let task = failure.into_parts().0.into_task_service();
                        Self::from_parts(
                            BluetoothLegacyAdvertisingRecurringAxes {
                                task: Some(task),
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted } => {
                let BluetoothLegacyAdvertisingRecurringAxes {
                    task: _,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothLegacyAdvertisingRecurringRunnerStep::WaitControllerTime(
                            Self::from_parts(
                                BluetoothLegacyAdvertisingRecurringAxes {
                                    task: None,
                                    order,
                                    previous_scheduler_item_address,
                                    hardware_list_index,
                                },
                                BluetoothLegacyAdvertisingRecurringPhase::SequenceWait {
                                    pending,
                                    admitted,
                                },
                            ),
                        )
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        match current.finish_legacy_advertising_recurring_event(admitted) {
                            BluetoothLegacyAdvertisingRecurringSequenceCompletion::Prepared {
                                task,
                                merged,
                            } => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                                Self::from_parts(
                                    BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    BluetoothLegacyAdvertisingRecurringPhase::Merged(merged),
                                ),
                            ),
                            BluetoothLegacyAdvertisingRecurringSequenceCompletion::EventRejected {
                                task,
                                failure,
                            } => {
                                let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Event(
                                    failure.error(),
                                );
                                Self::from_parts(
                                    BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    BluetoothLegacyAdvertisingRecurringPhase::Candidate(
                                        failure.into_candidate(),
                                    ),
                                )
                                .retryable(cause)
                            }
                            BluetoothLegacyAdvertisingRecurringSequenceCompletion::EmptyListRejected {
                                task,
                                failure,
                            } => {
                                let cause = BluetoothLegacyAdvertisingRecurringRetryCause::EmptyList(
                                    failure.error(),
                                );
                                Self::from_parts(
                                    BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    BluetoothLegacyAdvertisingRecurringPhase::Merge(
                                        failure.into_prepared(),
                                    ),
                                )
                                .retryable(cause)
                            }
                        }
                    }
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringFaultCause::ControllerTime(
                            failure.error(),
                        );
                        let task = failure.into_parts().0.into_task_service();
                        BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                            BluetoothLegacyAdvertisingRecurringFault {
                                cause,
                                _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::SequenceRecheck {
                                    axes: BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    admitted,
                                },
                            },
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Merge(prepared) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merge retry retains its task service")
                    .merge_legacy_advertising_recurring_event(prepared)
                {
                    Ok(merged) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Merged(merged),
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::EmptyList(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Merge(
                                failure.into_prepared(),
                            ),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Merged(merged) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merged recurrence retains its task service")
                    .publish_legacy_advertising_scheduler_head(merged)
                {
                    Ok(head) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Head(head),
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::HeadPublication(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Merged(
                                failure.into_merged(),
                            ),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Head(head) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a published recurrence retains its task service")
                    .start_legacy_advertising_scheduler(head)
                {
                    Ok(running) => {
                        let task = axes
                            .task
                            .take()
                            .expect("the running recurrence consumes its task service");
                        match axes.order {
                            BluetoothLegacyAdvertisingRecurringOrder::Ready(order) => {
                                BluetoothLegacyAdvertisingRecurringRunnerStep::Running(
                                    BluetoothLegacyAdvertisingActiveSession::from_recurring_running(
                                        task, order, running,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingRecurringOrder::ResponsePending(response) => {
                                BluetoothLegacyAdvertisingRecurringRunnerStep::RunningResponsePending(
                                    BluetoothLegacyAdvertisingActiveSession::from_recurring_response_pending(
                                        task, response, running,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingRecurringOrder::Stopping(order) => {
                                BluetoothLegacyAdvertisingRecurringRunnerStep::RunningStopping(
                                    BluetoothLegacyAdvertisingActiveSession::from_recurring_stopping(
                                        task, order, running,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingRecurringOrder::Detached => {
                                unreachable!("a running recurrence cannot have detached HCI order")
                            }
                        }
                    }
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Head(head),
                        )
                        .retryable(
                            BluetoothLegacyAdvertisingRecurringRetryCause::SchedulerStart(error),
                        )
                    }
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Whether the executor must await the absolute Controller-time recheck source.
    pub const fn controller_time_drain_required(&self) -> bool {
        self.controller_time_drain_required
    }

    /// Drain an abandoned time request, restore the graph, then expose HCI completion.
    pub fn step(
        mut self,
    ) -> BluetoothLegacyAdvertisingRecurringStopRestoreStep<'runtime, S, CAPACITY> {
        if self.controller_time_drain_required {
            match self.task.drain_abandoned_recurring_controller_time() {
                Ok(crate::BluetoothControllerTimeOrphanDrainStep::Waiting) => {
                    return BluetoothLegacyAdvertisingRecurringStopRestoreStep::WaitControllerTime(
                        self,
                    );
                }
                Ok(
                    crate::BluetoothControllerTimeOrphanDrainStep::Idle
                    | crate::BluetoothControllerTimeOrphanDrainStep::Drained,
                ) => self.controller_time_drain_required = false,
                Err(error) => {
                    return BluetoothLegacyAdvertisingRecurringStopRestoreStep::Fault(
                        BluetoothLegacyAdvertisingRecurringStopFault {
                            _task: self.task,
                            _cancelled: self.cancelled,
                            _order: self.order,
                            error,
                        },
                    );
                }
            }
        }
        match self
            .task
            .restore_legacy_advertising_cancelled_disabled(self.cancelled)
        {
            crate::BluetoothLegacyAdvertisingCancelledRestoreOutcome::Restored => {
                match self.order {
                    BluetoothLegacyAdvertisingStopOrder::Disable(deferred) => {
                        BluetoothLegacyAdvertisingRecurringStopRestoreStep::DisableResponse(
                            BluetoothLegacyAdvertisingDisableResponsePending::from_cancelled(
                                self.task, deferred,
                            ),
                        )
                    }
                    BluetoothLegacyAdvertisingStopOrder::Reset(barrier) => {
                        BluetoothLegacyAdvertisingRecurringStopRestoreStep::ResetCompletion(
                            BluetoothLegacyAdvertisingResetCompletionReady::from_cancelled(
                                self.task, barrier,
                            ),
                        )
                    }
                }
            }
            crate::BluetoothLegacyAdvertisingCancelledRestoreOutcome::Rejected(cancelled) => {
                self.cancelled = cancelled;
                BluetoothLegacyAdvertisingRecurringStopRestoreStep::Rejected(self)
            }
        }
    }
}
