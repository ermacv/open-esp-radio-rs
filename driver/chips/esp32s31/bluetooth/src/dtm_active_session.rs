//! Independent radio-progress and HCI-order axes for one active DTM session.
//!
//! The active Controller graph never waits behind Controller-to-Host queue
//! capacity. A pending command response is retained beside, rather than around,
//! the radio state machine. Only successful durable publication changes the
//! order marker which will admit later HCI commands.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, InProcessHciControllerEndpoint,
    LeControllerClassifiedCommandRoute as HciClassifiedCommandRoute,
    LeControllerCommandClassification, LeControllerCommandEndpoint,
    LeControllerResetBarrier as HciResetBarrier, LeControllerResponsePending as HciResponsePending,
    LeControllerResponsePublication as HciResponsePublication,
    LeControllerResponsePublished as HciResponsePublished, LeDtmActiveSessionDisposition,
    LeDtmCommandCompleteEvent,
};

use crate::{
    BluetoothDtmActiveCompletion, BluetoothDtmActiveCompletionFault,
    BluetoothDtmActiveCompletionFaultCause, BluetoothDtmActiveCompletionStep,
    BluetoothDtmActivePostUnlinkWait, BluetoothDtmActiveSchedulerWait, BluetoothDtmFirstRunning,
    BluetoothDtmPostUnlinkWakeCell, BluetoothDtmRecurringControllerTimeWait,
    BluetoothDtmRecurringFault, BluetoothDtmRecurringFaultCause, BluetoothDtmRecurringRetry,
    BluetoothDtmRecurringRetryCause, BluetoothDtmRecurringRunner, BluetoothDtmRecurringRunnerStep,
    BluetoothDtmRole, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerWakeCell,
};

pub(crate) enum BluetoothDtmActiveRadio<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completion(BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>),
    SchedulerWait(BluetoothDtmActiveSchedulerWait<'runtime, S, CAPACITY>),
    PostUnlinkWait(BluetoothDtmActivePostUnlinkWait<'runtime, S, CAPACITY>),
    Recurring(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    ControllerTimeWait(BluetoothDtmRecurringControllerTimeWait<'runtime, S, CAPACITY>),
    Retryable(BluetoothDtmRecurringRetry<'runtime, S, CAPACITY>),
}

/// HCI-order axis retaining one command response not yet durably enqueued.
#[must_use = "the response must remain paired with the active radio session"]
pub struct BluetoothDtmResponsePending<'runtime> {
    transaction: HciResponsePending<'runtime, ()>,
}

/// HCI-order axis proving the previous command response was durably enqueued.
///
/// Later HCI command intake is implemented only for sessions carrying this
/// marker. The epoch remains available to bind that intake to the same channel.
#[must_use = "retain the published order proof with the active radio session"]
pub struct BluetoothDtmResponsePublished<'runtime> {
    transaction: HciResponsePublished<'runtime, ()>,
}

impl<'runtime> BluetoothDtmResponsePublished<'runtime> {
    pub(crate) fn begin_next_response<Owner>(
        self,
        owner: Owner,
        response: LeDtmCommandCompleteEvent,
    ) -> HciResponsePending<'runtime, Owner> {
        self.transaction
            .map_owner(|()| owner)
            .begin_next_response(response)
    }
}

/// One affine DTM session with independent radio and HCI-order axes.
///
/// `Order` is either [`BluetoothDtmResponsePending`] or
/// [`BluetoothDtmResponsePublished`]. Radio progress consumes and returns
/// the same `Order`, so C2H backpressure cannot stop completion or recurrence.
#[must_use = "advance both the active radio and HCI-order axes"]
pub struct BluetoothDtmActiveSession<'runtime, S, const CAPACITY: usize, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    radio: BluetoothDtmActiveRadio<'runtime, S, CAPACITY>,
    order: Order,
}

/// Active session whose current Command Complete still awaits HCI capacity.
pub type BluetoothDtmResponsePendingSession<'runtime, S, const CAPACITY: usize> =
    BluetoothDtmActiveSession<'runtime, S, CAPACITY, BluetoothDtmResponsePending<'runtime>>;

/// Active session whose HCI order permits the next semantic command.
pub type BluetoothDtmCommandReadySession<'runtime, S, const CAPACITY: usize> =
    BluetoothDtmActiveSession<'runtime, S, CAPACITY, BluetoothDtmResponsePublished<'runtime>>;

/// Result of attempting the sole durable current-response publication.
#[must_use = "retain the unchanged pending session unless publication succeeds"]
pub enum BluetoothDtmResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The response entered the matching HCI epoch exactly once.
    Published(BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>),
    /// C2H capacity was unavailable; both independent axes are unchanged.
    Pending(BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>),
    /// The endpoint belongs to another Controller epoch; both axes are unchanged.
    EndpointMismatch(BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>),
    /// A non-capacity channel fault retained both axes unchanged.
    Fault {
        session: BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// One accepted Reset paired opaquely with the exact active DTM radio/order owner.
///
/// Construction does not mutate bootstrap state or claim that active radio
/// work has quiesced. Neither the radio owner nor its published-order proof can
/// be separated from the Reset token through this API.
#[must_use = "retain the Reset barrier until active radio work has quiesced"]
pub struct BluetoothDtmActiveResetBarrier<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    radio: BluetoothDtmActiveRadio<'runtime, S, CAPACITY>,
    barrier: HciResetBarrier<'runtime, ()>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveResetBarrier<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Begin terminal-neutral quiescence without releasing Reset/order authority.
    pub fn begin_quiescence(self) -> crate::BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY> {
        crate::BluetoothDtmResetStoppingRunner::new(self.radio, self.barrier)
    }
}

/// Typed result of routing one complete Controller classification while DTM is active.
#[must_use = "publish the response, run Test End, retain Reset, or retain a mismatch"]
pub enum BluetoothDtmActiveControllerCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// A terminal classification or second start became an ordered response.
    ResponsePending(BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>),
    /// Test End consumed the command and entered hardware quiescence.
    TestEnd(crate::BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>),
    /// Reset must wait behind lifecycle quiescence without exposing either axis.
    ResetBarrier(BluetoothDtmActiveResetBarrier<'runtime, S, CAPACITY>),
    /// Either the session or complete classification belongs to another HCI epoch.
    EndpointMismatch {
        /// Unchanged command-ready active session.
        session: BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>,
        /// Unchanged complete classification with its exact origin proof.
        command: HciEpochBound<'command, LeControllerCommandClassification>,
    },
}

/// Borrowed wait source for the current radio axis.
///
/// The complete session remains owned by the caller while an executor borrows
/// one of these sources. Controller time intentionally has no invented wake;
/// the executor supplies a cooperative deadline or yield before rechecking.
#[derive(Clone, Copy)]
pub enum BluetoothDtmActiveRadioWait<'session> {
    Scheduler(&'session BluetoothSchedulerWakeCell),
    PostUnlink(&'session BluetoothDtmPostUnlinkWakeCell),
    ControllerTime,
}

/// One bounded radio-axis transition while retaining an arbitrary order marker.
#[must_use = "retain the session, unrelated list or fail-stop owner"]
pub enum BluetoothDtmActiveSessionRadioStep<'runtime, S, const CAPACITY: usize, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Another bounded radio transition may run immediately.
    Continue(BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>),
    /// The radio axis is parked; inspect its borrowed wait source.
    Waiting(BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>),
    /// One unrelated finished list remains owned by the external dispatcher.
    UnrelatedList {
        session: BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// A finite recurring transition returned its unchanged owner.
    Retryable(BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>),
    /// A fail-closed radio transition retained its owner and HCI-order axis.
    Fault(BluetoothDtmActiveSessionFault<'runtime, S, CAPACITY, Order>),
}

/// Read-only fail-stop classification for the active session radio axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmActiveSessionFaultCause {
    Completion(BluetoothDtmActiveCompletionFaultCause),
    Recurring(BluetoothDtmRecurringFaultCause),
}

#[allow(
    dead_code,
    reason = "the fail-stop aggregate deliberately retains radio and order owners opaquely"
)]
enum BluetoothDtmActiveSessionFaultOwner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completion(BluetoothDtmActiveCompletionFault<'runtime, S, CAPACITY>),
    Recurring(BluetoothDtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Opaque fail-stop owner preserving both the radio and HCI-order axes.
#[must_use = "retain the exact fail-stop session for diagnostic shutdown"]
pub struct BluetoothDtmActiveSessionFault<'runtime, S, const CAPACITY: usize, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    role: BluetoothDtmRole,
    cause: BluetoothDtmActiveSessionFaultCause,
    _radio: BluetoothDtmActiveSessionFaultOwner<'runtime, S, CAPACITY>,
    _order: Order,
}

impl<S, const CAPACITY: usize, Order> BluetoothDtmActiveSessionFault<'_, S, CAPACITY, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn role(&self) -> BluetoothDtmRole {
        self.role
    }

    pub const fn cause(&self) -> BluetoothDtmActiveSessionFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothDtmActiveSession<'runtime, S, CAPACITY, BluetoothDtmResponsePending<'runtime>>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Whether an HCI endpoint belongs to the Controller epoch which started this session.
    pub fn matches_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.order.transaction.matches_endpoint(controller)
    }

    /// Attempt exact-once response publication without advancing radio state.
    pub fn try_publish_response<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothDtmResponsePublication<'runtime, S, CAPACITY> {
        if !self.matches_hci_endpoint(controller) {
            return BluetoothDtmResponsePublication::EndpointMismatch(self);
        }
        match self.order.transaction.try_publish(controller) {
            HciResponsePublication::Published(transaction) => {
                BluetoothDtmResponsePublication::Published(BluetoothDtmActiveSession {
                    radio: self.radio,
                    order: BluetoothDtmResponsePublished { transaction },
                })
            }
            HciResponsePublication::Pending(transaction) => {
                BluetoothDtmResponsePublication::Pending(BluetoothDtmActiveSession {
                    radio: self.radio,
                    order: BluetoothDtmResponsePending { transaction },
                })
            }
            HciResponsePublication::EndpointMismatch(transaction) => {
                BluetoothDtmResponsePublication::EndpointMismatch(BluetoothDtmActiveSession {
                    radio: self.radio,
                    order: BluetoothDtmResponsePending { transaction },
                })
            }
            HciResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothDtmResponsePublication::Fault {
                session: BluetoothDtmActiveSession {
                    radio: self.radio,
                    order: BluetoothDtmResponsePending { transaction },
                },
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothDtmActiveSession<'runtime, S, CAPACITY, BluetoothDtmResponsePublished<'runtime>>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Whether an endpoint may supply the next ordered HCI command.
    pub fn accepts_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.order.transaction.accepts_endpoint(controller)
    }

    /// Route one endpoint-bound Controller classification under active-session policy.
    ///
    /// Both the published response marker and the command must match the
    /// supplied endpoint. Terminal classifications and a second start enter the
    /// same generic response axis; Test End enters the existing stopping runner.
    /// Non-Reset bootstrap commands dispatch immediately into the same response
    /// axis. Reset instead remains an opaque lifecycle barrier. Until a pending
    /// response publishes, no value carrying command-ready authority exists.
    pub fn route_active_controller_command<
        'command,
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
        command: HciEpochBound<'command, LeControllerCommandClassification>,
    ) -> BluetoothDtmActiveControllerCommandRoute<'runtime, 'command, S, CAPACITY> {
        let Self { radio, order } = self;
        match controller.route_classified_command(order.transaction, command) {
            HciClassifiedCommandRoute::ResponsePending(pending) => {
                BluetoothDtmActiveControllerCommandRoute::ResponsePending(
                    BluetoothDtmActiveSession {
                        radio,
                        order: BluetoothDtmResponsePending {
                            transaction: pending,
                        },
                    },
                )
            }
            HciClassifiedCommandRoute::Dtm { published, command } => {
                match command.into_active_session_disposition() {
                    LeDtmActiveSessionDisposition::RejectControllerBusy(response) => {
                        BluetoothDtmActiveControllerCommandRoute::ResponsePending(
                            BluetoothDtmActiveSession {
                                radio,
                                order: BluetoothDtmResponsePending {
                                    transaction: published.begin_next_response(response),
                                },
                            },
                        )
                    }
                    LeDtmActiveSessionDisposition::End(command) => {
                        BluetoothDtmActiveControllerCommandRoute::TestEnd(
                            crate::BluetoothDtmStoppingRunner::new(
                                radio,
                                BluetoothDtmResponsePublished {
                                    transaction: published,
                                },
                                command,
                            ),
                        )
                    }
                }
            }
            HciClassifiedCommandRoute::ResetBarrier(barrier) => {
                BluetoothDtmActiveControllerCommandRoute::ResetBarrier(
                    BluetoothDtmActiveResetBarrier { radio, barrier },
                )
            }
            HciClassifiedCommandRoute::EndpointMismatch { published, command } => {
                BluetoothDtmActiveControllerCommandRoute::EndpointMismatch {
                    session: BluetoothDtmActiveSession {
                        radio,
                        order: BluetoothDtmResponsePublished {
                            transaction: published,
                        },
                    },
                    command,
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Borrow the exact current radio wait without moving either affine axis.
    pub fn radio_wait(&self) -> Option<BluetoothDtmActiveRadioWait<'_>> {
        match &self.radio {
            BluetoothDtmActiveRadio::SchedulerWait(wait) => {
                Some(BluetoothDtmActiveRadioWait::Scheduler(wait.wake()))
            }
            BluetoothDtmActiveRadio::PostUnlinkWait(wait) => {
                Some(BluetoothDtmActiveRadioWait::PostUnlink(wait.wake()))
            }
            BluetoothDtmActiveRadio::ControllerTimeWait(_) => {
                Some(BluetoothDtmActiveRadioWait::ControllerTime)
            }
            BluetoothDtmActiveRadio::Completion(_)
            | BluetoothDtmActiveRadio::Recurring(_)
            | BluetoothDtmActiveRadio::Retryable(_) => None,
        }
    }

    /// Borrow the finite recurring retry cause without separating either axis.
    pub fn recurring_retry_cause(&self) -> Option<&BluetoothDtmRecurringRetryCause<S::Error>> {
        match &self.radio {
            BluetoothDtmActiveRadio::Retryable(retry) => Some(retry.cause()),
            _ => None,
        }
    }

    /// Advance exactly one radio edge while carrying the HCI-order axis unchanged.
    ///
    /// Calling this on a parked wait performs one bounded recheck. Calling it
    /// on a retryable owner performs the explicit retry selected by the caller.
    pub fn step_radio(self) -> BluetoothDtmActiveSessionRadioStep<'runtime, S, CAPACITY, Order> {
        match self.radio {
            BluetoothDtmActiveRadio::Completion(completion) => {
                step_completion(self.order, completion)
            }
            BluetoothDtmActiveRadio::SchedulerWait(wait) => {
                let _observed = wait.wake().take();
                step_completion(self.order, wait.resume())
            }
            BluetoothDtmActiveRadio::PostUnlinkWait(wait) => {
                step_completion(self.order, wait.resume())
            }
            BluetoothDtmActiveRadio::Recurring(recurring) => step_recurring(self.order, recurring),
            BluetoothDtmActiveRadio::ControllerTimeWait(wait) => {
                step_recurring(self.order, wait.resume())
            }
            BluetoothDtmActiveRadio::Retryable(retry) => step_recurring(self.order, retry.retry()),
        }
    }
}

fn step_completion<'runtime, S, const CAPACITY: usize, Order>(
    order: Order,
    completion: BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>,
) -> BluetoothDtmActiveSessionRadioStep<'runtime, S, CAPACITY, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match completion.step() {
        BluetoothDtmActiveCompletionStep::Continue(completion) => {
            BluetoothDtmActiveSessionRadioStep::Continue(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::Completion(completion),
                order,
            })
        }
        BluetoothDtmActiveCompletionStep::WaitScheduler(wait) => {
            BluetoothDtmActiveSessionRadioStep::Waiting(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::SchedulerWait(wait),
                order,
            })
        }
        BluetoothDtmActiveCompletionStep::UnrelatedList {
            completion,
            observed,
        } => BluetoothDtmActiveSessionRadioStep::UnrelatedList {
            session: BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::Completion(completion),
                order,
            },
            observed,
        },
        BluetoothDtmActiveCompletionStep::WaitPostUnlink(wait) => {
            BluetoothDtmActiveSessionRadioStep::Waiting(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::PostUnlinkWait(wait),
                order,
            })
        }
        BluetoothDtmActiveCompletionStep::CpuOwned(owner) => {
            BluetoothDtmActiveSessionRadioStep::Continue(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::Recurring(owner.begin_recurring()),
                order,
            })
        }
        BluetoothDtmActiveCompletionStep::Fault(fault) => {
            BluetoothDtmActiveSessionRadioStep::Fault(BluetoothDtmActiveSessionFault {
                role: fault.role(),
                cause: BluetoothDtmActiveSessionFaultCause::Completion(fault.cause()),
                _radio: BluetoothDtmActiveSessionFaultOwner::Completion(fault),
                _order: order,
            })
        }
    }
}

fn step_recurring<'runtime, S, const CAPACITY: usize, Order>(
    order: Order,
    recurring: BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>,
) -> BluetoothDtmActiveSessionRadioStep<'runtime, S, CAPACITY, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match recurring.step() {
        BluetoothDtmRecurringRunnerStep::Continue(recurring) => {
            BluetoothDtmActiveSessionRadioStep::Continue(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::Recurring(recurring),
                order,
            })
        }
        BluetoothDtmRecurringRunnerStep::WaitControllerTime(wait) => {
            BluetoothDtmActiveSessionRadioStep::Waiting(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::ControllerTimeWait(wait),
                order,
            })
        }
        BluetoothDtmRecurringRunnerStep::Running(completion) => {
            BluetoothDtmActiveSessionRadioStep::Continue(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::Completion(completion),
                order,
            })
        }
        BluetoothDtmRecurringRunnerStep::Retryable(retry) => {
            BluetoothDtmActiveSessionRadioStep::Retryable(BluetoothDtmActiveSession {
                radio: BluetoothDtmActiveRadio::Retryable(retry),
                order,
            })
        }
        BluetoothDtmRecurringRunnerStep::Fault(fault) => {
            BluetoothDtmActiveSessionRadioStep::Fault(BluetoothDtmActiveSessionFault {
                role: fault.role(),
                cause: BluetoothDtmActiveSessionFaultCause::Recurring(fault.cause()),
                _radio: BluetoothDtmActiveSessionFaultOwner::Recurring(fault),
                _order: order,
            })
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmFirstRunning<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Split first `RUN` into independently progressing radio and HCI-order axes.
    pub fn into_active_session(self) -> BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY> {
        let (response, hci_epoch, completion) =
            BluetoothDtmActiveCompletion::from_first_running(self.into_parts());
        BluetoothDtmActiveSession {
            radio: BluetoothDtmActiveRadio::Completion(completion),
            order: BluetoothDtmResponsePending {
                transaction: HciResponsePending::new((), response, hci_epoch),
            },
        }
    }
}
