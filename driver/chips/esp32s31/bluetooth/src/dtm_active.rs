//! Executor-neutral completion of one active DTM scheduler event.
//!
//! Every operation advances exactly one lower ownership transition. Waiting
//! states retain the complete Controller and graph outside an executor future;
//! interrupt service remains the responsibility of the disjoint published ISR
//! endpoint.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemCompletionStatus;

use crate::dtm_runner::BluetoothDtmFirstRunningParts;
use crate::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginError,
    BluetoothControllerSchedulerCurrentBeginFailure, BluetoothControllerSchedulerCurrentError,
    BluetoothControllerSchedulerCurrentFailure, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothControllerSchedulerEpochRetained,
    BluetoothControllerSchedulerNowReady, BluetoothControllerTimeOrphanDrainStep,
    BluetoothDtmActiveReceiverCpuOwned, BluetoothDtmActiveTransmitterCpuOwned,
    BluetoothDtmControllerEventPreparationError, BluetoothDtmControllerPreparationOutcome,
    BluetoothDtmControllerPreparationPending, BluetoothDtmControllerPreparationStep,
    BluetoothDtmControllerPreparationTerminal, BluetoothDtmEmptySchedulerMergePrepared,
    BluetoothDtmPostUnlinkArmStep, BluetoothDtmPostUnlinkAwaiting, BluetoothDtmReceiverEvent,
    BluetoothDtmRecurringSchedulerItemPhase, BluetoothDtmRole, BluetoothDtmRxCompletionOutcome,
    BluetoothDtmSchedulerCompletionObserved, BluetoothDtmSchedulerCompletionObservedDrainStep,
    BluetoothDtmSchedulerCompletionStep, BluetoothDtmSchedulerHardwareHeadEmptyObserved,
    BluetoothDtmSchedulerHardwareHeadRetirementStep, BluetoothDtmSchedulerHeadPublished,
    BluetoothDtmSchedulerRecycleStep, BluetoothDtmSchedulerRunning,
    BluetoothDtmSchedulerRunningDrainStep, BluetoothDtmSchedulerRxSuccessRecycleStep,
    BluetoothDtmSchedulerSoftwareListRemovalReady, BluetoothDtmSoftwareListRemovalPublishedStep,
    BluetoothDtmTransmitterEvent, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerFinishedListDrainPending, BluetoothSchedulerFinishedListDrainState,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerWakeBatch,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

enum BluetoothDtmRoleCompletionPhase<'runtime, S, const CAPACITY: usize, Role> {
    RunningAwaitingWake {
        task: Task<'runtime, S, CAPACITY>,
        running: BluetoothDtmSchedulerRunning<Role>,
    },
    RunningReady {
        task: Task<'runtime, S, CAPACITY>,
        running: BluetoothDtmSchedulerRunning<Role>,
        wake: BluetoothSchedulerWakeBatch,
    },
    RunningDrain {
        task: Task<'runtime, S, CAPACITY>,
        pending: BluetoothSchedulerFinishedListDrainPending<BluetoothDtmSchedulerRunning<Role>>,
    },
    CompletionDrain {
        task: Task<'runtime, S, CAPACITY>,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
    },
    CompletionObserved {
        task: Task<'runtime, S, CAPACITY>,
        completed: BluetoothDtmSchedulerCompletionObserved<Role>,
    },
    HardwareHeadEmpty {
        task: Task<'runtime, S, CAPACITY>,
        observed: BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>,
    },
    PostUnlinkAwaiting {
        task: Task<'runtime, S, CAPACITY>,
        awaiting: BluetoothDtmPostUnlinkAwaiting<Role>,
    },
    RemovalReady {
        task: Task<'runtime, S, CAPACITY>,
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    },
}

enum BluetoothDtmActiveCompletionPhase<'runtime, S, const CAPACITY: usize> {
    Transmitter(
        BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, BluetoothDtmTransmitterEvent>,
    ),
    Receiver(BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, BluetoothDtmReceiverEvent>),
}

/// Executor-neutral owner of one active TX or RX scheduler completion.
#[must_use = "the active DTM graph must reach a wait, CPU ownership or an opaque fault"]
pub struct BluetoothDtmActiveCompletion<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmActiveCompletionPhase<'runtime, S, CAPACITY>,
}

/// One bounded active-completion transition.
#[must_use = "retain the active completion owner and every unrelated list observation"]
pub enum BluetoothDtmActiveCompletionStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The previous transition completed and another transition may run now.
    Continue(BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>),
    /// No completion is currently available; wait for a scheduler notification.
    WaitScheduler(BluetoothDtmActiveSchedulerWait<'runtime, S, CAPACITY>),
    /// One unrelated hardware list remains owned by its external dispatcher.
    UnrelatedList {
        completion: BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// The graph is unlinked and awaits a later published primary event.
    WaitPostUnlink(BluetoothDtmActivePostUnlinkWait<'runtime, S, CAPACITY>),
    /// Memory and timeline ownership returned to the active role.
    CpuOwned(BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>),
    /// A fail-closed lower transition retained every affine owner opaquely.
    Fault(BluetoothDtmActiveCompletionFault<'runtime, S, CAPACITY>),
}

/// Parked scheduler-completion owner with the only relevant durable wake source.
#[must_use = "register the wake, recheck, or retain the complete active owner"]
pub struct BluetoothDtmActiveSchedulerWait<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    completion: BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveSchedulerWait<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Durable scheduler wake belonging to this exact Controller epoch.
    pub fn wake(&self) -> &crate::BluetoothSchedulerWakeCell {
        match &self.completion.phase {
            BluetoothDtmActiveCompletionPhase::Transmitter(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, .. },
            )
            | BluetoothDtmActiveCompletionPhase::Receiver(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, .. },
            ) => task.scheduler_wake(),
            _ => unreachable!("scheduler wait retains a running graph"),
        }
    }

    /// Consume the parked wait and the exact dequeued scheduler batch before
    /// performing one fresh finished-list transfer.
    pub fn resume(
        self,
        wake: BluetoothSchedulerWakeBatch,
    ) -> BluetoothDtmActiveCompletion<'runtime, S, CAPACITY> {
        let phase = match self.completion.phase {
            BluetoothDtmActiveCompletionPhase::Transmitter(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ) => BluetoothDtmActiveCompletionPhase::Transmitter(
                BluetoothDtmRoleCompletionPhase::RunningReady {
                    task,
                    running,
                    wake,
                },
            ),
            BluetoothDtmActiveCompletionPhase::Receiver(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ) => BluetoothDtmActiveCompletionPhase::Receiver(
                BluetoothDtmRoleCompletionPhase::RunningReady {
                    task,
                    running,
                    wake,
                },
            ),
            _ => unreachable!("scheduler wait retains a wake-gated running graph"),
        };
        BluetoothDtmActiveCompletion { phase }
    }
}

/// Parked post-unlink owner with its exact mailbox notification source.
#[must_use = "register the wake, recheck, or retain the armed post-unlink owner"]
pub struct BluetoothDtmActivePostUnlinkWait<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    completion: BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActivePostUnlinkWait<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Durable notification for the exact armed post-unlink mailbox.
    pub fn wake(&self) -> &crate::BluetoothDtmPostUnlinkWakeCell {
        match &self.completion.phase {
            BluetoothDtmActiveCompletionPhase::Transmitter(
                BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { task, .. },
            )
            | BluetoothDtmActiveCompletionPhase::Receiver(
                BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { task, .. },
            ) => task.post_unlink_wake(),
            _ => unreachable!("post-unlink wait retains an armed mailbox owner"),
        }
    }

    /// Consume the parked wait before performing one later bounded mailbox take.
    pub fn resume(self) -> BluetoothDtmActiveCompletion<'runtime, S, CAPACITY> {
        self.completion
    }
}

/// CPU-owned active role after complete scheduler removal and recycle.
#[must_use = "the active role must recur or enter proven terminal quiescence"]
pub enum BluetoothDtmActiveCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter(BluetoothDtmActiveTransmitterReady<'runtime, S, CAPACITY>),
    Receiver(BluetoothDtmActiveReceiverReady<'runtime, S, CAPACITY>),
}

/// Exact Controller and active TX graph at the only CPU-owned command boundary.
#[must_use = "the transmitter must recur or enter proven terminal quiescence"]
pub struct BluetoothDtmActiveTransmitterReady<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _task: Task<'runtime, S, CAPACITY>,
    owner: BluetoothDtmActiveTransmitterCpuOwned,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveTransmitterReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.owner.status()
    }

    pub const fn packet_pattern(&self) -> crate::BluetoothDtmPayloadPattern {
        self.owner.packet_pattern()
    }

    pub const fn payload_length(&self) -> crate::BluetoothDtmPayloadLength {
        self.owner.packet_length()
    }

    #[allow(
        dead_code,
        reason = "the recurring runner consumes this clean CPU boundary in the next slice"
    )]
    pub(crate) fn into_parts(
        self,
    ) -> (
        Task<'runtime, S, CAPACITY>,
        BluetoothDtmActiveTransmitterCpuOwned,
    ) {
        (self._task, self.owner)
    }
}

/// Exact Controller and active RX graph at the only CPU-owned command boundary.
#[must_use = "the receiver must recur or enter proven terminal quiescence"]
pub struct BluetoothDtmActiveReceiverReady<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _task: Task<'runtime, S, CAPACITY>,
    owner: BluetoothDtmActiveReceiverCpuOwned,
    status: BluetoothDtmSchedulerItemCompletionStatus,
    outcome: Option<BluetoothDtmRxCompletionOutcome>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveReceiverReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.status
    }

    pub const fn outcome(&self) -> Option<BluetoothDtmRxCompletionOutcome> {
        self.outcome
    }

    pub const fn received_packet_count(&self) -> u16 {
        self.owner.received_packet_count()
    }

    #[allow(
        dead_code,
        reason = "the recurring runner consumes this clean CPU boundary in the next slice"
    )]
    pub(crate) fn into_parts(
        self,
    ) -> (
        Task<'runtime, S, CAPACITY>,
        BluetoothDtmActiveReceiverCpuOwned,
        BluetoothDtmSchedulerItemCompletionStatus,
        Option<BluetoothDtmRxCompletionOutcome>,
    ) {
        (self._task, self.owner, self.status, self.outcome)
    }
}

/// Finite fail-closed reason retained by an opaque active-completion owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmActiveCompletionFaultCause {
    FinishedListDrainAlreadyActive,
    SchedulerIdentityMismatch,
    FinishedListDrainLost,
    RepeatedDtmList,
    FinishedListDrainStillActive,
    ExpectedHardwareHeadStillPublished,
    UnexpectedHardwareHeadChanged,
    PostUnlinkMailboxBusy,
    PostUnlinkMailboxIdentityExhausted,
    PostUnlinkMailboxGenerationExhausted,
    PostUnlinkMailboxCommitMismatch,
    PostUnlinkMailboxAffinityMismatch,
    PrimaryInterruptFault,
    PostUnlinkNoSchedulerWorkRearmMismatch,
    PostUnlinkPendingRearmMismatch,
    PostUnlinkRecheckUnavailable,
    PostUnlinkRecheckRearmMismatch,
    MemoryIdentityMismatch,
    ReservationIdentityMismatch,
    ReceiverCompletionStatusMismatch,
    ReceiverReturnedTopologyRejected,
    ReceiverSpecializedRecycleRequired,
}

#[allow(
    dead_code,
    reason = "opaque fault ownership intentionally prevents graph recovery after fail-stop"
)]
enum BluetoothDtmRoleCompletionFault<'runtime, S, const CAPACITY: usize, Role> {
    Completion {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSchedulerCompletionStep<Role>,
    },
    RunningDrain {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSchedulerRunningDrainStep<Role>,
    },
    CompletionDrain {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSchedulerCompletionObservedDrainStep<Role>,
    },
    HardwareHeadRetirement {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSchedulerHardwareHeadRetirementStep<Role>,
    },
    PostUnlinkArm {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmPostUnlinkArmStep<Role>,
    },
    PostUnlinkPublished {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSoftwareListRemovalPublishedStep<Role>,
    },
    Recycle {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSchedulerRecycleStep<Role>,
    },
}

#[allow(
    dead_code,
    reason = "opaque fault ownership intentionally retains every affine lower token"
)]
enum BluetoothDtmActiveCompletionFaultOwner<'runtime, S, const CAPACITY: usize> {
    Transmitter(
        BluetoothDtmRoleCompletionFault<'runtime, S, CAPACITY, BluetoothDtmTransmitterEvent>,
    ),
    Receiver(BluetoothDtmRoleCompletionFault<'runtime, S, CAPACITY, BluetoothDtmReceiverEvent>),
    ReceiverSuccess {
        task: Task<'runtime, S, CAPACITY>,
        _step: BluetoothDtmSchedulerRxSuccessRecycleStep,
    },
}

/// Opaque fail-stop owner retaining every lower graph, event and Controller token.
#[must_use = "the exact fault owner must remain retained for diagnostic shutdown"]
pub struct BluetoothDtmActiveCompletionFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    role: BluetoothDtmRole,
    cause: BluetoothDtmActiveCompletionFaultCause,
    _owner: BluetoothDtmActiveCompletionFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothDtmActiveCompletionFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Role of the exact graph retained by this fault owner.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.role
    }

    pub const fn cause(&self) -> BluetoothDtmActiveCompletionFaultCause {
        self.cause
    }
}

enum BluetoothDtmRoleCompletionAdvance<'runtime, S, const CAPACITY: usize, Role> {
    Continue(BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>),
    WaitScheduler(BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>),
    UnrelatedList {
        phase: BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    WaitPostUnlink(BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>),
    Recycle {
        task: Task<'runtime, S, CAPACITY>,
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    },
    Fault {
        cause: BluetoothDtmActiveCompletionFaultCause,
        owner: BluetoothDtmRoleCompletionFault<'runtime, S, CAPACITY, Role>,
    },
}

fn drained_or_pending_running<'runtime, S, const CAPACITY: usize, Role>(
    task: Task<'runtime, S, CAPACITY>,
    drain: BluetoothSchedulerFinishedListDrainState<BluetoothDtmSchedulerRunning<Role>>,
) -> BluetoothDtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role> {
    match drain {
        BluetoothSchedulerFinishedListDrainState::Drained(running) => {
            BluetoothDtmRoleCompletionAdvance::WaitScheduler(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            )
        }
        BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothDtmRoleCompletionAdvance::Continue(
                BluetoothDtmRoleCompletionPhase::RunningDrain { task, pending },
            )
        }
    }
}

fn drained_or_pending_completed<'runtime, S, const CAPACITY: usize, Role>(
    task: Task<'runtime, S, CAPACITY>,
    drain: BluetoothSchedulerFinishedListDrainState<BluetoothDtmSchedulerCompletionObserved<Role>>,
) -> BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role> {
    match drain {
        BluetoothSchedulerFinishedListDrainState::Drained(completed) => {
            BluetoothDtmRoleCompletionPhase::CompletionObserved { task, completed }
        }
        BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothDtmRoleCompletionPhase::CompletionDrain { task, pending }
        }
    }
}

fn step_role<'runtime, S, const CAPACITY: usize, Role>(
    phase: BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
) -> BluetoothDtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match phase {
        BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running } => {
            BluetoothDtmRoleCompletionAdvance::WaitScheduler(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            )
        }
        BluetoothDtmRoleCompletionPhase::RunningReady {
            mut task,
            running,
            wake,
        } => {
            let step = task.observe_dtm_completion(running, wake);
            match step {
                BluetoothDtmSchedulerCompletionStep::DrainAlreadyActive(running) => {
                    let step = BluetoothDtmSchedulerCompletionStep::DrainAlreadyActive(running);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause:
                            BluetoothDtmActiveCompletionFaultCause::FinishedListDrainAlreadyActive,
                        owner: BluetoothDtmRoleCompletionFault::Completion { task, _step: step },
                    }
                }
                BluetoothDtmSchedulerCompletionStep::SchedulerIdentityMismatch(running) => {
                    let step =
                        BluetoothDtmSchedulerCompletionStep::SchedulerIdentityMismatch(running);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::Completion { task, _step: step },
                    }
                }
                BluetoothDtmSchedulerCompletionStep::NoFinishedList(running) => {
                    BluetoothDtmRoleCompletionAdvance::WaitScheduler(
                        BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
                    )
                }
                BluetoothDtmSchedulerCompletionStep::UnrelatedList { drain, observed } => {
                    let advance = drained_or_pending_running(task, drain);
                    let phase = match advance {
                        BluetoothDtmRoleCompletionAdvance::Continue(phase)
                        | BluetoothDtmRoleCompletionAdvance::WaitScheduler(phase) => phase,
                        _ => unreachable!(),
                    };
                    BluetoothDtmRoleCompletionAdvance::UnrelatedList { phase, observed }
                }
                BluetoothDtmSchedulerCompletionStep::StillInFlight(drain) => {
                    drained_or_pending_running(task, drain)
                }
                BluetoothDtmSchedulerCompletionStep::CompletionObserved(drain) => {
                    BluetoothDtmRoleCompletionAdvance::Continue(drained_or_pending_completed(
                        task, drain,
                    ))
                }
            }
        }
        BluetoothDtmRoleCompletionPhase::RunningDrain { mut task, pending } => {
            let step = task.continue_dtm_running_finished_list_drain(pending);
            match step {
                BluetoothDtmSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending) => {
                    let step =
                        BluetoothDtmSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::RunningDrain { task, _step: step },
                    }
                }
                BluetoothDtmSchedulerRunningDrainStep::DrainLost(pending) => {
                    let step = BluetoothDtmSchedulerRunningDrainStep::DrainLost(pending);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::FinishedListDrainLost,
                        owner: BluetoothDtmRoleCompletionFault::RunningDrain { task, _step: step },
                    }
                }
                BluetoothDtmSchedulerRunningDrainStep::UnrelatedList { drain, observed } => {
                    let advance = drained_or_pending_running(task, drain);
                    let phase = match advance {
                        BluetoothDtmRoleCompletionAdvance::Continue(phase)
                        | BluetoothDtmRoleCompletionAdvance::WaitScheduler(phase) => phase,
                        _ => unreachable!(),
                    };
                    BluetoothDtmRoleCompletionAdvance::UnrelatedList { phase, observed }
                }
                BluetoothDtmSchedulerRunningDrainStep::StillInFlight(drain) => {
                    drained_or_pending_running(task, drain)
                }
                BluetoothDtmSchedulerRunningDrainStep::CompletionObserved(drain) => {
                    BluetoothDtmRoleCompletionAdvance::Continue(drained_or_pending_completed(
                        task, drain,
                    ))
                }
            }
        }
        BluetoothDtmRoleCompletionPhase::CompletionDrain { mut task, pending } => {
            let step = task.continue_dtm_completed_finished_list_drain(pending);
            match step {
                BluetoothDtmSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                    pending,
                ) => {
                    let step =
                        BluetoothDtmSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                            pending,
                        );
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::CompletionDrain {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSchedulerCompletionObservedDrainStep::DrainLost(pending) => {
                    let step = BluetoothDtmSchedulerCompletionObservedDrainStep::DrainLost(pending);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::FinishedListDrainLost,
                        owner: BluetoothDtmRoleCompletionFault::CompletionDrain {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSchedulerCompletionObservedDrainStep::UnrelatedList {
                    drain,
                    observed,
                } => BluetoothDtmRoleCompletionAdvance::UnrelatedList {
                    phase: drained_or_pending_completed(task, drain),
                    observed,
                },
                BluetoothDtmSchedulerCompletionObservedDrainStep::RepeatedDtmList {
                    drain,
                    observed,
                } => {
                    let step = BluetoothDtmSchedulerCompletionObservedDrainStep::RepeatedDtmList {
                        drain,
                        observed,
                    };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::RepeatedDtmList,
                        owner: BluetoothDtmRoleCompletionFault::CompletionDrain {
                            task,
                            _step: step,
                        },
                    }
                }
            }
        }
        BluetoothDtmRoleCompletionPhase::CompletionObserved {
            mut task,
            completed,
        } => {
            let step = task.observe_dtm_hardware_head_retirement(completed);
            match step {
                BluetoothDtmSchedulerHardwareHeadRetirementStep::EmptyObserved(observed) => {
                    BluetoothDtmRoleCompletionAdvance::Continue(
                        BluetoothDtmRoleCompletionPhase::HardwareHeadEmpty { task, observed },
                    )
                }
                BluetoothDtmSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                    completed,
                ) => {
                    let step =
                        BluetoothDtmSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                            completed,
                        );
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::HardwareHeadRetirement {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                    completed,
                ) => {
                    let step = BluetoothDtmSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::FinishedListDrainStillActive,
                        owner: BluetoothDtmRoleCompletionFault::HardwareHeadRetirement {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                    completed,
                    observed,
                } => {
                    let step = BluetoothDtmSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                        completed,
                        observed,
                    };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::ExpectedHardwareHeadStillPublished,
                        owner: BluetoothDtmRoleCompletionFault::HardwareHeadRetirement { task, _step: step },
                    }
                }
                BluetoothDtmSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                    completed,
                    observed,
                } => {
                    let step =
                        BluetoothDtmSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                            completed,
                            observed,
                        };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause:
                            BluetoothDtmActiveCompletionFaultCause::UnexpectedHardwareHeadChanged,
                        owner: BluetoothDtmRoleCompletionFault::HardwareHeadRetirement {
                            task,
                            _step: step,
                        },
                    }
                }
            }
        }
        BluetoothDtmRoleCompletionPhase::HardwareHeadEmpty { mut task, observed } => {
            let step = task.unlink_and_arm_dtm_software_list_removal(observed);
            match step {
                BluetoothDtmPostUnlinkArmStep::Armed(awaiting) => {
                    BluetoothDtmRoleCompletionAdvance::Continue(
                        BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { task, awaiting },
                    )
                }
                BluetoothDtmPostUnlinkArmStep::MailboxBusy(observed) => {
                    let step = BluetoothDtmPostUnlinkArmStep::MailboxBusy(observed);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PostUnlinkMailboxBusy,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                BluetoothDtmPostUnlinkArmStep::MailboxIdentityExhausted(observed) => {
                    let step = BluetoothDtmPostUnlinkArmStep::MailboxIdentityExhausted(observed);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PostUnlinkMailboxIdentityExhausted,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                BluetoothDtmPostUnlinkArmStep::GenerationExhausted(observed) => {
                    let step = BluetoothDtmPostUnlinkArmStep::GenerationExhausted(observed);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PostUnlinkMailboxGenerationExhausted,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                BluetoothDtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed) => {
                    let step = BluetoothDtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                BluetoothDtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked) => {
                    let step = BluetoothDtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked);
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause:
                            BluetoothDtmActiveCompletionFaultCause::PostUnlinkMailboxCommitMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
            }
        }
        BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { mut task, awaiting } => {
            let step = task.consume_published_dtm_software_list_removal(awaiting);
            match step {
                BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWork {
                    awaiting,
                    epoch: _,
                }
                | BluetoothDtmSoftwareListRemovalPublishedStep::PublishedPending { awaiting } => {
                    BluetoothDtmRoleCompletionAdvance::Continue(
                        BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { task, awaiting },
                    )
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::DirectPending { awaiting } => {
                    BluetoothDtmRoleCompletionAdvance::WaitPostUnlink(
                        BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { task, awaiting },
                    )
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::Ready { ready } => {
                    BluetoothDtmRoleCompletionAdvance::Continue(
                        BluetoothDtmRoleCompletionPhase::RemovalReady { task, ready },
                    )
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting) => {
                    let step =
                        BluetoothDtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                            awaiting,
                        );
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PostUnlinkMailboxAffinityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault } => {
                    let step =
                        BluetoothDtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PrimaryInterruptFault,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                    unlinked,
                    epoch,
                } => {
                    let step = BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { unlinked, epoch };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked } => {
                    let step = BluetoothDtmSoftwareListRemovalPublishedStep::PendingRearmMismatch {
                        unlinked,
                    };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause:
                            BluetoothDtmActiveCompletionFaultCause::PostUnlinkPendingRearmMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting } => {
                    let step = BluetoothDtmSoftwareListRemovalPublishedStep::RecheckUnavailable {
                        awaiting,
                    };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::PostUnlinkRecheckUnavailable,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked } => {
                    let step = BluetoothDtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                        unlinked,
                    };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause:
                            BluetoothDtmActiveCompletionFaultCause::PostUnlinkRecheckRearmMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                    unlinked,
                    event,
                } => {
                    let step =
                        BluetoothDtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished {
                            task,
                            _step: step,
                        },
                    }
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                    unlinked,
                } => {
                    let step = BluetoothDtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                        unlinked,
                    };
                    BluetoothDtmRoleCompletionAdvance::Fault {
                        cause: BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: BluetoothDtmRoleCompletionFault::PostUnlinkPublished {
                            task,
                            _step: step,
                        },
                    }
                }
            }
        }
        BluetoothDtmRoleCompletionPhase::RemovalReady { task, ready } => {
            BluetoothDtmRoleCompletionAdvance::Recycle { task, ready }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn from_first_running(
        parts: BluetoothDtmFirstRunningParts<'runtime, S, CAPACITY>,
    ) -> (
        open_esp_radio_bluetooth_hci::LeControllerResponsePending<'runtime, ()>,
        Self,
    ) {
        match parts {
            BluetoothDtmFirstRunningParts::Transmitter { response, running } => {
                let (task, response) = response.into_parts();
                (response, Self::from_transmitter_running(task, running))
            }
            BluetoothDtmFirstRunningParts::Receiver { response, running } => {
                let (task, response) = response.into_parts();
                (response, Self::from_receiver_running(task, running))
            }
        }
    }

    fn from_transmitter_running(
        task: Task<'runtime, S, CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmTransmitterEvent>,
    ) -> Self {
        Self {
            phase: BluetoothDtmActiveCompletionPhase::Transmitter(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ),
        }
    }

    fn from_receiver_running(
        task: Task<'runtime, S, CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmReceiverEvent>,
    ) -> Self {
        Self {
            phase: BluetoothDtmActiveCompletionPhase::Receiver(
                BluetoothDtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ),
        }
    }

    /// Role of the exact graph retained by this completion owner.
    pub const fn role(&self) -> BluetoothDtmRole {
        match self.phase {
            BluetoothDtmActiveCompletionPhase::Transmitter(_) => BluetoothDtmRole::Transmitter,
            BluetoothDtmActiveCompletionPhase::Receiver(_) => BluetoothDtmRole::Receiver,
        }
    }

    /// Advance exactly one lower completion, drain, unlink, mailbox or recycle edge.
    pub fn step(self) -> BluetoothDtmActiveCompletionStep<'runtime, S, CAPACITY> {
        match self.phase {
            BluetoothDtmActiveCompletionPhase::Transmitter(phase) => {
                map_transmitter_advance(step_role(phase))
            }
            BluetoothDtmActiveCompletionPhase::Receiver(phase) => {
                map_receiver_advance(step_role(phase))
            }
        }
    }
}

fn map_transmitter_advance<'runtime, S, const CAPACITY: usize>(
    advance: BluetoothDtmRoleCompletionAdvance<'runtime, S, CAPACITY, BluetoothDtmTransmitterEvent>,
) -> BluetoothDtmActiveCompletionStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    map_role_advance(
        advance,
        BluetoothDtmRole::Transmitter,
        BluetoothDtmActiveCompletionPhase::Transmitter,
        BluetoothDtmActiveCompletionFaultOwner::Transmitter,
        |mut task, ready| match task.recycle_dtm_completed(ready) {
            BluetoothDtmSchedulerRecycleStep::Recycled(recycled) => {
                BluetoothDtmActiveCompletionStep::CpuOwned(BluetoothDtmActiveCpuOwned::Transmitter(
                    BluetoothDtmActiveTransmitterReady {
                        _task: task,
                        owner: recycled.into_next(),
                    },
                ))
            }
            step @ (BluetoothDtmSchedulerRecycleStep::SchedulerIdentityMismatch(_)
            | BluetoothDtmSchedulerRecycleStep::FinishedListDrainStillActive(_)
            | BluetoothDtmSchedulerRecycleStep::MemoryIdentityMismatch { .. }
            | BluetoothDtmSchedulerRecycleStep::ReservationIdentityMismatch(_)
            | BluetoothDtmSchedulerRecycleStep::ReceiverSuccessRequiresSpecializedRecycle(
                _,
            )) => {
                let cause = recycle_fault_cause(&step);
                BluetoothDtmActiveCompletionStep::Fault(BluetoothDtmActiveCompletionFault {
                    role: BluetoothDtmRole::Transmitter,
                    cause,
                    _owner: BluetoothDtmActiveCompletionFaultOwner::Transmitter(
                        BluetoothDtmRoleCompletionFault::Recycle { task, _step: step },
                    ),
                })
            }
        },
    )
}

fn map_receiver_advance<'runtime, S, const CAPACITY: usize>(
    advance: BluetoothDtmRoleCompletionAdvance<'runtime, S, CAPACITY, BluetoothDtmReceiverEvent>,
) -> BluetoothDtmActiveCompletionStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    map_role_advance(
        advance,
        BluetoothDtmRole::Receiver,
        BluetoothDtmActiveCompletionPhase::Receiver,
        BluetoothDtmActiveCompletionFaultOwner::Receiver,
        |mut task, ready| {
            let status = ready.status();
            if status == BluetoothDtmSchedulerItemCompletionStatus::Zero {
                match task.recycle_dtm_receiver_success(ready) {
                    BluetoothDtmSchedulerRxSuccessRecycleStep::Rearmed(rearmed) => {
                        let outcome = rearmed.outcome();
                        BluetoothDtmActiveCompletionStep::CpuOwned(
                            BluetoothDtmActiveCpuOwned::Receiver(BluetoothDtmActiveReceiverReady {
                                _task: task,
                                owner: rearmed.into_next(),
                                status,
                                outcome: Some(outcome),
                            }),
                        )
                    }
                    step => {
                        let cause = rx_success_recycle_fault_cause(&step);
                        BluetoothDtmActiveCompletionStep::Fault(BluetoothDtmActiveCompletionFault {
                            role: BluetoothDtmRole::Receiver,
                            cause,
                            _owner: BluetoothDtmActiveCompletionFaultOwner::ReceiverSuccess {
                                task,
                                _step: step,
                            },
                        })
                    }
                }
            } else {
                match task.recycle_dtm_completed(ready) {
                    BluetoothDtmSchedulerRecycleStep::Recycled(recycled) => {
                        BluetoothDtmActiveCompletionStep::CpuOwned(
                            BluetoothDtmActiveCpuOwned::Receiver(BluetoothDtmActiveReceiverReady {
                                _task: task,
                                owner: recycled.into_next(),
                                status,
                                outcome: None,
                            }),
                        )
                    }
                    step => {
                        let cause = recycle_fault_cause(&step);
                        BluetoothDtmActiveCompletionStep::Fault(BluetoothDtmActiveCompletionFault {
                            role: BluetoothDtmRole::Receiver,
                            cause,
                            _owner: BluetoothDtmActiveCompletionFaultOwner::Receiver(
                                BluetoothDtmRoleCompletionFault::Recycle { task, _step: step },
                            ),
                        })
                    }
                }
            }
        },
    )
}

fn map_role_advance<'runtime, S, const CAPACITY: usize, Role>(
    advance: BluetoothDtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role>,
    role: BluetoothDtmRole,
    phase: impl FnOnce(
        BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
    ) -> BluetoothDtmActiveCompletionPhase<'runtime, S, CAPACITY>
    + Copy,
    fault_owner: impl FnOnce(
        BluetoothDtmRoleCompletionFault<'runtime, S, CAPACITY, Role>,
    ) -> BluetoothDtmActiveCompletionFaultOwner<'runtime, S, CAPACITY>,
    recycle: impl FnOnce(
        Task<'runtime, S, CAPACITY>,
        BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> BluetoothDtmActiveCompletionStep<'runtime, S, CAPACITY>,
) -> BluetoothDtmActiveCompletionStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match advance {
        BluetoothDtmRoleCompletionAdvance::Continue(next) => {
            BluetoothDtmActiveCompletionStep::Continue(BluetoothDtmActiveCompletion {
                phase: phase(next),
            })
        }
        BluetoothDtmRoleCompletionAdvance::WaitScheduler(next) => {
            BluetoothDtmActiveCompletionStep::WaitScheduler(BluetoothDtmActiveSchedulerWait {
                completion: BluetoothDtmActiveCompletion { phase: phase(next) },
            })
        }
        BluetoothDtmRoleCompletionAdvance::UnrelatedList {
            phase: next,
            observed,
        } => BluetoothDtmActiveCompletionStep::UnrelatedList {
            completion: BluetoothDtmActiveCompletion { phase: phase(next) },
            observed,
        },
        BluetoothDtmRoleCompletionAdvance::WaitPostUnlink(next) => {
            BluetoothDtmActiveCompletionStep::WaitPostUnlink(BluetoothDtmActivePostUnlinkWait {
                completion: BluetoothDtmActiveCompletion { phase: phase(next) },
            })
        }
        BluetoothDtmRoleCompletionAdvance::Recycle { task, ready } => recycle(task, ready),
        BluetoothDtmRoleCompletionAdvance::Fault { cause, owner } => {
            BluetoothDtmActiveCompletionStep::Fault(BluetoothDtmActiveCompletionFault {
                role,
                cause,
                _owner: fault_owner(owner),
            })
        }
    }
}

fn recycle_fault_cause<Role>(
    step: &BluetoothDtmSchedulerRecycleStep<Role>,
) -> BluetoothDtmActiveCompletionFaultCause {
    match step {
        BluetoothDtmSchedulerRecycleStep::SchedulerIdentityMismatch(_) => {
            BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch
        }
        BluetoothDtmSchedulerRecycleStep::FinishedListDrainStillActive(_) => {
            BluetoothDtmActiveCompletionFaultCause::FinishedListDrainStillActive
        }
        BluetoothDtmSchedulerRecycleStep::MemoryIdentityMismatch { .. } => {
            BluetoothDtmActiveCompletionFaultCause::MemoryIdentityMismatch
        }
        BluetoothDtmSchedulerRecycleStep::ReservationIdentityMismatch(_) => {
            BluetoothDtmActiveCompletionFaultCause::ReservationIdentityMismatch
        }
        BluetoothDtmSchedulerRecycleStep::ReceiverSuccessRequiresSpecializedRecycle(_) => {
            BluetoothDtmActiveCompletionFaultCause::ReceiverSpecializedRecycleRequired
        }
        BluetoothDtmSchedulerRecycleStep::Recycled(_) => unreachable!(),
    }
}

fn rx_success_recycle_fault_cause(
    step: &BluetoothDtmSchedulerRxSuccessRecycleStep,
) -> BluetoothDtmActiveCompletionFaultCause {
    match step {
        BluetoothDtmSchedulerRxSuccessRecycleStep::SchedulerIdentityMismatch(_) => {
            BluetoothDtmActiveCompletionFaultCause::SchedulerIdentityMismatch
        }
        BluetoothDtmSchedulerRxSuccessRecycleStep::FinishedListDrainStillActive(_) => {
            BluetoothDtmActiveCompletionFaultCause::FinishedListDrainStillActive
        }
        BluetoothDtmSchedulerRxSuccessRecycleStep::CompletionStatusMismatch(_) => {
            BluetoothDtmActiveCompletionFaultCause::ReceiverCompletionStatusMismatch
        }
        BluetoothDtmSchedulerRxSuccessRecycleStep::MemoryIdentityMismatch { .. } => {
            BluetoothDtmActiveCompletionFaultCause::MemoryIdentityMismatch
        }
        BluetoothDtmSchedulerRxSuccessRecycleStep::ReturnedTopologyRejected { .. } => {
            BluetoothDtmActiveCompletionFaultCause::ReceiverReturnedTopologyRejected
        }
        BluetoothDtmSchedulerRxSuccessRecycleStep::ReservationIdentityMismatch(_) => {
            BluetoothDtmActiveCompletionFaultCause::ReservationIdentityMismatch
        }
        BluetoothDtmSchedulerRxSuccessRecycleStep::Rearmed(_) => unreachable!(),
    }
}

type BluetoothDtmRecurringTxMerge = BluetoothDtmEmptySchedulerMergePrepared<
    BluetoothDtmTransmitterEvent,
    BluetoothDtmRecurringSchedulerItemPhase,
>;
type BluetoothDtmRecurringRxMerge = BluetoothDtmEmptySchedulerMergePrepared<
    BluetoothDtmReceiverEvent,
    BluetoothDtmRecurringSchedulerItemPhase,
>;

#[derive(Clone, Copy)]
struct BluetoothDtmRecurringReceiverMetadata {
    status: BluetoothDtmSchedulerItemCompletionStatus,
    outcome: Option<BluetoothDtmRxCompletionOutcome>,
}

enum BluetoothDtmRecurringPhase<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    TransmitterCpu(BluetoothDtmActiveTransmitterReady<'runtime, S, CAPACITY>),
    ReceiverCpu(BluetoothDtmActiveReceiverReady<'runtime, S, CAPACITY>),
    TransmitterEpoch {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    ReceiverEpoch {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
    TransmitterCurrent {
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    TransmitterNow {
        current: BluetoothControllerSchedulerNowReady<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    TransmitterPreparation(BluetoothDtmControllerPreparationPending<'runtime, S, CAPACITY>),
    ReceiverPreparation {
        pending: BluetoothDtmControllerPreparationPending<'runtime, S, CAPACITY>,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
    TransmitterPrepared {
        task: Task<'runtime, S, CAPACITY>,
        merged: BluetoothDtmRecurringTxMerge,
    },
    ReceiverPrepared {
        task: Task<'runtime, S, CAPACITY>,
        merged: BluetoothDtmRecurringRxMerge,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
    TransmitterHead {
        task: Task<'runtime, S, CAPACITY>,
        head: BluetoothDtmSchedulerHeadPublished<BluetoothDtmTransmitterEvent>,
    },
    ReceiverHead {
        task: Task<'runtime, S, CAPACITY>,
        head: BluetoothDtmSchedulerHeadPublished<BluetoothDtmReceiverEvent>,
    },
}

/// Executor-neutral owner of one recurring TX or RX preparation and `RUN`.
#[must_use = "the recurring graph must reach RUN, a cooperative wait, retry or fail-stop"]
pub struct BluetoothDtmRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmRecurringPhase<'runtime, S, CAPACITY>,
}

/// Result of one bounded recurring transition.
#[must_use = "retain every recurring owner until it reaches RUN or explicit failure handling"]
pub enum BluetoothDtmRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// One ownership-only or completed lower transition permits immediate progress.
    Continue(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    /// An exact Controller-time request must be rechecked cooperatively later.
    WaitControllerTime(BluetoothDtmRecurringControllerTimeWait<'runtime, S, CAPACITY>),
    /// The recurring item reached scheduler `RUN` and re-enters active completion.
    Running(BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>),
    /// A pre-`RUN` transition returned an unchanged, safely retryable owner.
    Retryable(BluetoothDtmRecurringRetry<'runtime, S, CAPACITY>),
    /// A fail-closed transition retained every owner without exposing reconstruction.
    Fault(BluetoothDtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Parked recurring Controller-time owner outside any executor future.
#[must_use = "await one cooperative recheck opportunity, resume, or retain the exact owner"]
pub struct BluetoothDtmRecurringControllerTimeWait<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothDtmRecurringControllerTimeWait<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Consume the parked owner before one later bounded Controller-time recheck.
    pub fn resume(self) -> BluetoothDtmRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }

    /// Cancel the exact private time request instead of dropping its affine owner.
    pub fn cancel(self) -> BluetoothDtmRecurringRunnerCancel<'runtime, S, CAPACITY> {
        self.runner.cancel()
    }
}

/// Finite reason a recurring owner can be retried without reconstruction.
#[must_use = "inspect the retry cause before advancing the unchanged runner"]
pub enum BluetoothDtmRecurringRetryCause<E> {
    /// CPU-owned preparation rejected after returning the unchanged active role.
    Preparation(BluetoothDtmControllerEventPreparationError),
    /// The prepared merge remained CPU-owned because head publication was rejected.
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    /// Dynamic interrupt preparation rejected the unchanged published head.
    SchedulerStart(E),
}

/// Opaque retry owner retaining the exact role-consistent task and graph.
#[must_use = "retry or retain the exact pre-RUN owner"]
pub struct BluetoothDtmRecurringRetry<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothDtmRecurringRetryCause<S::Error>,
    runner: BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmRecurringRetry<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Exact finite rejection paired with the unchanged retry owner.
    pub const fn cause(&self) -> &BluetoothDtmRecurringRetryCause<S::Error> {
        &self.cause
    }

    /// Recover the unchanged runner for an explicit caller-selected retry.
    pub fn retry(self) -> BluetoothDtmRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }

    /// Stop recurrence without first advancing the retained retry owner.
    ///
    /// Preparation and head-publication rejection retain a cancellable CPU
    /// owner. Scheduler-start rejection retains an already published head, so
    /// the returned cancellation disposition reports `HeadPublished` instead
    /// of fabricating rollback. The terminal quiescence runner uses that
    /// distinction to finish exactly the hardware-visible event.
    pub(crate) fn cancel_for_quiescence(
        self,
    ) -> BluetoothDtmRecurringRunnerCancel<'runtime, S, CAPACITY> {
        self.runner.cancel()
    }
}

/// Fail-closed reason recurring ownership cannot advance normally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRecurringFaultCause {
    SchedulerEpochUnavailable,
    SchedulerCurrentBegin(BluetoothControllerSchedulerCurrentBeginError),
    SchedulerCurrent(BluetoothControllerSchedulerCurrentError),
    Preparation(BluetoothDtmControllerEventPreparationError),
    UnexpectedPreparationOutcome,
}

#[allow(
    dead_code,
    reason = "opaque recurring faults deliberately retain all graph and Controller owners"
)]
enum BluetoothDtmRecurringFaultOwner<'runtime, S, const CAPACITY: usize> {
    TransmitterEpochUnavailable {
        task: Task<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    ReceiverEpochUnavailable {
        task: Task<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
    TransmitterCurrentBegin {
        failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    TransmitterCurrent {
        failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    TransmitterOrphanDrain {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    ReceiverOrphanDrain {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
    TransmitterPreparation {
        task: Task<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    ReceiverPreparation {
        task: Task<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
    UnexpectedPreparation {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        outcome: BluetoothDtmControllerPreparationOutcome,
        receiver_metadata: Option<BluetoothDtmRecurringReceiverMetadata>,
    },
}

enum BluetoothDtmRecurringCancellationDrainPhase<'runtime, S, const CAPACITY: usize> {
    Transmitter {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
    },
    Receiver {
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        metadata: BluetoothDtmRecurringReceiverMetadata,
    },
}

/// Cancelled recurring time request whose abandoned latch must be drained.
#[must_use = "drain the exact orphan before reusing this Controller epoch"]
pub struct BluetoothDtmRecurringCancellationDrain<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmRecurringCancellationDrainPhase<'runtime, S, CAPACITY>,
}

/// One bounded orphan-drain observation after recurring cancellation.
#[must_use = "retain Waiting, recovered CPU ownership or the opaque fail-stop owner"]
pub enum BluetoothDtmRecurringCancellationDrainStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Waiting(BluetoothDtmRecurringCancellationDrain<'runtime, S, CAPACITY>),
    CpuOwned(BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>),
    Fault(BluetoothDtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Lossless disposition of explicit recurring cancellation.
#[must_use = "retain recovered ownership, drain the orphan or preserve an irreversible head"]
pub enum BluetoothDtmRecurringRunnerCancel<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// No Controller-time request or prepared merge remains.
    CpuOwned(BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>),
    /// A cancelled time request must be drained before the Controller is reusable.
    NeedsControllerTimeDrain(BluetoothDtmRecurringCancellationDrain<'runtime, S, CAPACITY>),
    /// Lower empty-list identity rejected pre-head cancellation unchanged.
    CancellationRejected(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    /// The scheduler head is already visible and CPU cancellation is impossible.
    HeadPublished(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    /// Cancellation found a fail-stop ownership mismatch.
    Fault(BluetoothDtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Opaque fail-stop owner for a recurring invariant or timing failure.
#[must_use = "retain the exact fail-stop owner for diagnostic shutdown"]
pub struct BluetoothDtmRecurringFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    role: BluetoothDtmRole,
    cause: BluetoothDtmRecurringFaultCause,
    _owner: BluetoothDtmRecurringFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothDtmRecurringFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn role(&self) -> BluetoothDtmRole {
        self.role
    }

    pub const fn cause(&self) -> BluetoothDtmRecurringFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Begin the exact role-specific recurring transaction without HCI policy.
    pub fn begin_recurring(self) -> BluetoothDtmRecurringRunner<'runtime, S, CAPACITY> {
        let phase = match self {
            Self::Transmitter(ready) => BluetoothDtmRecurringPhase::TransmitterCpu(ready),
            Self::Receiver(ready) => BluetoothDtmRecurringPhase::ReceiverCpu(ready),
        };
        BluetoothDtmRecurringRunner { phase }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Execute exactly one ownership, Controller-time, publication or RUN transition.
    pub fn step(self) -> BluetoothDtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        match self.phase {
            BluetoothDtmRecurringPhase::TransmitterCpu(ready) => {
                let (task, owner) = ready.into_parts();
                match task.retain_scheduler_epoch() {
                    Ok(epoch) => {
                        Self::continue_with(BluetoothDtmRecurringPhase::TransmitterEpoch {
                            epoch,
                            owner,
                        })
                    }
                    Err(unavailable) => {
                        BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
                            role: BluetoothDtmRole::Transmitter,
                            cause: BluetoothDtmRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: BluetoothDtmRecurringFaultOwner::TransmitterEpochUnavailable {
                                task: unavailable.into_task_service(),
                                owner,
                            },
                        })
                    }
                }
            }
            BluetoothDtmRecurringPhase::ReceiverCpu(ready) => {
                let (task, owner, status, outcome) = ready.into_parts();
                let metadata = BluetoothDtmRecurringReceiverMetadata { status, outcome };
                match task.retain_scheduler_epoch() {
                    Ok(epoch) => Self::continue_with(BluetoothDtmRecurringPhase::ReceiverEpoch {
                        epoch,
                        owner,
                        metadata,
                    }),
                    Err(unavailable) => {
                        BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
                            role: BluetoothDtmRole::Receiver,
                            cause: BluetoothDtmRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: BluetoothDtmRecurringFaultOwner::ReceiverEpochUnavailable {
                                task: unavailable.into_task_service(),
                                owner,
                                metadata,
                            },
                        })
                    }
                }
            }
            BluetoothDtmRecurringPhase::TransmitterEpoch { epoch, owner } => {
                match epoch.begin_fresh_scheduler_current() {
                    Ok(pending) => {
                        Self::wait_with(BluetoothDtmRecurringPhase::TransmitterCurrent {
                            pending,
                            owner,
                        })
                    }
                    Err(failure) => {
                        let cause =
                            BluetoothDtmRecurringFaultCause::SchedulerCurrentBegin(failure.error());
                        BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
                            role: BluetoothDtmRole::Transmitter,
                            cause,
                            _owner: BluetoothDtmRecurringFaultOwner::TransmitterCurrentBegin {
                                failure,
                                owner,
                            },
                        })
                    }
                }
            }
            BluetoothDtmRecurringPhase::ReceiverEpoch {
                epoch,
                owner,
                metadata,
            } => match epoch.begin_dtm_receiver_recurring_item(owner) {
                Ok(pending) => Self::wait_with(BluetoothDtmRecurringPhase::ReceiverPreparation {
                    pending,
                    metadata,
                }),
                Err(terminal) => finish_receiver_recurring(terminal, metadata),
            },
            BluetoothDtmRecurringPhase::TransmitterCurrent { pending, owner } => {
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        Self::wait_with(BluetoothDtmRecurringPhase::TransmitterCurrent {
                            pending,
                            owner,
                        })
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        Self::continue_with(BluetoothDtmRecurringPhase::TransmitterNow {
                            current,
                            owner,
                        })
                    }
                    Err(failure) => {
                        let cause =
                            BluetoothDtmRecurringFaultCause::SchedulerCurrent(failure.error());
                        BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
                            role: BluetoothDtmRole::Transmitter,
                            cause,
                            _owner: BluetoothDtmRecurringFaultOwner::TransmitterCurrent {
                                failure,
                                owner,
                            },
                        })
                    }
                }
            }
            BluetoothDtmRecurringPhase::TransmitterNow { current, owner } => {
                match current.begin_dtm_transmitter_recurring_item(owner) {
                    Ok(pending) => {
                        Self::wait_with(BluetoothDtmRecurringPhase::TransmitterPreparation(pending))
                    }
                    Err(terminal) => finish_transmitter_recurring(terminal),
                }
            }
            BluetoothDtmRecurringPhase::TransmitterPreparation(pending) => {
                match pending.recheck() {
                    BluetoothDtmControllerPreparationStep::Pending(pending) => {
                        Self::wait_with(BluetoothDtmRecurringPhase::TransmitterPreparation(pending))
                    }
                    BluetoothDtmControllerPreparationStep::Terminal(terminal) => {
                        finish_transmitter_recurring(terminal)
                    }
                }
            }
            BluetoothDtmRecurringPhase::ReceiverPreparation { pending, metadata } => {
                match pending.recheck() {
                    BluetoothDtmControllerPreparationStep::Pending(pending) => {
                        Self::wait_with(BluetoothDtmRecurringPhase::ReceiverPreparation {
                            pending,
                            metadata,
                        })
                    }
                    BluetoothDtmControllerPreparationStep::Terminal(terminal) => {
                        finish_receiver_recurring(terminal, metadata)
                    }
                }
            }
            BluetoothDtmRecurringPhase::TransmitterPrepared { mut task, merged } => {
                match task.publish_dtm_scheduler_head(merged) {
                    Ok(head) => Self::continue_with(BluetoothDtmRecurringPhase::TransmitterHead {
                        task,
                        head,
                    }),
                    Err(failure) => {
                        let cause =
                            BluetoothDtmRecurringRetryCause::HeadPublication(failure.error());
                        Self::retry_with(
                            BluetoothDtmRecurringPhase::TransmitterPrepared {
                                task,
                                merged: failure.into_merged(),
                            },
                            cause,
                        )
                    }
                }
            }
            BluetoothDtmRecurringPhase::ReceiverPrepared {
                mut task,
                merged,
                metadata,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => {
                    Self::continue_with(BluetoothDtmRecurringPhase::ReceiverHead { task, head })
                }
                Err(failure) => {
                    let cause = BluetoothDtmRecurringRetryCause::HeadPublication(failure.error());
                    Self::retry_with(
                        BluetoothDtmRecurringPhase::ReceiverPrepared {
                            task,
                            merged: failure.into_merged(),
                            metadata,
                        },
                        cause,
                    )
                }
            },
            BluetoothDtmRecurringPhase::TransmitterHead { mut task, head } => {
                match task.start_dtm_scheduler(head) {
                    Ok(running) => BluetoothDtmRecurringRunnerStep::Running(
                        BluetoothDtmActiveCompletion::from_transmitter_running(task, running),
                    ),
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::retry_with(
                            BluetoothDtmRecurringPhase::TransmitterHead { task, head },
                            BluetoothDtmRecurringRetryCause::SchedulerStart(error),
                        )
                    }
                }
            }
            BluetoothDtmRecurringPhase::ReceiverHead { mut task, head } => {
                match task.start_dtm_scheduler(head) {
                    Ok(running) => BluetoothDtmRecurringRunnerStep::Running(
                        BluetoothDtmActiveCompletion::from_receiver_running(task, running),
                    ),
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::retry_with(
                            BluetoothDtmRecurringPhase::ReceiverHead { task, head },
                            BluetoothDtmRecurringRetryCause::SchedulerStart(error),
                        )
                    }
                }
            }
        }
    }

    fn continue_with(
        phase: BluetoothDtmRecurringPhase<'runtime, S, CAPACITY>,
    ) -> BluetoothDtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        BluetoothDtmRecurringRunnerStep::Continue(Self { phase })
    }

    fn wait_with(
        phase: BluetoothDtmRecurringPhase<'runtime, S, CAPACITY>,
    ) -> BluetoothDtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        BluetoothDtmRecurringRunnerStep::WaitControllerTime(
            BluetoothDtmRecurringControllerTimeWait {
                runner: Self { phase },
            },
        )
    }

    fn retry_with(
        phase: BluetoothDtmRecurringPhase<'runtime, S, CAPACITY>,
        cause: BluetoothDtmRecurringRetryCause<S::Error>,
    ) -> BluetoothDtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        BluetoothDtmRecurringRunnerStep::Retryable(BluetoothDtmRecurringRetry {
            cause,
            runner: Self { phase },
        })
    }

    /// Cancel only while the graph is still CPU-owned or owns a cancellable time request.
    ///
    /// Once `HEAD` has been published this returns `HeadPublished` unchanged;
    /// there is no fabricated rollback from hardware-visible ownership.
    pub fn cancel(self) -> BluetoothDtmRecurringRunnerCancel<'runtime, S, CAPACITY> {
        match self.phase {
            BluetoothDtmRecurringPhase::TransmitterCpu(ready) => {
                BluetoothDtmRecurringRunnerCancel::CpuOwned(
                    BluetoothDtmActiveCpuOwned::Transmitter(ready),
                )
            }
            BluetoothDtmRecurringPhase::ReceiverCpu(ready) => {
                BluetoothDtmRecurringRunnerCancel::CpuOwned(BluetoothDtmActiveCpuOwned::Receiver(
                    ready,
                ))
            }
            BluetoothDtmRecurringPhase::TransmitterEpoch { epoch, owner } => {
                BluetoothDtmRecurringRunnerCancel::CpuOwned(
                    BluetoothDtmActiveCpuOwned::Transmitter(BluetoothDtmActiveTransmitterReady {
                        _task: epoch.into_task_service(),
                        owner,
                    }),
                )
            }
            BluetoothDtmRecurringPhase::ReceiverEpoch {
                epoch,
                owner,
                metadata,
            } => BluetoothDtmRecurringRunnerCancel::CpuOwned(BluetoothDtmActiveCpuOwned::Receiver(
                BluetoothDtmActiveReceiverReady {
                    _task: epoch.into_task_service(),
                    owner,
                    status: metadata.status,
                    outcome: metadata.outcome,
                },
            )),
            BluetoothDtmRecurringPhase::TransmitterCurrent { pending, owner } => {
                match pending.cancel() {
                    Ok(epoch) => BluetoothDtmRecurringRunnerCancel::NeedsControllerTimeDrain(
                        BluetoothDtmRecurringCancellationDrain {
                            phase: BluetoothDtmRecurringCancellationDrainPhase::Transmitter {
                                epoch,
                                owner,
                            },
                        },
                    ),
                    Err(failure) => {
                        let cause =
                            BluetoothDtmRecurringFaultCause::SchedulerCurrent(failure.error());
                        BluetoothDtmRecurringRunnerCancel::Fault(BluetoothDtmRecurringFault {
                            role: BluetoothDtmRole::Transmitter,
                            cause,
                            _owner: BluetoothDtmRecurringFaultOwner::TransmitterCurrent {
                                failure,
                                owner,
                            },
                        })
                    }
                }
            }
            BluetoothDtmRecurringPhase::TransmitterNow { current, owner } => {
                let epoch = current.into_retained_epoch();
                BluetoothDtmRecurringRunnerCancel::CpuOwned(
                    BluetoothDtmActiveCpuOwned::Transmitter(BluetoothDtmActiveTransmitterReady {
                        _task: epoch.into_task_service(),
                        owner,
                    }),
                )
            }
            BluetoothDtmRecurringPhase::TransmitterPreparation(pending) => {
                cancel_transmitter_preparation(pending.cancel())
            }
            BluetoothDtmRecurringPhase::ReceiverPreparation { pending, metadata } => {
                cancel_receiver_preparation(pending.cancel(), metadata)
            }
            BluetoothDtmRecurringPhase::TransmitterPrepared { mut task, merged } => {
                match task.cancel_dtm_transmitter_recurring_item(merged) {
                    Ok(owner) => BluetoothDtmRecurringRunnerCancel::CpuOwned(
                        BluetoothDtmActiveCpuOwned::Transmitter(
                            BluetoothDtmActiveTransmitterReady { _task: task, owner },
                        ),
                    ),
                    Err(merged) => BluetoothDtmRecurringRunnerCancel::CancellationRejected(Self {
                        phase: BluetoothDtmRecurringPhase::TransmitterPrepared { task, merged },
                    }),
                }
            }
            BluetoothDtmRecurringPhase::ReceiverPrepared {
                mut task,
                merged,
                metadata,
            } => match task.cancel_dtm_receiver_recurring_item(merged) {
                Ok(owner) => BluetoothDtmRecurringRunnerCancel::CpuOwned(
                    BluetoothDtmActiveCpuOwned::Receiver(BluetoothDtmActiveReceiverReady {
                        _task: task,
                        owner,
                        status: metadata.status,
                        outcome: metadata.outcome,
                    }),
                ),
                Err(merged) => BluetoothDtmRecurringRunnerCancel::CancellationRejected(Self {
                    phase: BluetoothDtmRecurringPhase::ReceiverPrepared {
                        task,
                        merged,
                        metadata,
                    },
                }),
            },
            phase @ (BluetoothDtmRecurringPhase::TransmitterHead { .. }
            | BluetoothDtmRecurringPhase::ReceiverHead { .. }) => {
                BluetoothDtmRecurringRunnerCancel::HeadPublished(Self { phase })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothDtmRecurringCancellationDrain<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Perform one bounded orphan-drain observation.
    pub fn step(mut self) -> BluetoothDtmRecurringCancellationDrainStep<'runtime, S, CAPACITY> {
        match &mut self.phase {
            BluetoothDtmRecurringCancellationDrainPhase::Transmitter { epoch, .. }
            | BluetoothDtmRecurringCancellationDrainPhase::Receiver { epoch, .. } => {
                match epoch.drain_abandoned_controller_time() {
                    Ok(BluetoothControllerTimeOrphanDrainStep::Waiting) => {
                        BluetoothDtmRecurringCancellationDrainStep::Waiting(self)
                    }
                    Ok(BluetoothControllerTimeOrphanDrainStep::Idle)
                    | Ok(BluetoothControllerTimeOrphanDrainStep::Drained) => self.into_cpu_owned(),
                    Err(error) => self.into_fault(error),
                }
            }
        }
    }

    fn into_cpu_owned(self) -> BluetoothDtmRecurringCancellationDrainStep<'runtime, S, CAPACITY> {
        let ready = match self.phase {
            BluetoothDtmRecurringCancellationDrainPhase::Transmitter { epoch, owner } => {
                BluetoothDtmActiveCpuOwned::Transmitter(BluetoothDtmActiveTransmitterReady {
                    _task: epoch.into_task_service(),
                    owner,
                })
            }
            BluetoothDtmRecurringCancellationDrainPhase::Receiver {
                epoch,
                owner,
                metadata,
            } => BluetoothDtmActiveCpuOwned::Receiver(BluetoothDtmActiveReceiverReady {
                _task: epoch.into_task_service(),
                owner,
                status: metadata.status,
                outcome: metadata.outcome,
            }),
        };
        BluetoothDtmRecurringCancellationDrainStep::CpuOwned(ready)
    }

    fn into_fault(
        self,
        error: BluetoothControllerSchedulerCurrentError,
    ) -> BluetoothDtmRecurringCancellationDrainStep<'runtime, S, CAPACITY> {
        let (role, owner) = match self.phase {
            BluetoothDtmRecurringCancellationDrainPhase::Transmitter { epoch, owner } => (
                BluetoothDtmRole::Transmitter,
                BluetoothDtmRecurringFaultOwner::TransmitterOrphanDrain { epoch, owner },
            ),
            BluetoothDtmRecurringCancellationDrainPhase::Receiver {
                epoch,
                owner,
                metadata,
            } => (
                BluetoothDtmRole::Receiver,
                BluetoothDtmRecurringFaultOwner::ReceiverOrphanDrain {
                    epoch,
                    owner,
                    metadata,
                },
            ),
        };
        BluetoothDtmRecurringCancellationDrainStep::Fault(BluetoothDtmRecurringFault {
            role,
            cause: BluetoothDtmRecurringFaultCause::SchedulerCurrent(error),
            _owner: owner,
        })
    }
}

fn cancel_transmitter_preparation<'runtime, S, const CAPACITY: usize>(
    terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
) -> BluetoothDtmRecurringRunnerCancel<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(Err(failure)) => {
            BluetoothDtmRecurringRunnerCancel::NeedsControllerTimeDrain(
                BluetoothDtmRecurringCancellationDrain {
                    phase: BluetoothDtmRecurringCancellationDrainPhase::Transmitter {
                        epoch,
                        owner: failure.into_owner(),
                    },
                },
            )
        }
        outcome => BluetoothDtmRecurringRunnerCancel::Fault(BluetoothDtmRecurringFault {
            role: BluetoothDtmRole::Transmitter,
            cause: BluetoothDtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: BluetoothDtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: None,
            },
        }),
    }
}

fn cancel_receiver_preparation<'runtime, S, const CAPACITY: usize>(
    terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
    metadata: BluetoothDtmRecurringReceiverMetadata,
) -> BluetoothDtmRecurringRunnerCancel<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Err(failure)) => {
            BluetoothDtmRecurringRunnerCancel::NeedsControllerTimeDrain(
                BluetoothDtmRecurringCancellationDrain {
                    phase: BluetoothDtmRecurringCancellationDrainPhase::Receiver {
                        epoch,
                        owner: failure.into_owner(),
                        metadata,
                    },
                },
            )
        }
        outcome => BluetoothDtmRecurringRunnerCancel::Fault(BluetoothDtmRecurringFault {
            role: BluetoothDtmRole::Receiver,
            cause: BluetoothDtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: BluetoothDtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: Some(metadata),
            },
        }),
    }
}

fn finish_transmitter_recurring<'runtime, S, const CAPACITY: usize>(
    terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
) -> BluetoothDtmRecurringRunnerStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(Ok(merged)) => {
            BluetoothDtmRecurringRunner::continue_with(
                BluetoothDtmRecurringPhase::TransmitterPrepared {
                    task: epoch.into_task_service(),
                    merged,
                },
            )
        }
        BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(Err(failure)) => {
            let error = failure.error();
            let task = epoch.into_task_service();
            let owner = failure.into_owner();
            if matches!(
                error,
                BluetoothDtmControllerEventPreparationError::ControllerTime(_)
            ) {
                BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
                    role: BluetoothDtmRole::Transmitter,
                    cause: BluetoothDtmRecurringFaultCause::Preparation(error),
                    _owner: BluetoothDtmRecurringFaultOwner::TransmitterPreparation { task, owner },
                })
            } else {
                BluetoothDtmRecurringRunner::retry_with(
                    BluetoothDtmRecurringPhase::TransmitterCpu(
                        BluetoothDtmActiveTransmitterReady { _task: task, owner },
                    ),
                    BluetoothDtmRecurringRetryCause::Preparation(error),
                )
            }
        }
        outcome => BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
            role: BluetoothDtmRole::Transmitter,
            cause: BluetoothDtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: BluetoothDtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: None,
            },
        }),
    }
}

fn finish_receiver_recurring<'runtime, S, const CAPACITY: usize>(
    terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
    metadata: BluetoothDtmRecurringReceiverMetadata,
) -> BluetoothDtmRecurringRunnerStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Ok(merged)) => {
            BluetoothDtmRecurringRunner::continue_with(
                BluetoothDtmRecurringPhase::ReceiverPrepared {
                    task: epoch.into_task_service(),
                    merged,
                    metadata,
                },
            )
        }
        BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(Err(failure)) => {
            let error = failure.error();
            let task = epoch.into_task_service();
            let owner = failure.into_owner();
            if matches!(
                error,
                BluetoothDtmControllerEventPreparationError::ControllerTime(_)
            ) {
                BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
                    role: BluetoothDtmRole::Receiver,
                    cause: BluetoothDtmRecurringFaultCause::Preparation(error),
                    _owner: BluetoothDtmRecurringFaultOwner::ReceiverPreparation {
                        task,
                        owner,
                        metadata,
                    },
                })
            } else {
                BluetoothDtmRecurringRunner::retry_with(
                    BluetoothDtmRecurringPhase::ReceiverCpu(BluetoothDtmActiveReceiverReady {
                        _task: task,
                        owner,
                        status: metadata.status,
                        outcome: metadata.outcome,
                    }),
                    BluetoothDtmRecurringRetryCause::Preparation(error),
                )
            }
        }
        outcome => BluetoothDtmRecurringRunnerStep::Fault(BluetoothDtmRecurringFault {
            role: BluetoothDtmRole::Receiver,
            cause: BluetoothDtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: BluetoothDtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: Some(metadata),
            },
        }),
    }
}
