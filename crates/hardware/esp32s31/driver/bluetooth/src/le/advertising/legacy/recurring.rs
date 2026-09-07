//! Bounded successor scheduling for active legacy advertising.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginError,
        ControllerSchedulerCurrentError, ControllerSchedulerCurrentPending,
        ControllerSchedulerCurrentStep, SchedulerRunInterruptStorage,
        boot::{
            LegacyAdvertisingRecurringCandidateFailure,
            LegacyAdvertisingRecurringSequenceCompletion,
        },
    },
    le::advertising::{
        LegacyAdvertisingActiveResponsePending, LegacyAdvertisingActiveSession,
        LegacyAdvertisingDisableResponsePending, LegacyAdvertisingEventCpuOwned,
        LegacyAdvertisingNextEventScheduled, LegacyAdvertisingRecurringEventCandidate,
        LegacyAdvertisingRecurringPreparationError, LegacyAdvertisingRecurringPreparationFailure,
        LegacyAdvertisingResetCompletionReady, LegacyAdvertisingStopping,
        legacy::active::LegacyAdvertisingStopOrder,
    },
    scheduler::{
        BluetoothSchedulerHardwareListIndex, LegacyAdvertisingEmptySchedulerMergePrepared,
        LegacyAdvertisingEventPrepared, LegacyAdvertisingRecurringEventPreparationError,
        LegacyAdvertisingRecurringPreSequence, LegacyAdvertisingSchedulerHeadPublished,
        SchedulerEmptyListMergeError, SchedulerHeadPublicationError,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciActiveLegacyAdvertisingCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerResponsePending, LeControllerResponsePublication,
};

use oer_bluetooth_ll::advertising::AdvertisingDelay;

use oer_esp32s31_hal::types::BluetoothControllerSramAddress;

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;
type Order<'runtime> = LeControllerCommandReady<'runtime, ()>;

enum LegacyAdvertisingRecurringOrder<'runtime> {
    Ready(Order<'runtime>),
    ResponsePending(LeControllerResponsePending<'runtime, ()>),
    Stopping(LegacyAdvertisingStopOrder<'runtime>),
    Detached,
}

struct LegacyAdvertisingRecurringAxes<'runtime, S, const CAPACITY: usize> {
    task: Option<Task<'runtime, S, CAPACITY>>,
    order: LegacyAdvertisingRecurringOrder<'runtime>,
    previous_scheduler_item_address: BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

enum LegacyAdvertisingRecurringPhase<'runtime, S, const CAPACITY: usize> {
    Scheduled(LegacyAdvertisingNextEventScheduled<'static>),
    CandidatePreparationFailure(LegacyAdvertisingRecurringPreparationFailure<'static>),
    Candidate(LegacyAdvertisingRecurringEventCandidate<'static>),
    SequenceBegin(LegacyAdvertisingRecurringPreSequence<'static>),
    SequenceWait {
        pending: ControllerSchedulerCurrentPending<'runtime, S, CAPACITY>,
        admitted: LegacyAdvertisingRecurringPreSequence<'static>,
    },
    Merge(LegacyAdvertisingEventPrepared<'static>),
    Merged(LegacyAdvertisingEmptySchedulerMergePrepared<'static>),
    Head(LegacyAdvertisingSchedulerHeadPublished<'static>),
}

/// One successor from a completed event through the next scheduler `RUN`.
#[must_use = "drive, retry, or retain the exact recurring advertising owner"]
pub struct LegacyAdvertisingRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    axes: Option<LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>>,
    phase: LegacyAdvertisingRecurringPhase<'runtime, S, CAPACITY>,
}

/// Result of attaching the caller's fresh advertising delay.
#[must_use = "retain the recurring runner or sequence-exhausted CPU owner"]
#[expect(
    clippy::large_enum_variant,
    reason = "each variant retains its exact command, radio continuation, or sealed failure owners inline"
)]
pub enum LegacyAdvertisingRecurringStart<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Runner(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    SequenceExhausted(LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
}

/// One finite recurring transition.
#[must_use = "retain the runner, wait, running graph, retry, or fail-stop owner"]
pub enum LegacyAdvertisingRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    WaitControllerTime(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Running(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    RunningResponsePending(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    RunningStopping(LegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    Retryable(LegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// HCI order currently retained beside a recurring radio graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingRecurringOrderState {
    CommandReady,
    ResponsePending,
    Stopping,
}

/// One readiness observation for the recurring HCI order axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingRecurringOrderProgress {
    Command,
    Response,
}

/// Opaque owner for an impossible endpoint mismatch after recurring intake.
#[must_use = "retain the complete command, recurring radio graph and order"]
pub struct LegacyAdvertisingRecurringCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
    >,
}

/// Typed route for a command accepted while preparing the next event.
#[must_use = "continue, publish, stop, or retain the exact mismatch owner"]
pub enum LegacyAdvertisingRecurringCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingRecurringCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking command intake while a successor event is being prepared.
#[must_use = "route a command or retain the exact recurring runner"]
pub enum LegacyAdvertisingRecurringCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: LegacyAdvertisingRecurringCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// Result of publishing a recurring response without pausing radio progress.
#[must_use = "retain the recurring runner and exact HCI response order"]
pub enum LegacyAdvertisingRecurringResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Pending(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Fault {
        runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Entry selected by a retained Disable or Reset before the successor `RUN`.
#[must_use = "restore the cancelled graph or finish the already-published successor"]
pub enum LegacyAdvertisingRecurringStopBegin<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Restore(LegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>),
    Published(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// Cancelled unpublished successor retained until runtime restore is accepted.
#[must_use = "drain Controller time if required and restore the exact graph"]
pub struct LegacyAdvertisingRecurringStopRestore<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    order: LegacyAdvertisingStopOrder<'runtime>,
    controller_time_drain_required: bool,
}

/// One exact unpublished-successor stop/restore transition.
#[must_use = "retain the wait, response order, rejected restore, or fail-stop owner"]
pub enum LegacyAdvertisingRecurringStopRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>),
    DisableResponse(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    ResetCompletion(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
    Rejected(LegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingRecurringStopFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner for an abandoned Controller-time drain during recurrence cancellation.
#[must_use = "retain every cancelled radio and HCI owner for shutdown diagnostics"]
pub struct LegacyAdvertisingRecurringStopFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _task: Task<'runtime, S, CAPACITY>,
    _cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    _order: LegacyAdvertisingStopOrder<'runtime>,
    error: ControllerSchedulerCurrentError,
}

impl<S, const CAPACITY: usize> LegacyAdvertisingRecurringStopFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn error(&self) -> ControllerSchedulerCurrentError {
        self.error
    }
}

/// Finite reason a lossless recurring phase asks its supervisor to retry.
#[derive(Debug)]
pub enum LegacyAdvertisingRecurringRetryCause<E> {
    Preparation(LegacyAdvertisingRecurringPreparationError),
    Event(LegacyAdvertisingRecurringEventPreparationError),
    ControllerTimeBegin(ControllerSchedulerCurrentBeginError),
    EmptyList(SchedulerEmptyListMergeError),
    HeadPublication(SchedulerHeadPublicationError),
    SchedulerStart(E),
}

/// Exact retryable recurring phase.
#[must_use = "inspect and retry the unchanged recurring runner"]
pub struct LegacyAdvertisingRecurringRetry<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyAdvertisingRecurringRetryCause<S::Error>,
    runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &LegacyAdvertisingRecurringRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }
}

/// Non-retryable recurring ownership failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingRecurringFaultCause {
    SchedulerEpochUnavailable,
    ControllerTime(ControllerSchedulerCurrentError),
    SchedulerMergeCancellationRejected,
}

#[allow(
    dead_code,
    reason = "the fail-stop owner intentionally retains every recurrence input"
)]
enum LegacyAdvertisingRecurringFaultOwner<'runtime, S, const CAPACITY: usize> {
    Scheduled {
        axes: LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        scheduled: LegacyAdvertisingNextEventScheduled<'static>,
    },
    SequenceBegin {
        axes: LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        admitted: LegacyAdvertisingRecurringPreSequence<'static>,
    },
    SequenceRecheck {
        axes: LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        admitted: LegacyAdvertisingRecurringPreSequence<'static>,
    },
    Merged {
        axes: LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    },
}

/// Opaque fail-stop owner for a recurring event.
#[must_use = "retain the exact failed recurrence for diagnostic shutdown"]
pub struct LegacyAdvertisingRecurringFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyAdvertisingRecurringFaultCause,
    _owner: LegacyAdvertisingRecurringFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> LegacyAdvertisingRecurringFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyAdvertisingRecurringFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Attach one source-owned random delay without hiding entropy policy.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> LegacyAdvertisingRecurringStart<'runtime, S, CAPACITY> {
        let (task, order, address, index, completed) = self.into_parts();
        match completed.schedule_next(delay) {
            Ok(scheduled) => {
                LegacyAdvertisingRecurringStart::Runner(LegacyAdvertisingRecurringRunner {
                    axes: Some(LegacyAdvertisingRecurringAxes {
                        task: Some(task),
                        order: LegacyAdvertisingRecurringOrder::Ready(order),
                        previous_scheduler_item_address: address,
                        hardware_list_index: index,
                    }),
                    phase: LegacyAdvertisingRecurringPhase::Scheduled(scheduled),
                })
            }
            Err(failure) => LegacyAdvertisingRecurringStart::SequenceExhausted(
                LegacyAdvertisingEventCpuOwned::from_parts(
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

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn from_parts(
        axes: LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        phase: LegacyAdvertisingRecurringPhase<'runtime, S, CAPACITY>,
    ) -> Self {
        Self {
            axes: Some(axes),
            phase,
        }
    }

    fn retryable(
        self,
        cause: LegacyAdvertisingRecurringRetryCause<S::Error>,
    ) -> LegacyAdvertisingRecurringRunnerStep<'runtime, S, CAPACITY> {
        LegacyAdvertisingRecurringRunnerStep::Retryable(LegacyAdvertisingRecurringRetry {
            cause,
            runner: self,
        })
    }

    /// Current independently progressing HCI order beside the radio graph.
    pub fn order_state(&self) -> LegacyAdvertisingRecurringOrderState {
        let axes = self
            .axes
            .as_ref()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            LegacyAdvertisingRecurringOrder::Ready(_) => {
                LegacyAdvertisingRecurringOrderState::CommandReady
            }
            LegacyAdvertisingRecurringOrder::ResponsePending(_) => {
                LegacyAdvertisingRecurringOrderState::ResponsePending
            }
            LegacyAdvertisingRecurringOrder::Stopping(_) => {
                LegacyAdvertisingRecurringOrderState::Stopping
            }
            LegacyAdvertisingRecurringOrder::Detached => {
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
        LegacyAdvertisingRecurringOrderProgress,
        oer_bluetooth_hci::LeControllerEndpointMismatch,
    > {
        let axes = self
            .axes
            .as_ref()
            .expect("a recurring runner retains its exact Controller axes");
        match &axes.order {
            LegacyAdvertisingRecurringOrder::Ready(order) => {
                controller.wait_command_available(order).await?;
                Ok(LegacyAdvertisingRecurringOrderProgress::Command)
            }
            LegacyAdvertisingRecurringOrder::ResponsePending(response) => {
                controller.wait_response_capacity(response).await?;
                Ok(LegacyAdvertisingRecurringOrderProgress::Response)
            }
            LegacyAdvertisingRecurringOrder::Stopping(_) => {
                unreachable!("a stopping recurrence has no independently progressing HCI wait")
            }
            LegacyAdvertisingRecurringOrder::Detached => {
                unreachable!("a stored recurring runner cannot have detached HCI order")
            }
        }
    }

    fn detach_ready_order(mut self) -> (Self, Order<'runtime>) {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        let order = core::mem::replace(&mut axes.order, LegacyAdvertisingRecurringOrder::Detached);
        match order {
            LegacyAdvertisingRecurringOrder::Ready(order) => (self, order),
            _ => unreachable!("only a command-ready recurrence can accept another command"),
        }
    }

    fn attach_ready_order(mut self, order: Order<'runtime>) -> Self {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            LegacyAdvertisingRecurringOrder::Detached => {
                axes.order = LegacyAdvertisingRecurringOrder::Ready(order);
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
            LegacyAdvertisingRecurringOrder::Detached => {
                axes.order = LegacyAdvertisingRecurringOrder::ResponsePending(response);
                self
            }
            _ => unreachable!("a recurring runner cannot acquire a second HCI order"),
        }
    }

    fn attach_stopping(mut self, order: LegacyAdvertisingStopOrder<'runtime>) -> Self {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        match axes.order {
            LegacyAdvertisingRecurringOrder::Detached => {
                axes.order = LegacyAdvertisingRecurringOrder::Stopping(order);
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
    ) -> LegacyAdvertisingRecurringCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let (runner, order) = self.detach_ready_order();
        let ready = order.map_owner(|()| runner);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route = match controller
                    .route_active_legacy_advertising_classified_command(command)
                {
                    HciActiveLegacyAdvertisingCommandRoute::ResponsePending(response) => {
                        let (runner, response) = response.into_parts();
                        LegacyAdvertisingRecurringCommandRoute::Continue(
                            runner.attach_response(response),
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::Disable(deferred) => {
                        let (runner, deferred) = deferred.into_parts();
                        LegacyAdvertisingRecurringCommandRoute::Continue(
                            runner.attach_stopping(LegacyAdvertisingStopOrder::Disable(deferred)),
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) => {
                        let (runner, barrier) = barrier.into_parts();
                        LegacyAdvertisingRecurringCommandRoute::Continue(
                            runner.attach_stopping(LegacyAdvertisingStopOrder::Reset(barrier)),
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command) => {
                        LegacyAdvertisingRecurringCommandRoute::EndpointMismatch(
                            LegacyAdvertisingRecurringCommandMismatch { _command: command },
                        )
                    }
                };
                LegacyAdvertisingRecurringCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (runner, order) = ready.into_parts();
                LegacyAdvertisingRecurringCommandIntake::Empty {
                    runner: runner.attach_ready_order(order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (runner, order) = ready.into_parts();
                LegacyAdvertisingRecurringCommandIntake::EndpointMismatch {
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
                LegacyAdvertisingRecurringCommandIntake::Channel {
                    runner: runner.attach_ready_order(order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (runner, order) = ready.into_parts();
                LegacyAdvertisingRecurringCommandIntake::NonCommand {
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
    ) -> LegacyAdvertisingRecurringResponsePublication<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .as_mut()
            .expect("a recurring runner retains its exact Controller axes");
        let response =
            core::mem::replace(&mut axes.order, LegacyAdvertisingRecurringOrder::Detached);
        let LegacyAdvertisingRecurringOrder::ResponsePending(response) = response else {
            unreachable!("only a response-pending recurrence can publish a response")
        };
        match response.map_owner(|()| self).try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (runner, order) = ready.into_parts();
                LegacyAdvertisingRecurringResponsePublication::Published(
                    runner.attach_ready_order(order),
                )
            }
            LeControllerResponsePublication::Pending(response) => {
                let (runner, response) = response.into_parts();
                LegacyAdvertisingRecurringResponsePublication::Pending(
                    runner.attach_response(response),
                )
            }
            LeControllerResponsePublication::EndpointMismatch(response) => {
                let (runner, response) = response.into_parts();
                LegacyAdvertisingRecurringResponsePublication::EndpointMismatch(
                    runner.attach_response(response),
                )
            }
            LeControllerResponsePublication::Fault {
                pending: response,
                error,
            } => {
                let (runner, response) = response.into_parts();
                LegacyAdvertisingRecurringResponsePublication::Fault {
                    runner: runner.attach_response(response),
                    error,
                }
            }
        }
    }

    fn stop_restore(
        axes: LegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
        controller_time_drain_required: bool,
    ) -> LegacyAdvertisingRecurringStopBegin<'runtime, S, CAPACITY> {
        let LegacyAdvertisingRecurringAxes {
            task,
            order,
            previous_scheduler_item_address: _,
            hardware_list_index: _,
        } = axes;
        let LegacyAdvertisingRecurringOrder::Stopping(order) = order else {
            unreachable!("only a retained stop order can cancel a recurrence")
        };
        LegacyAdvertisingRecurringStopBegin::Restore(LegacyAdvertisingRecurringStopRestore {
            task: task.expect("a cancelled recurrence retains its task service"),
            cancelled,
            order,
            controller_time_drain_required,
        })
    }

    /// Cancel every unpublished successor phase; a published HEAD must finish once.
    pub fn begin_stopping(mut self) -> LegacyAdvertisingRecurringStopBegin<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .take()
            .expect("a recurring runner retains its exact Controller axes");
        assert!(matches!(
            axes.order,
            LegacyAdvertisingRecurringOrder::Stopping(_)
        ));
        match self.phase {
            LegacyAdvertisingRecurringPhase::Scheduled(scheduled) => {
                Self::stop_restore(axes, scheduled.cancel(), false)
            }
            LegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure) => {
                Self::stop_restore(axes, failure.cancel(), false)
            }
            LegacyAdvertisingRecurringPhase::Candidate(candidate) => {
                Self::stop_restore(axes, candidate.cancel().into_parts().0, false)
            }
            LegacyAdvertisingRecurringPhase::SequenceBegin(admitted) => {
                let mut axes = axes;
                let cancelled = axes
                    .task
                    .as_mut()
                    .expect("an admitted recurrence retains its task service")
                    .cancel_legacy_advertising_recurring_pre_sequence(admitted);
                Self::stop_restore(axes, cancelled, false)
            }
            LegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted } => {
                let LegacyAdvertisingRecurringAxes {
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
                            LegacyAdvertisingRecurringAxes {
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
                        let cause =
                            LegacyAdvertisingRecurringFaultCause::ControllerTime(failure.error());
                        let task = failure.into_parts().0.into_task_service();
                        LegacyAdvertisingRecurringStopBegin::Fault(
                            LegacyAdvertisingRecurringFault {
                                cause,
                                _owner: LegacyAdvertisingRecurringFaultOwner::SequenceRecheck {
                                    axes: LegacyAdvertisingRecurringAxes {
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
            LegacyAdvertisingRecurringPhase::Merge(prepared) => {
                let mut axes = axes;
                let cancelled = axes
                    .task
                    .as_mut()
                    .expect("a prepared recurrence retains its task service")
                    .cancel_legacy_advertising_recurring_prepared(prepared);
                Self::stop_restore(axes, cancelled, false)
            }
            LegacyAdvertisingRecurringPhase::Merged(merged) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merged recurrence retains its task service")
                    .cancel_legacy_advertising_recurring_merge(merged)
                {
                    Ok(cancelled) => Self::stop_restore(axes, cancelled, false),
                    Err(merged) => LegacyAdvertisingRecurringStopBegin::Fault(
                        LegacyAdvertisingRecurringFault {
                            cause: LegacyAdvertisingRecurringFaultCause::SchedulerMergeCancellationRejected,
                            _owner: LegacyAdvertisingRecurringFaultOwner::Merged {
                                axes,
                                merged,
                            },
                        },
                    ),
                }
            }
            LegacyAdvertisingRecurringPhase::Head(head) => {
                LegacyAdvertisingRecurringStopBegin::Published(Self::from_parts(
                    axes,
                    LegacyAdvertisingRecurringPhase::Head(head),
                ))
            }
        }
    }

    /// Execute exactly one recurrence edge.
    pub fn step(mut self) -> LegacyAdvertisingRecurringRunnerStep<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .take()
            .expect("a recurring runner retains its exact Controller axes");
        match self.phase {
            LegacyAdvertisingRecurringPhase::Scheduled(scheduled) => {
                match axes
                    .task
                    .as_ref()
                    .expect("a scheduled recurrence retains its task service")
                    .prepare_legacy_advertising_recurring_candidate(scheduled)
                {
                    Ok(candidate) => {
                        LegacyAdvertisingRecurringRunnerStep::Continue(Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::Candidate(candidate),
                        ))
                    }
                    Err(LegacyAdvertisingRecurringCandidateFailure::Preparation(failure)) => {
                        let cause =
                            LegacyAdvertisingRecurringRetryCause::Preparation(failure.error());
                        Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure),
                        )
                        .retryable(cause)
                    }
                    Err(LegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                        scheduled,
                    )) => LegacyAdvertisingRecurringRunnerStep::Fault(
                        LegacyAdvertisingRecurringFault {
                            cause: LegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: LegacyAdvertisingRecurringFaultOwner::Scheduled {
                                axes,
                                scheduled,
                            },
                        },
                    ),
                }
            }
            LegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure) => {
                match axes
                    .task
                    .as_ref()
                    .expect("a preparation retry retains its task service")
                    .retry_legacy_advertising_recurring_candidate(failure)
                {
                    Ok(candidate) => {
                        LegacyAdvertisingRecurringRunnerStep::Continue(Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::Candidate(candidate),
                        ))
                    }
                    Err(LegacyAdvertisingRecurringCandidateFailure::Preparation(failure)) => {
                        let cause =
                            LegacyAdvertisingRecurringRetryCause::Preparation(failure.error());
                        Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure),
                        )
                        .retryable(cause)
                    }
                    Err(LegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                        scheduled,
                    )) => LegacyAdvertisingRecurringRunnerStep::Fault(
                        LegacyAdvertisingRecurringFault {
                            cause: LegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: LegacyAdvertisingRecurringFaultOwner::Scheduled {
                                axes,
                                scheduled,
                            },
                        },
                    ),
                }
            }
            LegacyAdvertisingRecurringPhase::Candidate(candidate) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a candidate recurrence retains its task service")
                    .admit_legacy_advertising_recurring_candidate(candidate)
                {
                    Ok(admitted) => {
                        LegacyAdvertisingRecurringRunnerStep::Continue(Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::SequenceBegin(admitted),
                        ))
                    }
                    Err(failure) => {
                        let cause = LegacyAdvertisingRecurringRetryCause::Event(failure.error());
                        Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::Candidate(failure.into_candidate()),
                        )
                        .retryable(cause)
                    }
                }
            }
            LegacyAdvertisingRecurringPhase::SequenceBegin(admitted) => {
                let LegacyAdvertisingRecurringAxes {
                    task,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                let task = task.expect("a sequence-begin recurrence retains its task service");
                let epoch = match task.retain_scheduler_epoch() {
                    Ok(epoch) => epoch,
                    Err(unavailable) => {
                        return LegacyAdvertisingRecurringRunnerStep::Fault(
                            LegacyAdvertisingRecurringFault {
                                cause:
                                    LegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                                _owner: LegacyAdvertisingRecurringFaultOwner::SequenceBegin {
                                    axes: LegacyAdvertisingRecurringAxes {
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
                    Ok(pending) => {
                        LegacyAdvertisingRecurringRunnerStep::WaitControllerTime(Self::from_parts(
                            LegacyAdvertisingRecurringAxes {
                                task: None,
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            LegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted },
                        ))
                    }
                    Err(failure) => {
                        let cause = LegacyAdvertisingRecurringRetryCause::ControllerTimeBegin(
                            failure.error(),
                        );
                        let task = failure.into_parts().0.into_task_service();
                        Self::from_parts(
                            LegacyAdvertisingRecurringAxes {
                                task: Some(task),
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            LegacyAdvertisingRecurringPhase::SequenceBegin(admitted),
                        )
                        .retryable(cause)
                    }
                }
            }
            LegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted } => {
                let LegacyAdvertisingRecurringAxes {
                    task: _,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                match pending.recheck() {
                    Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                        LegacyAdvertisingRecurringRunnerStep::WaitControllerTime(Self::from_parts(
                            LegacyAdvertisingRecurringAxes {
                                task: None,
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            LegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted },
                        ))
                    }
                    Ok(ControllerSchedulerCurrentStep::Ready(current)) => {
                        match current.finish_legacy_advertising_recurring_event(admitted) {
                            LegacyAdvertisingRecurringSequenceCompletion::Prepared {
                                task,
                                merged,
                            } => LegacyAdvertisingRecurringRunnerStep::Continue(Self::from_parts(
                                LegacyAdvertisingRecurringAxes {
                                    task: Some(task),
                                    order,
                                    previous_scheduler_item_address,
                                    hardware_list_index,
                                },
                                LegacyAdvertisingRecurringPhase::Merged(merged),
                            )),
                            LegacyAdvertisingRecurringSequenceCompletion::EventRejected {
                                task,
                                failure,
                            } => {
                                let cause =
                                    LegacyAdvertisingRecurringRetryCause::Event(failure.error());
                                Self::from_parts(
                                    LegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    LegacyAdvertisingRecurringPhase::Candidate(
                                        failure.into_candidate(),
                                    ),
                                )
                                .retryable(cause)
                            }
                            LegacyAdvertisingRecurringSequenceCompletion::EmptyListRejected {
                                task,
                                failure,
                            } => {
                                let cause = LegacyAdvertisingRecurringRetryCause::EmptyList(
                                    failure.error(),
                                );
                                Self::from_parts(
                                    LegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    LegacyAdvertisingRecurringPhase::Merge(failure.into_prepared()),
                                )
                                .retryable(cause)
                            }
                        }
                    }
                    Err(failure) => {
                        let cause =
                            LegacyAdvertisingRecurringFaultCause::ControllerTime(failure.error());
                        let task = failure.into_parts().0.into_task_service();
                        LegacyAdvertisingRecurringRunnerStep::Fault(
                            LegacyAdvertisingRecurringFault {
                                cause,
                                _owner: LegacyAdvertisingRecurringFaultOwner::SequenceRecheck {
                                    axes: LegacyAdvertisingRecurringAxes {
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
            LegacyAdvertisingRecurringPhase::Merge(prepared) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merge retry retains its task service")
                    .merge_legacy_advertising_recurring_event(prepared)
                {
                    Ok(merged) => LegacyAdvertisingRecurringRunnerStep::Continue(Self::from_parts(
                        axes,
                        LegacyAdvertisingRecurringPhase::Merged(merged),
                    )),
                    Err(failure) => {
                        let cause =
                            LegacyAdvertisingRecurringRetryCause::EmptyList(failure.error());
                        Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::Merge(failure.into_prepared()),
                        )
                        .retryable(cause)
                    }
                }
            }
            LegacyAdvertisingRecurringPhase::Merged(merged) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merged recurrence retains its task service")
                    .publish_legacy_advertising_scheduler_head(merged)
                {
                    Ok(head) => LegacyAdvertisingRecurringRunnerStep::Continue(Self::from_parts(
                        axes,
                        LegacyAdvertisingRecurringPhase::Head(head),
                    )),
                    Err(failure) => {
                        let cause =
                            LegacyAdvertisingRecurringRetryCause::HeadPublication(failure.error());
                        Self::from_parts(
                            axes,
                            LegacyAdvertisingRecurringPhase::Merged(failure.into_merged()),
                        )
                        .retryable(cause)
                    }
                }
            }
            LegacyAdvertisingRecurringPhase::Head(head) => {
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
                            LegacyAdvertisingRecurringOrder::Ready(order) => {
                                LegacyAdvertisingRecurringRunnerStep::Running(
                                    LegacyAdvertisingActiveSession::from_recurring_running(
                                        task, order, running,
                                    ),
                                )
                            }
                            LegacyAdvertisingRecurringOrder::ResponsePending(response) => {
                                LegacyAdvertisingRecurringRunnerStep::RunningResponsePending(
                                    LegacyAdvertisingActiveSession::from_recurring_response_pending(
                                        task, response, running,
                                    ),
                                )
                            }
                            LegacyAdvertisingRecurringOrder::Stopping(order) => {
                                LegacyAdvertisingRecurringRunnerStep::RunningStopping(
                                    LegacyAdvertisingActiveSession::from_recurring_stopping(
                                        task, order, running,
                                    ),
                                )
                            }
                            LegacyAdvertisingRecurringOrder::Detached => {
                                unreachable!("a running recurrence cannot have detached HCI order")
                            }
                        }
                    }
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::from_parts(axes, LegacyAdvertisingRecurringPhase::Head(head))
                            .retryable(LegacyAdvertisingRecurringRetryCause::SchedulerStart(error))
                    }
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Whether the executor must await the absolute Controller-time recheck source.
    pub const fn controller_time_drain_required(&self) -> bool {
        self.controller_time_drain_required
    }

    /// Drain an abandoned time request, restore the graph, then expose HCI completion.
    pub fn step(mut self) -> LegacyAdvertisingRecurringStopRestoreStep<'runtime, S, CAPACITY> {
        if self.controller_time_drain_required {
            match self.task.drain_abandoned_recurring_controller_time() {
                Ok(crate::controller::ControllerTimeOrphanDrainStep::Waiting) => {
                    return LegacyAdvertisingRecurringStopRestoreStep::WaitControllerTime(self);
                }
                Ok(
                    crate::controller::ControllerTimeOrphanDrainStep::Idle
                    | crate::controller::ControllerTimeOrphanDrainStep::Drained,
                ) => self.controller_time_drain_required = false,
                Err(error) => {
                    return LegacyAdvertisingRecurringStopRestoreStep::Fault(
                        LegacyAdvertisingRecurringStopFault {
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
            crate::LegacyAdvertisingCancelledRestoreOutcome::Restored => match self.order {
                LegacyAdvertisingStopOrder::Disable(deferred) => {
                    LegacyAdvertisingRecurringStopRestoreStep::DisableResponse(
                        LegacyAdvertisingDisableResponsePending::from_cancelled(
                            self.task, deferred,
                        ),
                    )
                }
                LegacyAdvertisingStopOrder::Reset(barrier) => {
                    LegacyAdvertisingRecurringStopRestoreStep::ResetCompletion(
                        LegacyAdvertisingResetCompletionReady::from_cancelled(self.task, barrier),
                    )
                }
            },
            crate::LegacyAdvertisingCancelledRestoreOutcome::Rejected(cancelled) => {
                self.cancelled = cancelled;
                LegacyAdvertisingRecurringStopRestoreStep::Rejected(self)
            }
        }
    }
}
