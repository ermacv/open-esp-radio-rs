//! Independent radio-progress and HCI-order axes for one active DTM session.
//!
//! The active Controller graph never waits behind Controller-to-Host queue
//! capacity. A pending command response is retained beside, rather than around,
//! the radio state machine. Only successful durable publication changes the
//! order marker which will admit later HCI commands.

#![forbid(unsafe_code)]

use crate::{
    controller::SchedulerRunInterruptStorage,
    interrupt::SchedulerWakeCell,
    le::dtm::{
        DtmActiveCompletion, DtmActiveCompletionFault, DtmActiveCompletionFaultCause,
        DtmActiveCompletionStep, DtmActivePostUnlinkWait, DtmActiveSchedulerWait, DtmFirstRunning,
        DtmPostUnlinkWakeCell, DtmRecurringControllerTimeWait, DtmRecurringFault,
        DtmRecurringFaultCause, DtmRecurringRetry, DtmRecurringRetryCause, DtmRecurringRunner,
        DtmRecurringRunnerStep, DtmRole,
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveDtmCommandRoute as HciActiveDtmCommandRoute, LeControllerClassifiedCommand,
    LeControllerClassifiedCommandRoute as HciClassifiedCommandRoute, LeControllerCommandEndpoint,
    LeControllerCommandIntake, LeControllerCommandReady as HciCommandReady,
    LeControllerResetBarrier as HciResetBarrier, LeControllerResponsePending as HciResponsePending,
    LeControllerResponsePublication as HciResponsePublication,
};

pub(crate) enum DtmActiveRadio<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completion(DtmActiveCompletion<'runtime, S, CAPACITY>),
    SchedulerWait(DtmActiveSchedulerWait<'runtime, S, CAPACITY>),
    PostUnlinkWait(DtmActivePostUnlinkWait<'runtime, S, CAPACITY>),
    Recurring(DtmRecurringRunner<'runtime, S, CAPACITY>),
    ControllerTimeWait(DtmRecurringControllerTimeWait<'runtime, S, CAPACITY>),
    Retryable(DtmRecurringRetry<'runtime, S, CAPACITY>),
}

/// HCI-order axis retaining one command response not yet durably enqueued.
#[must_use = "the response must remain paired with the active radio session"]
pub struct DtmResponsePending<'runtime> {
    transaction: HciResponsePending<'runtime, ()>,
}

/// HCI-order axis carrying the sole authority to accept the next command.
///
/// Later HCI command intake is implemented only for sessions carrying this
/// marker. The epoch remains available to bind that intake to the same channel.
#[must_use = "retain command-ready authority with the active radio session"]
pub struct DtmOrderReady<'runtime> {
    transaction: HciCommandReady<'runtime, ()>,
}

/// One affine DTM session with independent radio and HCI-order axes.
///
/// `Order` is either [`DtmResponsePending`] or
/// [`DtmOrderReady`]. Radio progress consumes and returns
/// the same `Order`, so C2H backpressure cannot stop completion or recurrence.
#[must_use = "advance both the active radio and HCI-order axes"]
pub struct DtmActiveSession<'runtime, S, const CAPACITY: usize, Order>
where
    S: SchedulerRunInterruptStorage,
{
    radio: DtmActiveRadio<'runtime, S, CAPACITY>,
    order: Order,
}

/// Active session whose current Command Complete still awaits HCI capacity.
pub type DtmResponsePendingSession<'runtime, S, const CAPACITY: usize> =
    DtmActiveSession<'runtime, S, CAPACITY, DtmResponsePending<'runtime>>;

/// Active session whose HCI order permits the next semantic command.
pub type DtmCommandReadySession<'runtime, S, const CAPACITY: usize> =
    DtmActiveSession<'runtime, S, CAPACITY, DtmOrderReady<'runtime>>;

/// Result of attempting the sole durable current-response publication.
#[must_use = "retain the unchanged pending session unless publication succeeds"]
pub enum DtmResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// The response entered the matching HCI epoch exactly once.
    Published(DtmCommandReadySession<'runtime, S, CAPACITY>),
    /// C2H capacity was unavailable; both independent axes are unchanged.
    Pending(DtmResponsePendingSession<'runtime, S, CAPACITY>),
    /// The endpoint belongs to another Controller epoch; both axes are unchanged.
    EndpointMismatch(DtmResponsePendingSession<'runtime, S, CAPACITY>),
    /// A non-capacity channel fault retained both axes unchanged.
    Fault {
        session: DtmResponsePendingSession<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// One accepted Reset paired opaquely with the exact active DTM radio/order owner.
///
/// Construction does not mutate bootstrap state or claim that active radio
/// work has quiesced. Neither the radio owner nor its command-ready authority can
/// be separated from the Reset token through this API.
#[must_use = "retain the Reset barrier until active radio work has quiesced"]
pub struct DtmActiveResetBarrier<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    radio: DtmActiveRadio<'runtime, S, CAPACITY>,
    barrier: HciResetBarrier<'runtime, ()>,
}

impl<'runtime, S, const CAPACITY: usize> DtmActiveResetBarrier<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Begin terminal-neutral quiescence without releasing Reset/order authority.
    pub fn begin_quiescence(self) -> crate::le::dtm::DtmResetStoppingRunner<'runtime, S, CAPACITY> {
        crate::le::dtm::DtmResetStoppingRunner::new(self.radio, self.barrier)
    }
}

/// Typed result of routing one complete Controller classification while DTM is active.
#[must_use = "publish the response, run Test End, retain Reset, or retain a mismatch"]
pub enum DtmActiveControllerCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// A terminal classification or second start became an ordered response.
    ResponsePending(DtmResponsePendingSession<'runtime, S, CAPACITY>),
    /// Test End consumed the command and entered hardware quiescence.
    TestEnd(crate::le::dtm::DtmStoppingRunner<'runtime, S, CAPACITY>),
    /// Reset must wait behind lifecycle quiescence without exposing either axis.
    ResetBarrier(DtmActiveResetBarrier<'runtime, S, CAPACITY>),
    /// Defensive fail-stop owner for an impossible post-intake epoch mismatch.
    EndpointMismatch(DtmActiveCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// Opaque active command retaining radio state, classification and HCI order.
#[must_use = "retain the complete post-intake mismatch owner"]
pub struct DtmActiveCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command:
        LeControllerClassifiedCommand<'runtime, 'command, DtmActiveRadio<'runtime, S, CAPACITY>>,
}

/// One non-blocking active-session command intake through the combined endpoint.
#[must_use = "route the command or retain the returned active session"]
pub enum DtmActiveCommandIntake<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: DtmActiveControllerCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        session: DtmCommandReadySession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        session: DtmCommandReadySession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        session: DtmCommandReadySession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        session: DtmCommandReadySession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// Borrowed wait source for the current radio axis.
///
/// The complete session remains owned by the caller while an executor borrows
/// one of these sources. Controller time intentionally has no invented wake;
/// the executor supplies a cooperative deadline or yield before rechecking.
#[derive(Clone, Copy)]
pub enum DtmActiveRadioWait<'session> {
    /// Yield between events before sampling the next event's deadline.
    Cooperative,
    Scheduler(&'session SchedulerWakeCell),
    PostUnlink(&'session DtmPostUnlinkWakeCell),
    ControllerTime,
}

/// One bounded radio-axis transition while retaining an arbitrary order marker.
#[must_use = "retain the session, unrelated list or fail-stop owner"]
pub enum DtmActiveSessionRadioStep<'runtime, S, const CAPACITY: usize, Order>
where
    S: SchedulerRunInterruptStorage,
{
    /// Another bounded radio transition may run immediately.
    Continue(DtmActiveSession<'runtime, S, CAPACITY, Order>),
    /// The radio axis is parked; inspect its borrowed wait source.
    Waiting(DtmActiveSession<'runtime, S, CAPACITY, Order>),
    /// One unrelated finished list remains owned by the external dispatcher.
    UnrelatedList {
        session: DtmActiveSession<'runtime, S, CAPACITY, Order>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// A finite recurring transition returned its unchanged owner.
    Retryable(DtmActiveSession<'runtime, S, CAPACITY, Order>),
    /// A fail-closed radio transition retained its owner and HCI-order axis.
    Fault(DtmActiveSessionFault<'runtime, S, CAPACITY, Order>),
}

/// Read-only fail-stop classification for the active session radio axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmActiveSessionFaultCause {
    Completion(DtmActiveCompletionFaultCause),
    Recurring(DtmRecurringFaultCause),
}

#[allow(
    dead_code,
    reason = "the fail-stop aggregate deliberately retains radio and order owners opaquely"
)]
enum DtmActiveSessionFaultOwner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completion(DtmActiveCompletionFault<'runtime, S, CAPACITY>),
    Recurring(DtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Opaque fail-stop owner preserving both the radio and HCI-order axes.
#[must_use = "retain the exact fail-stop session for diagnostic shutdown"]
pub struct DtmActiveSessionFault<'runtime, S, const CAPACITY: usize, Order>
where
    S: SchedulerRunInterruptStorage,
{
    role: DtmRole,
    cause: DtmActiveSessionFaultCause,
    _radio: DtmActiveSessionFaultOwner<'runtime, S, CAPACITY>,
    _order: Order,
}

impl<S, const CAPACITY: usize, Order> DtmActiveSessionFault<'_, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn role(&self) -> DtmRole {
        self.role
    }

    pub const fn cause(&self) -> DtmActiveSessionFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    DtmActiveSession<'runtime, S, CAPACITY, DtmResponsePending<'runtime>>
where
    S: SchedulerRunInterruptStorage,
{
    /// Whether an HCI endpoint belongs to the Controller epoch which started this session.
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
        self.order.transaction.matches_endpoint(controller)
    }

    /// Wait until the matching Controller-to-Host queue may accept this response.
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
        controller
            .wait_response_capacity(&self.order.transaction)
            .await
    }

    /// Attempt exact-once response publication without advancing radio state.
    pub fn try_publish_response<
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
    ) -> DtmResponsePublication<'runtime, S, CAPACITY> {
        if !self.matches_hci_endpoint(controller) {
            return DtmResponsePublication::EndpointMismatch(self);
        }
        match self.order.transaction.try_publish(controller) {
            HciResponsePublication::Published(transaction) => {
                DtmResponsePublication::Published(DtmActiveSession {
                    radio: self.radio,
                    order: DtmOrderReady { transaction },
                })
            }
            HciResponsePublication::Pending(transaction) => {
                DtmResponsePublication::Pending(DtmActiveSession {
                    radio: self.radio,
                    order: DtmResponsePending { transaction },
                })
            }
            HciResponsePublication::EndpointMismatch(transaction) => {
                DtmResponsePublication::EndpointMismatch(DtmActiveSession {
                    radio: self.radio,
                    order: DtmResponsePending { transaction },
                })
            }
            HciResponsePublication::Fault {
                pending: transaction,
                error,
            } => DtmResponsePublication::Fault {
                session: DtmActiveSession {
                    radio: self.radio,
                    order: DtmResponsePending { transaction },
                },
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    DtmActiveSession<'runtime, S, CAPACITY, DtmOrderReady<'runtime>>
where
    S: SchedulerRunInterruptStorage,
{
    /// Whether an endpoint may supply the next ordered HCI command.
    pub fn accepts_hci_endpoint<
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
        self.order.transaction.accepts_endpoint(controller)
    }

    /// Wait until the matching Host queue may contain a command.
    ///
    /// Cancellation preserves the complete active session and its sole affine
    /// next-command authority.
    pub async fn wait_command_available<
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
        controller
            .wait_command_available(&self.order.transaction)
            .await
    }

    /// Consume, classify and route at most one active-session Host command.
    pub fn try_route_active_controller_command_with_buffer<
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
    ) -> DtmActiveCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let Self { radio, order } = self;
        let ready = order.transaction.map_owner(|()| radio);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                DtmActiveCommandIntake::Routed {
                    route: Self::route_active_classified_command(controller, command),
                    buffer,
                }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => DtmActiveCommandIntake::Empty {
                session: Self::from_ready(ready),
                buffer,
            },
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                DtmActiveCommandIntake::EndpointMismatch {
                    session: Self::from_ready(ready),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => DtmActiveCommandIntake::Channel {
                session: Self::from_ready(ready),
                buffer,
                error,
            },
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                DtmActiveCommandIntake::NonCommand {
                    session: Self::from_ready(ready),
                    frame,
                }
            }
        }
    }

    fn from_ready(ready: HciCommandReady<'runtime, DtmActiveRadio<'runtime, S, CAPACITY>>) -> Self {
        let (radio, transaction) = ready.into_parts();
        Self {
            radio,
            order: DtmOrderReady { transaction },
        }
    }

    fn route_active_classified_command<
        'command,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        controller: &mut LeControllerCommandEndpoint<
            'command,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        command: LeControllerClassifiedCommand<
            'runtime,
            'command,
            DtmActiveRadio<'runtime, S, CAPACITY>,
        >,
    ) -> DtmActiveControllerCommandRoute<'runtime, 'command, S, CAPACITY> {
        match controller.route_classified_command(command) {
            HciClassifiedCommandRoute::ResponsePending(pending) => {
                let (radio, transaction) = pending.into_parts();
                DtmActiveControllerCommandRoute::ResponsePending(DtmActiveSession {
                    radio,
                    order: DtmResponsePending { transaction },
                })
            }
            HciClassifiedCommandRoute::Dtm(deferred) => {
                match deferred.into_active_session_route() {
                    HciActiveDtmCommandRoute::ResponsePending(pending) => {
                        let (radio, transaction) = pending.into_parts();
                        DtmActiveControllerCommandRoute::ResponsePending(DtmActiveSession {
                            radio,
                            order: DtmResponsePending { transaction },
                        })
                    }
                    HciActiveDtmCommandRoute::TestEnd(deferred) => {
                        let (radio, deferred) = deferred.into_parts();
                        DtmActiveControllerCommandRoute::TestEnd(
                            crate::le::dtm::DtmStoppingRunner::new(radio, deferred),
                        )
                    }
                }
            }
            HciClassifiedCommandRoute::ResetBarrier(barrier) => {
                let (radio, barrier) = barrier.into_parts();
                DtmActiveControllerCommandRoute::ResetBarrier(DtmActiveResetBarrier {
                    radio,
                    barrier,
                })
            }
            HciClassifiedCommandRoute::EndpointMismatch(command) => {
                DtmActiveControllerCommandRoute::EndpointMismatch(DtmActiveCommandMismatch {
                    _command: command,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize, Order> DtmActiveSession<'runtime, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
{
    /// Borrow the exact current radio wait without moving either affine axis.
    pub fn radio_wait(&self) -> Option<DtmActiveRadioWait<'_>> {
        match &self.radio {
            DtmActiveRadio::Recurring(runner) if runner.is_event_boundary() => {
                Some(DtmActiveRadioWait::Cooperative)
            }
            DtmActiveRadio::SchedulerWait(wait) => Some(DtmActiveRadioWait::Scheduler(wait.wake())),
            DtmActiveRadio::PostUnlinkWait(wait) => {
                Some(DtmActiveRadioWait::PostUnlink(wait.wake()))
            }
            DtmActiveRadio::ControllerTimeWait(_) => Some(DtmActiveRadioWait::ControllerTime),
            DtmActiveRadio::Completion(_)
            | DtmActiveRadio::Recurring(_)
            | DtmActiveRadio::Retryable(_) => None,
        }
    }

    /// Borrow the finite recurring retry cause without separating either axis.
    pub fn recurring_retry_cause(&self) -> Option<&DtmRecurringRetryCause<S::Error>> {
        match &self.radio {
            DtmActiveRadio::Retryable(retry) => Some(retry.cause()),
            _ => None,
        }
    }

    /// Advance exactly one radio edge while carrying the HCI-order axis unchanged.
    ///
    /// Calling this on a parked wait performs one bounded recheck. Calling it
    /// on a retryable owner performs the explicit retry selected by the caller.
    pub fn step_radio(self) -> DtmActiveSessionRadioStep<'runtime, S, CAPACITY, Order> {
        match self.radio {
            DtmActiveRadio::Completion(completion) => step_completion(self.order, completion),
            DtmActiveRadio::SchedulerWait(wait) => match wait.wake().take() {
                Some(wake) => step_completion(self.order, wait.resume(wake)),
                None => DtmActiveSessionRadioStep::Waiting(DtmActiveSession {
                    radio: DtmActiveRadio::SchedulerWait(wait),
                    order: self.order,
                }),
            },
            DtmActiveRadio::PostUnlinkWait(wait) => step_completion(self.order, wait.resume()),
            DtmActiveRadio::Recurring(recurring) => step_recurring(self.order, recurring),
            DtmActiveRadio::ControllerTimeWait(wait) => step_recurring(self.order, wait.resume()),
            DtmActiveRadio::Retryable(retry) => step_recurring(self.order, retry.retry()),
        }
    }
}

fn step_completion<'runtime, S, const CAPACITY: usize, Order>(
    order: Order,
    completion: DtmActiveCompletion<'runtime, S, CAPACITY>,
) -> DtmActiveSessionRadioStep<'runtime, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
{
    match completion.step() {
        DtmActiveCompletionStep::Continue(completion) => {
            DtmActiveSessionRadioStep::Continue(DtmActiveSession {
                radio: DtmActiveRadio::Completion(completion),
                order,
            })
        }
        DtmActiveCompletionStep::WaitScheduler(wait) => {
            DtmActiveSessionRadioStep::Waiting(DtmActiveSession {
                radio: DtmActiveRadio::SchedulerWait(wait),
                order,
            })
        }
        DtmActiveCompletionStep::UnrelatedList {
            completion,
            observed,
        } => DtmActiveSessionRadioStep::UnrelatedList {
            session: DtmActiveSession {
                radio: DtmActiveRadio::Completion(completion),
                order,
            },
            observed,
        },
        DtmActiveCompletionStep::WaitPostUnlink(wait) => {
            DtmActiveSessionRadioStep::Waiting(DtmActiveSession {
                radio: DtmActiveRadio::PostUnlinkWait(wait),
                order,
            })
        }
        DtmActiveCompletionStep::CpuOwned(owner) => {
            DtmActiveSessionRadioStep::Continue(DtmActiveSession {
                radio: DtmActiveRadio::Recurring(owner.begin_recurring()),
                order,
            })
        }
        DtmActiveCompletionStep::Fault(fault) => {
            DtmActiveSessionRadioStep::Fault(DtmActiveSessionFault {
                role: fault.role(),
                cause: DtmActiveSessionFaultCause::Completion(fault.cause()),
                _radio: DtmActiveSessionFaultOwner::Completion(fault),
                _order: order,
            })
        }
    }
}

fn step_recurring<'runtime, S, const CAPACITY: usize, Order>(
    order: Order,
    recurring: DtmRecurringRunner<'runtime, S, CAPACITY>,
) -> DtmActiveSessionRadioStep<'runtime, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
{
    match recurring.step() {
        DtmRecurringRunnerStep::Continue(recurring) => {
            DtmActiveSessionRadioStep::Continue(DtmActiveSession {
                radio: DtmActiveRadio::Recurring(recurring),
                order,
            })
        }
        DtmRecurringRunnerStep::WaitControllerTime(wait) => {
            DtmActiveSessionRadioStep::Waiting(DtmActiveSession {
                radio: DtmActiveRadio::ControllerTimeWait(wait),
                order,
            })
        }
        DtmRecurringRunnerStep::Running(completion) => {
            DtmActiveSessionRadioStep::Continue(DtmActiveSession {
                radio: DtmActiveRadio::Completion(completion),
                order,
            })
        }
        DtmRecurringRunnerStep::Retryable(retry) => {
            DtmActiveSessionRadioStep::Retryable(DtmActiveSession {
                radio: DtmActiveRadio::Retryable(retry),
                order,
            })
        }
        DtmRecurringRunnerStep::Fault(fault) => {
            DtmActiveSessionRadioStep::Fault(DtmActiveSessionFault {
                role: fault.role(),
                cause: DtmActiveSessionFaultCause::Recurring(fault.cause()),
                _radio: DtmActiveSessionFaultOwner::Recurring(fault),
                _order: order,
            })
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmFirstRunning<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Split first `RUN` into independently progressing radio and HCI-order axes.
    pub fn into_active_session(self) -> DtmResponsePendingSession<'runtime, S, CAPACITY> {
        let (transaction, completion) = DtmActiveCompletion::from_first_running(self.into_parts());
        DtmActiveSession {
            radio: DtmActiveRadio::Completion(completion),
            order: DtmResponsePending { transaction },
        }
    }
}
