#![forbid(unsafe_code)]

//! Executor-neutral completion of one active DTM scheduler event.
//!
//! Every operation advances exactly one lower ownership transition. Waiting
//! states retain the complete Controller and graph outside an executor future;
//! interrupt service remains the responsibility of the disjoint published ISR
//! endpoint.

#[cfg(target_arch = "riscv32")]
pub(crate) mod session;

use crate::{
    controller::{
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginError,
        ControllerSchedulerCurrentBeginFailure, ControllerSchedulerCurrentError,
        ControllerSchedulerCurrentFailure, ControllerSchedulerCurrentPending,
        ControllerSchedulerCurrentStep, ControllerSchedulerEpochRetained,
        ControllerSchedulerNowReady, ControllerTimeOrphanDrainStep,
        DtmControllerPreparationOutcome, DtmControllerPreparationPending,
        DtmControllerPreparationStep, DtmControllerPreparationTerminal, DtmPostUnlinkArmStep,
        DtmSoftwareListRemovalPublishedStep, SchedulerRunInterruptStorage,
    },
    interrupt::SchedulerWakeBatch,
    le::dtm::{
        BluetoothPostUnlinkAwaiting, DtmActiveReceiverCpuOwned, DtmActiveTransmitterCpuOwned,
        DtmReceiverEvent, DtmRole, DtmRxCompletionOutcome, DtmTransmitterEvent,
        runner::DtmFirstRunningParts,
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved, DtmControllerEventPreparationError,
        DtmEmptySchedulerMergePrepared, DtmRecurringSchedulerItemPhase,
        DtmSchedulerCompletionObserved, DtmSchedulerCompletionObservedDrainStep,
        DtmSchedulerCompletionStep, DtmSchedulerHardwareHeadEmptyObserved,
        DtmSchedulerHardwareHeadRetirementStep, DtmSchedulerHeadPublished, DtmSchedulerRecycleStep,
        DtmSchedulerRunning, DtmSchedulerRunningDrainStep, DtmSchedulerRxSuccessRecycleStep,
        DtmSchedulerSoftwareListRemovalReady, SchedulerFinishedListDrainPending,
        SchedulerFinishedListDrainState, SchedulerHeadPublicationError,
    },
};

use oer_esp32s31_bluetooth_memory::DtmSchedulerItemCompletionStatus;

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;

enum DtmRoleCompletionPhase<'runtime, S, const CAPACITY: usize, Role> {
    RunningAwaitingWake {
        task: Task<'runtime, S, CAPACITY>,
        running: DtmSchedulerRunning<Role>,
    },
    RunningReady {
        task: Task<'runtime, S, CAPACITY>,
        running: DtmSchedulerRunning<Role>,
        wake: SchedulerWakeBatch,
    },
    RunningDrain {
        task: Task<'runtime, S, CAPACITY>,
        pending: SchedulerFinishedListDrainPending<DtmSchedulerRunning<Role>>,
    },
    CompletionDrain {
        task: Task<'runtime, S, CAPACITY>,
        pending: SchedulerFinishedListDrainPending<DtmSchedulerCompletionObserved<Role>>,
    },
    CompletionObserved {
        task: Task<'runtime, S, CAPACITY>,
        completed: DtmSchedulerCompletionObserved<Role>,
    },
    HardwareHeadEmpty {
        task: Task<'runtime, S, CAPACITY>,
        observed: DtmSchedulerHardwareHeadEmptyObserved<Role>,
    },
    PostUnlinkAwaiting {
        task: Task<'runtime, S, CAPACITY>,
        awaiting:
            BluetoothPostUnlinkAwaiting<crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>>,
    },
    RemovalReady {
        task: Task<'runtime, S, CAPACITY>,
        ready: DtmSchedulerSoftwareListRemovalReady<Role>,
    },
}

enum DtmActiveCompletionPhase<'runtime, S, const CAPACITY: usize> {
    Transmitter(DtmRoleCompletionPhase<'runtime, S, CAPACITY, DtmTransmitterEvent>),
    Receiver(DtmRoleCompletionPhase<'runtime, S, CAPACITY, DtmReceiverEvent>),
}

/// Executor-neutral owner of one active TX or RX scheduler completion.
#[must_use = "the active DTM graph must reach a wait, CPU ownership or an opaque fault"]
pub struct DtmActiveCompletion<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: DtmActiveCompletionPhase<'runtime, S, CAPACITY>,
}

/// One bounded active-completion transition.
#[must_use = "retain the active completion owner and every unrelated list observation"]
pub enum DtmActiveCompletionStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// The previous transition completed and another transition may run now.
    Continue(DtmActiveCompletion<'runtime, S, CAPACITY>),
    /// No completion is currently available; wait for a scheduler notification.
    WaitScheduler(DtmActiveSchedulerWait<'runtime, S, CAPACITY>),
    /// One unrelated hardware list remains owned by its external dispatcher.
    UnrelatedList {
        completion: DtmActiveCompletion<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// The graph is unlinked and awaits a later published primary event.
    WaitPostUnlink(DtmActivePostUnlinkWait<'runtime, S, CAPACITY>),
    /// Memory and timeline ownership returned to the active role.
    CpuOwned(DtmActiveCpuOwned<'runtime, S, CAPACITY>),
    /// A fail-closed lower transition retained every affine owner opaquely.
    Fault(DtmActiveCompletionFault<'runtime, S, CAPACITY>),
}

/// Parked scheduler-completion owner with the only relevant durable wake source.
#[must_use = "register the wake, recheck, or retain the complete active owner"]
pub struct DtmActiveSchedulerWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    completion: DtmActiveCompletion<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> DtmActiveSchedulerWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn step_stop(
        self,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    ) -> (
        DtmActiveCompletionStep<'runtime, S, CAPACITY>,
        Option<oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop>,
    ) {
        match self.completion.phase {
            DtmActiveCompletionPhase::Transmitter(phase) => {
                let (advance, stop) = step_stopping_role(phase, stop);
                (map_transmitter_advance(advance), stop)
            }
            DtmActiveCompletionPhase::Receiver(phase) => {
                let (advance, stop) = step_stopping_role(phase, stop);
                (map_receiver_advance(advance), stop)
            }
        }
    }

    /// Durable scheduler wake belonging to this exact Controller epoch.
    pub fn wake(&self) -> &crate::interrupt::SchedulerWakeCell {
        match &self.completion.phase {
            DtmActiveCompletionPhase::Transmitter(
                DtmRoleCompletionPhase::RunningAwaitingWake { task, .. },
            )
            | DtmActiveCompletionPhase::Receiver(DtmRoleCompletionPhase::RunningAwaitingWake {
                task,
                ..
            }) => task.scheduler_wake(),
            _ => unreachable!("scheduler wait retains a running graph"),
        }
    }

    /// Consume the parked wait and the exact dequeued scheduler batch before
    /// performing one fresh finished-list transfer.
    pub fn resume(self, wake: SchedulerWakeBatch) -> DtmActiveCompletion<'runtime, S, CAPACITY> {
        let phase = match self.completion.phase {
            DtmActiveCompletionPhase::Transmitter(
                DtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ) => DtmActiveCompletionPhase::Transmitter(DtmRoleCompletionPhase::RunningReady {
                task,
                running,
                wake,
            }),
            DtmActiveCompletionPhase::Receiver(DtmRoleCompletionPhase::RunningAwaitingWake {
                task,
                running,
            }) => DtmActiveCompletionPhase::Receiver(DtmRoleCompletionPhase::RunningReady {
                task,
                running,
                wake,
            }),
            _ => unreachable!("scheduler wait retains a wake-gated running graph"),
        };
        DtmActiveCompletion { phase }
    }
}

/// Parked post-unlink owner with its exact mailbox notification source.
#[must_use = "register the wake, recheck, or retain the armed post-unlink owner"]
pub struct DtmActivePostUnlinkWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    completion: DtmActiveCompletion<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> DtmActivePostUnlinkWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Durable notification for the exact armed post-unlink mailbox.
    pub fn wake(&self) -> &crate::le::dtm::DtmPostUnlinkWakeCell {
        match &self.completion.phase {
            DtmActiveCompletionPhase::Transmitter(DtmRoleCompletionPhase::PostUnlinkAwaiting {
                task,
                ..
            })
            | DtmActiveCompletionPhase::Receiver(DtmRoleCompletionPhase::PostUnlinkAwaiting {
                task,
                ..
            }) => task.post_unlink_wake(),
            _ => unreachable!("post-unlink wait retains an armed mailbox owner"),
        }
    }

    /// Consume the parked wait before performing one later bounded mailbox take.
    pub fn resume(self) -> DtmActiveCompletion<'runtime, S, CAPACITY> {
        self.completion
    }
}

/// CPU-owned active role after complete scheduler removal and recycle.
#[must_use = "the active role must recur or enter proven terminal quiescence"]
pub enum DtmActiveCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Transmitter(DtmActiveTransmitterReady<'runtime, S, CAPACITY>),
    Receiver(DtmActiveReceiverReady<'runtime, S, CAPACITY>),
}

/// Exact Controller and active TX graph at the only CPU-owned command boundary.
#[must_use = "the transmitter must recur or enter proven terminal quiescence"]
pub struct DtmActiveTransmitterReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _task: Task<'runtime, S, CAPACITY>,
    owner: DtmActiveTransmitterCpuOwned,
}

impl<'runtime, S, const CAPACITY: usize> DtmActiveTransmitterReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.owner.status()
    }

    pub const fn packet_pattern(&self) -> crate::le::dtm::DtmPayloadPattern {
        self.owner.packet_pattern()
    }

    pub const fn payload_length(&self) -> crate::le::dtm::DtmPayloadLength {
        self.owner.packet_length()
    }

    #[allow(
        dead_code,
        reason = "the recurring runner consumes this clean CPU boundary in the next slice"
    )]
    pub(crate) fn into_parts(self) -> (Task<'runtime, S, CAPACITY>, DtmActiveTransmitterCpuOwned) {
        (self._task, self.owner)
    }
}

/// Exact Controller and active RX graph at the only CPU-owned command boundary.
#[must_use = "the receiver must recur or enter proven terminal quiescence"]
pub struct DtmActiveReceiverReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _task: Task<'runtime, S, CAPACITY>,
    owner: DtmActiveReceiverCpuOwned,
    status: DtmSchedulerItemCompletionStatus,
    outcome: Option<DtmRxCompletionOutcome>,
}

impl<'runtime, S, const CAPACITY: usize> DtmActiveReceiverReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.status
    }

    pub const fn outcome(&self) -> Option<DtmRxCompletionOutcome> {
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
        DtmActiveReceiverCpuOwned,
        DtmSchedulerItemCompletionStatus,
        Option<DtmRxCompletionOutcome>,
    ) {
        (self._task, self.owner, self.status, self.outcome)
    }
}

/// Finite fail-closed reason retained by an opaque active-completion owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmActiveCompletionFaultCause {
    SchedulerStopInvariant,
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
enum DtmRoleCompletionFault<'runtime, S, const CAPACITY: usize, Role> {
    Stop {
        task: Task<'runtime, S, CAPACITY>,
        _step: crate::scheduler::core::DtmSchedulerStopStep<Role>,
    },

    Completion {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSchedulerCompletionStep<Role>,
    },
    RunningDrain {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSchedulerRunningDrainStep<Role>,
    },
    CompletionDrain {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSchedulerCompletionObservedDrainStep<Role>,
    },
    HardwareHeadRetirement {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSchedulerHardwareHeadRetirementStep<Role>,
    },
    PostUnlinkArm {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmPostUnlinkArmStep<Role>,
    },
    PostUnlinkPublished {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSoftwareListRemovalPublishedStep<Role>,
    },
    Recycle {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSchedulerRecycleStep<Role>,
    },
}

#[allow(
    dead_code,
    reason = "opaque fault ownership intentionally retains every affine lower token"
)]
enum DtmActiveCompletionFaultOwner<'runtime, S, const CAPACITY: usize> {
    Transmitter(DtmRoleCompletionFault<'runtime, S, CAPACITY, DtmTransmitterEvent>),
    Receiver(DtmRoleCompletionFault<'runtime, S, CAPACITY, DtmReceiverEvent>),
    ReceiverSuccess {
        task: Task<'runtime, S, CAPACITY>,
        _step: DtmSchedulerRxSuccessRecycleStep,
    },
}

/// Opaque fail-stop owner retaining every lower graph, event and Controller token.
#[must_use = "the exact fault owner must remain retained for diagnostic shutdown"]
pub struct DtmActiveCompletionFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    role: DtmRole,
    cause: DtmActiveCompletionFaultCause,
    _owner: DtmActiveCompletionFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> DtmActiveCompletionFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Role of the exact graph retained by this fault owner.
    pub const fn role(&self) -> DtmRole {
        self.role
    }

    pub const fn cause(&self) -> DtmActiveCompletionFaultCause {
        self.cause
    }
}

enum DtmRoleCompletionAdvance<'runtime, S, const CAPACITY: usize, Role> {
    Continue(DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>),
    WaitScheduler(DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>),
    UnrelatedList {
        phase: DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    WaitPostUnlink(DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>),
    Recycle {
        task: Task<'runtime, S, CAPACITY>,
        ready: DtmSchedulerSoftwareListRemovalReady<Role>,
    },
    Fault {
        cause: DtmActiveCompletionFaultCause,
        owner: DtmRoleCompletionFault<'runtime, S, CAPACITY, Role>,
    },
}

fn drained_or_pending_running<'runtime, S, const CAPACITY: usize, Role>(
    task: Task<'runtime, S, CAPACITY>,
    drain: SchedulerFinishedListDrainState<DtmSchedulerRunning<Role>>,
) -> DtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role> {
    match drain {
        SchedulerFinishedListDrainState::Drained(running) => {
            DtmRoleCompletionAdvance::WaitScheduler(DtmRoleCompletionPhase::RunningAwaitingWake {
                task,
                running,
            })
        }
        SchedulerFinishedListDrainState::Pending(pending) => {
            DtmRoleCompletionAdvance::Continue(DtmRoleCompletionPhase::RunningDrain {
                task,
                pending,
            })
        }
    }
}

fn drained_or_pending_completed<'runtime, S, const CAPACITY: usize, Role>(
    task: Task<'runtime, S, CAPACITY>,
    drain: SchedulerFinishedListDrainState<DtmSchedulerCompletionObserved<Role>>,
) -> DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role> {
    match drain {
        SchedulerFinishedListDrainState::Drained(completed) => {
            DtmRoleCompletionPhase::CompletionObserved { task, completed }
        }
        SchedulerFinishedListDrainState::Pending(pending) => {
            DtmRoleCompletionPhase::CompletionDrain { task, pending }
        }
    }
}

fn step_stopping_role<'runtime, S, const CAPACITY: usize, Role>(
    phase: DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
    stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
) -> (
    DtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role>,
    Option<oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop>,
)
where
    S: SchedulerRunInterruptStorage,
{
    use crate::scheduler::core::DtmSchedulerStopStep;
    let DtmRoleCompletionPhase::RunningAwaitingWake { mut task, running } = phase else {
        unreachable!("stop starts only after the running finished-list drain is empty")
    };
    match task.step_dtm_stop(running, stop) {
        DtmSchedulerStopStep::Pending { running, stop } => (
            DtmRoleCompletionAdvance::WaitScheduler(DtmRoleCompletionPhase::RunningAwaitingWake {
                task,
                running,
            }),
            Some(stop),
        ),
        DtmSchedulerStopStep::Retired(DtmSchedulerHardwareHeadRetirementStep::EmptyObserved(
            observed,
        )) => (
            DtmRoleCompletionAdvance::Continue(DtmRoleCompletionPhase::HardwareHeadEmpty {
                task,
                observed,
            }),
            None,
        ),
        step => (
            DtmRoleCompletionAdvance::Fault {
                cause: DtmActiveCompletionFaultCause::SchedulerStopInvariant,
                owner: DtmRoleCompletionFault::Stop { task, _step: step },
            },
            None,
        ),
    }
}

fn step_role<'runtime, S, const CAPACITY: usize, Role>(
    phase: DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
) -> DtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role>
where
    S: SchedulerRunInterruptStorage,
{
    match phase {
        DtmRoleCompletionPhase::RunningAwaitingWake { task, running } => {
            DtmRoleCompletionAdvance::WaitScheduler(DtmRoleCompletionPhase::RunningAwaitingWake {
                task,
                running,
            })
        }
        DtmRoleCompletionPhase::RunningReady {
            mut task,
            running,
            wake,
        } => {
            let step = task.observe_dtm_completion(running, wake);
            match step {
                DtmSchedulerCompletionStep::DrainAlreadyActive(running) => {
                    let step = DtmSchedulerCompletionStep::DrainAlreadyActive(running);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::FinishedListDrainAlreadyActive,
                        owner: DtmRoleCompletionFault::Completion { task, _step: step },
                    }
                }
                DtmSchedulerCompletionStep::SchedulerIdentityMismatch(running) => {
                    let step = DtmSchedulerCompletionStep::SchedulerIdentityMismatch(running);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::Completion { task, _step: step },
                    }
                }
                DtmSchedulerCompletionStep::NoFinishedList(running) => {
                    DtmRoleCompletionAdvance::WaitScheduler(
                        DtmRoleCompletionPhase::RunningAwaitingWake { task, running },
                    )
                }
                DtmSchedulerCompletionStep::UnrelatedList { drain, observed } => {
                    let advance = drained_or_pending_running(task, drain);
                    let phase = match advance {
                        DtmRoleCompletionAdvance::Continue(phase)
                        | DtmRoleCompletionAdvance::WaitScheduler(phase) => phase,
                        _ => unreachable!(),
                    };
                    DtmRoleCompletionAdvance::UnrelatedList { phase, observed }
                }
                DtmSchedulerCompletionStep::StillInFlight(drain) => {
                    drained_or_pending_running(task, drain)
                }
                DtmSchedulerCompletionStep::CompletionObserved(drain) => {
                    DtmRoleCompletionAdvance::Continue(drained_or_pending_completed(task, drain))
                }
            }
        }
        DtmRoleCompletionPhase::RunningDrain { mut task, pending } => {
            let step = task.continue_dtm_running_finished_list_drain(pending);
            match step {
                DtmSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending) => {
                    let step = DtmSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::RunningDrain { task, _step: step },
                    }
                }
                DtmSchedulerRunningDrainStep::DrainLost(pending) => {
                    let step = DtmSchedulerRunningDrainStep::DrainLost(pending);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::FinishedListDrainLost,
                        owner: DtmRoleCompletionFault::RunningDrain { task, _step: step },
                    }
                }
                DtmSchedulerRunningDrainStep::UnrelatedList { drain, observed } => {
                    let advance = drained_or_pending_running(task, drain);
                    let phase = match advance {
                        DtmRoleCompletionAdvance::Continue(phase)
                        | DtmRoleCompletionAdvance::WaitScheduler(phase) => phase,
                        _ => unreachable!(),
                    };
                    DtmRoleCompletionAdvance::UnrelatedList { phase, observed }
                }
                DtmSchedulerRunningDrainStep::StillInFlight(drain) => {
                    drained_or_pending_running(task, drain)
                }
                DtmSchedulerRunningDrainStep::CompletionObserved(drain) => {
                    DtmRoleCompletionAdvance::Continue(drained_or_pending_completed(task, drain))
                }
            }
        }
        DtmRoleCompletionPhase::CompletionDrain { mut task, pending } => {
            let step = task.continue_dtm_completed_finished_list_drain(pending);
            match step {
                DtmSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(pending) => {
                    let step =
                        DtmSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(pending);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::CompletionDrain { task, _step: step },
                    }
                }
                DtmSchedulerCompletionObservedDrainStep::DrainLost(pending) => {
                    let step = DtmSchedulerCompletionObservedDrainStep::DrainLost(pending);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::FinishedListDrainLost,
                        owner: DtmRoleCompletionFault::CompletionDrain { task, _step: step },
                    }
                }
                DtmSchedulerCompletionObservedDrainStep::UnrelatedList { drain, observed } => {
                    DtmRoleCompletionAdvance::UnrelatedList {
                        phase: drained_or_pending_completed(task, drain),
                        observed,
                    }
                }
                DtmSchedulerCompletionObservedDrainStep::RepeatedDtmList { drain, observed } => {
                    let step = DtmSchedulerCompletionObservedDrainStep::RepeatedDtmList {
                        drain,
                        observed,
                    };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::RepeatedDtmList,
                        owner: DtmRoleCompletionFault::CompletionDrain { task, _step: step },
                    }
                }
            }
        }
        DtmRoleCompletionPhase::CompletionObserved {
            mut task,
            completed,
        } => {
            let step = task.observe_dtm_hardware_head_retirement(completed);
            match step {
                DtmSchedulerHardwareHeadRetirementStep::EmptyObserved(observed) => {
                    DtmRoleCompletionAdvance::Continue(DtmRoleCompletionPhase::HardwareHeadEmpty {
                        task,
                        observed,
                    })
                }
                DtmSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed) => {
                    let step = DtmSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                        completed,
                    );
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::HardwareHeadRetirement { task, _step: step },
                    }
                }
                DtmSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed) => {
                    let step = DtmSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                        completed,
                    );
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::FinishedListDrainStillActive,
                        owner: DtmRoleCompletionFault::HardwareHeadRetirement { task, _step: step },
                    }
                }
                DtmSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                    completed,
                    observed,
                } => {
                    let step = DtmSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                        completed,
                        observed,
                    };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::ExpectedHardwareHeadStillPublished,
                        owner: DtmRoleCompletionFault::HardwareHeadRetirement { task, _step: step },
                    }
                }
                DtmSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                    completed,
                    observed,
                } => {
                    let step = DtmSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                        completed,
                        observed,
                    };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::UnexpectedHardwareHeadChanged,
                        owner: DtmRoleCompletionFault::HardwareHeadRetirement { task, _step: step },
                    }
                }
            }
        }
        DtmRoleCompletionPhase::HardwareHeadEmpty { mut task, observed } => {
            let step = task.unlink_and_arm_dtm_software_list_removal(observed);
            match step {
                DtmPostUnlinkArmStep::Armed(awaiting) => {
                    DtmRoleCompletionAdvance::Continue(DtmRoleCompletionPhase::PostUnlinkAwaiting {
                        task,
                        awaiting,
                    })
                }
                DtmPostUnlinkArmStep::MailboxBusy(observed) => {
                    let step = DtmPostUnlinkArmStep::MailboxBusy(observed);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkMailboxBusy,
                        owner: DtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                DtmPostUnlinkArmStep::MailboxIdentityExhausted(observed) => {
                    let step = DtmPostUnlinkArmStep::MailboxIdentityExhausted(observed);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkMailboxIdentityExhausted,
                        owner: DtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                DtmPostUnlinkArmStep::GenerationExhausted(observed) => {
                    let step = DtmPostUnlinkArmStep::GenerationExhausted(observed);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkMailboxGenerationExhausted,
                        owner: DtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                DtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed) => {
                    let step = DtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
                DtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked) => {
                    let step = DtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkMailboxCommitMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkArm { task, _step: step },
                    }
                }
            }
        }
        DtmRoleCompletionPhase::PostUnlinkAwaiting { mut task, awaiting } => {
            let step = task.consume_published_dtm_software_list_removal(awaiting);
            match step {
                DtmSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, epoch: _ }
                | DtmSoftwareListRemovalPublishedStep::PublishedPending { awaiting } => {
                    DtmRoleCompletionAdvance::Continue(DtmRoleCompletionPhase::PostUnlinkAwaiting {
                        task,
                        awaiting,
                    })
                }
                DtmSoftwareListRemovalPublishedStep::DirectPending { awaiting } => {
                    DtmRoleCompletionAdvance::WaitPostUnlink(
                        DtmRoleCompletionPhase::PostUnlinkAwaiting { task, awaiting },
                    )
                }
                DtmSoftwareListRemovalPublishedStep::Ready { ready } => {
                    DtmRoleCompletionAdvance::Continue(DtmRoleCompletionPhase::RemovalReady {
                        task,
                        ready,
                    })
                }
                DtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting) => {
                    let step =
                        DtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting);
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkMailboxAffinityMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault } => {
                    let step = DtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PrimaryInterruptFault,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                    unlinked,
                    epoch,
                } => {
                    let step = DtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                        unlinked,
                        epoch,
                    };
                    DtmRoleCompletionAdvance::Fault {
                        cause:
                            DtmActiveCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked } => {
                    let step =
                        DtmSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkPendingRearmMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting } => {
                    let step = DtmSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkRecheckUnavailable,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked } => {
                    let step =
                        DtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::PostUnlinkRecheckRearmMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                    unlinked,
                    event,
                } => {
                    let step = DtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                        unlinked,
                        event,
                    };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
                DtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                    unlinked,
                } => {
                    let step =
                        DtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                            unlinked,
                        };
                    DtmRoleCompletionAdvance::Fault {
                        cause: DtmActiveCompletionFaultCause::SchedulerIdentityMismatch,
                        owner: DtmRoleCompletionFault::PostUnlinkPublished { task, _step: step },
                    }
                }
            }
        }
        DtmRoleCompletionPhase::RemovalReady { task, ready } => {
            DtmRoleCompletionAdvance::Recycle { task, ready }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmActiveCompletion<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn from_first_running(
        parts: DtmFirstRunningParts<'runtime, S, CAPACITY>,
    ) -> (
        oer_bluetooth_hci::LeControllerResponsePending<'runtime, ()>,
        Self,
    ) {
        match parts {
            DtmFirstRunningParts::Transmitter { response, running } => {
                let (task, response) = response.into_parts();
                (response, Self::from_transmitter_running(task, running))
            }
            DtmFirstRunningParts::Receiver { response, running } => {
                let (task, response) = response.into_parts();
                (response, Self::from_receiver_running(task, running))
            }
        }
    }

    fn from_transmitter_running(
        task: Task<'runtime, S, CAPACITY>,
        running: DtmSchedulerRunning<DtmTransmitterEvent>,
    ) -> Self {
        Self {
            phase: DtmActiveCompletionPhase::Transmitter(
                DtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ),
        }
    }

    fn from_receiver_running(
        task: Task<'runtime, S, CAPACITY>,
        running: DtmSchedulerRunning<DtmReceiverEvent>,
    ) -> Self {
        Self {
            phase: DtmActiveCompletionPhase::Receiver(
                DtmRoleCompletionPhase::RunningAwaitingWake { task, running },
            ),
        }
    }

    /// Role of the exact graph retained by this completion owner.
    pub const fn role(&self) -> DtmRole {
        match self.phase {
            DtmActiveCompletionPhase::Transmitter(_) => DtmRole::Transmitter,
            DtmActiveCompletionPhase::Receiver(_) => DtmRole::Receiver,
        }
    }

    /// Advance exactly one lower completion, drain, unlink, mailbox or recycle edge.
    pub fn step(self) -> DtmActiveCompletionStep<'runtime, S, CAPACITY> {
        match self.phase {
            DtmActiveCompletionPhase::Transmitter(phase) => {
                map_transmitter_advance(step_role(phase))
            }
            DtmActiveCompletionPhase::Receiver(phase) => map_receiver_advance(step_role(phase)),
        }
    }
}

fn map_transmitter_advance<'runtime, S, const CAPACITY: usize>(
    advance: DtmRoleCompletionAdvance<'runtime, S, CAPACITY, DtmTransmitterEvent>,
) -> DtmActiveCompletionStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    map_role_advance(
        advance,
        DtmRole::Transmitter,
        DtmActiveCompletionPhase::Transmitter,
        DtmActiveCompletionFaultOwner::Transmitter,
        |mut task, ready| match task.recycle_dtm_completed(ready) {
            DtmSchedulerRecycleStep::Recycled(recycled) => DtmActiveCompletionStep::CpuOwned(
                DtmActiveCpuOwned::Transmitter(DtmActiveTransmitterReady {
                    _task: task,
                    owner: recycled.into_next(),
                }),
            ),
            step @ (DtmSchedulerRecycleStep::SchedulerIdentityMismatch(_)
            | DtmSchedulerRecycleStep::FinishedListDrainStillActive(_)
            | DtmSchedulerRecycleStep::MemoryIdentityMismatch { .. }
            | DtmSchedulerRecycleStep::ReservationIdentityMismatch(_)
            | DtmSchedulerRecycleStep::ReceiverSuccessRequiresSpecializedRecycle(_)) => {
                let cause = recycle_fault_cause(&step);
                DtmActiveCompletionStep::Fault(DtmActiveCompletionFault {
                    role: DtmRole::Transmitter,
                    cause,
                    _owner: DtmActiveCompletionFaultOwner::Transmitter(
                        DtmRoleCompletionFault::Recycle { task, _step: step },
                    ),
                })
            }
        },
    )
}

fn map_receiver_advance<'runtime, S, const CAPACITY: usize>(
    advance: DtmRoleCompletionAdvance<'runtime, S, CAPACITY, DtmReceiverEvent>,
) -> DtmActiveCompletionStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    map_role_advance(
        advance,
        DtmRole::Receiver,
        DtmActiveCompletionPhase::Receiver,
        DtmActiveCompletionFaultOwner::Receiver,
        |mut task, ready| {
            let status = ready.status();
            if status == DtmSchedulerItemCompletionStatus::Zero {
                match task.recycle_dtm_receiver_success(ready) {
                    DtmSchedulerRxSuccessRecycleStep::Rearmed(rearmed) => {
                        let outcome = rearmed.outcome();
                        #[cfg(feature = "dtm-diagnostics")]
                        super::diagnostics::record(status, Some(outcome));
                        DtmActiveCompletionStep::CpuOwned(DtmActiveCpuOwned::Receiver(
                            DtmActiveReceiverReady {
                                _task: task,
                                owner: rearmed.into_next(),
                                status,
                                outcome: Some(outcome),
                            },
                        ))
                    }
                    step => {
                        let cause = rx_success_recycle_fault_cause(&step);
                        DtmActiveCompletionStep::Fault(DtmActiveCompletionFault {
                            role: DtmRole::Receiver,
                            cause,
                            _owner: DtmActiveCompletionFaultOwner::ReceiverSuccess {
                                task,
                                _step: step,
                            },
                        })
                    }
                }
            } else {
                #[cfg(feature = "dtm-diagnostics")]
                super::diagnostics::record(status, None);
                match task.recycle_dtm_completed(ready) {
                    DtmSchedulerRecycleStep::Recycled(recycled) => {
                        DtmActiveCompletionStep::CpuOwned(DtmActiveCpuOwned::Receiver(
                            DtmActiveReceiverReady {
                                _task: task,
                                owner: recycled.into_next(),
                                status,
                                outcome: None,
                            },
                        ))
                    }
                    step => {
                        let cause = recycle_fault_cause(&step);
                        DtmActiveCompletionStep::Fault(DtmActiveCompletionFault {
                            role: DtmRole::Receiver,
                            cause,
                            _owner: DtmActiveCompletionFaultOwner::Receiver(
                                DtmRoleCompletionFault::Recycle { task, _step: step },
                            ),
                        })
                    }
                }
            }
        },
    )
}

fn map_role_advance<'runtime, S, const CAPACITY: usize, Role>(
    advance: DtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role>,
    role: DtmRole,
    phase: impl FnOnce(
        DtmRoleCompletionPhase<'runtime, S, CAPACITY, Role>,
    ) -> DtmActiveCompletionPhase<'runtime, S, CAPACITY>
    + Copy,
    fault_owner: impl FnOnce(
        DtmRoleCompletionFault<'runtime, S, CAPACITY, Role>,
    ) -> DtmActiveCompletionFaultOwner<'runtime, S, CAPACITY>,
    recycle: impl FnOnce(
        Task<'runtime, S, CAPACITY>,
        DtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> DtmActiveCompletionStep<'runtime, S, CAPACITY>,
) -> DtmActiveCompletionStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match advance {
        DtmRoleCompletionAdvance::Continue(next) => {
            DtmActiveCompletionStep::Continue(DtmActiveCompletion { phase: phase(next) })
        }
        DtmRoleCompletionAdvance::WaitScheduler(next) => {
            DtmActiveCompletionStep::WaitScheduler(DtmActiveSchedulerWait {
                completion: DtmActiveCompletion { phase: phase(next) },
            })
        }
        DtmRoleCompletionAdvance::UnrelatedList {
            phase: next,
            observed,
        } => DtmActiveCompletionStep::UnrelatedList {
            completion: DtmActiveCompletion { phase: phase(next) },
            observed,
        },
        DtmRoleCompletionAdvance::WaitPostUnlink(next) => {
            DtmActiveCompletionStep::WaitPostUnlink(DtmActivePostUnlinkWait {
                completion: DtmActiveCompletion { phase: phase(next) },
            })
        }
        DtmRoleCompletionAdvance::Recycle { task, ready } => recycle(task, ready),
        DtmRoleCompletionAdvance::Fault { cause, owner } => {
            DtmActiveCompletionStep::Fault(DtmActiveCompletionFault {
                role,
                cause,
                _owner: fault_owner(owner),
            })
        }
    }
}

fn recycle_fault_cause<Role>(
    step: &DtmSchedulerRecycleStep<Role>,
) -> DtmActiveCompletionFaultCause {
    match step {
        DtmSchedulerRecycleStep::SchedulerIdentityMismatch(_) => {
            DtmActiveCompletionFaultCause::SchedulerIdentityMismatch
        }
        DtmSchedulerRecycleStep::FinishedListDrainStillActive(_) => {
            DtmActiveCompletionFaultCause::FinishedListDrainStillActive
        }
        DtmSchedulerRecycleStep::MemoryIdentityMismatch { .. } => {
            DtmActiveCompletionFaultCause::MemoryIdentityMismatch
        }
        DtmSchedulerRecycleStep::ReservationIdentityMismatch(_) => {
            DtmActiveCompletionFaultCause::ReservationIdentityMismatch
        }
        DtmSchedulerRecycleStep::ReceiverSuccessRequiresSpecializedRecycle(_) => {
            DtmActiveCompletionFaultCause::ReceiverSpecializedRecycleRequired
        }
        DtmSchedulerRecycleStep::Recycled(_) => unreachable!(),
    }
}

fn rx_success_recycle_fault_cause(
    step: &DtmSchedulerRxSuccessRecycleStep,
) -> DtmActiveCompletionFaultCause {
    match step {
        DtmSchedulerRxSuccessRecycleStep::SchedulerIdentityMismatch(_) => {
            DtmActiveCompletionFaultCause::SchedulerIdentityMismatch
        }
        DtmSchedulerRxSuccessRecycleStep::FinishedListDrainStillActive(_) => {
            DtmActiveCompletionFaultCause::FinishedListDrainStillActive
        }
        DtmSchedulerRxSuccessRecycleStep::CompletionStatusMismatch(_) => {
            DtmActiveCompletionFaultCause::ReceiverCompletionStatusMismatch
        }
        DtmSchedulerRxSuccessRecycleStep::MemoryIdentityMismatch { .. } => {
            DtmActiveCompletionFaultCause::MemoryIdentityMismatch
        }
        DtmSchedulerRxSuccessRecycleStep::ReturnedTopologyRejected { .. } => {
            DtmActiveCompletionFaultCause::ReceiverReturnedTopologyRejected
        }
        DtmSchedulerRxSuccessRecycleStep::ReservationIdentityMismatch(_) => {
            DtmActiveCompletionFaultCause::ReservationIdentityMismatch
        }
        DtmSchedulerRxSuccessRecycleStep::Rearmed(_) => unreachable!(),
    }
}

type DtmRecurringTxMerge =
    DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmRecurringSchedulerItemPhase>;
type DtmRecurringRxMerge =
    DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmRecurringSchedulerItemPhase>;

#[derive(Clone, Copy)]
struct DtmRecurringReceiverMetadata {
    status: DtmSchedulerItemCompletionStatus,
    outcome: Option<DtmRxCompletionOutcome>,
}

enum DtmRecurringPhase<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    TransmitterCpu(DtmActiveTransmitterReady<'runtime, S, CAPACITY>),
    ReceiverCpu(DtmActiveReceiverReady<'runtime, S, CAPACITY>),
    TransmitterEpoch {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    ReceiverEpoch {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: DtmActiveReceiverCpuOwned,
        metadata: DtmRecurringReceiverMetadata,
    },
    TransmitterCurrent {
        pending: ControllerSchedulerCurrentPending<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    TransmitterNow {
        current: ControllerSchedulerNowReady<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    TransmitterPreparation(DtmControllerPreparationPending<'runtime, S, CAPACITY>),
    ReceiverPreparation {
        pending: DtmControllerPreparationPending<'runtime, S, CAPACITY>,
        metadata: DtmRecurringReceiverMetadata,
    },
    TransmitterPrepared {
        task: Task<'runtime, S, CAPACITY>,
        merged: DtmRecurringTxMerge,
    },
    ReceiverPrepared {
        task: Task<'runtime, S, CAPACITY>,
        merged: DtmRecurringRxMerge,
        metadata: DtmRecurringReceiverMetadata,
    },
    TransmitterHead {
        task: Task<'runtime, S, CAPACITY>,
        head: DtmSchedulerHeadPublished<DtmTransmitterEvent>,
    },
    ReceiverHead {
        task: Task<'runtime, S, CAPACITY>,
        head: DtmSchedulerHeadPublished<DtmReceiverEvent>,
    },
}

/// Executor-neutral owner of one recurring TX or RX preparation and `RUN`.
#[must_use = "the recurring graph must reach RUN, a cooperative wait, retry or fail-stop"]
pub struct DtmRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: DtmRecurringPhase<'runtime, S, CAPACITY>,
}

/// Result of one bounded recurring transition.
#[must_use = "retain every recurring owner until it reaches RUN or explicit failure handling"]
pub enum DtmRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// One ownership-only or completed lower transition permits immediate progress.
    Continue(DtmRecurringRunner<'runtime, S, CAPACITY>),
    /// An exact Controller-time request must be rechecked cooperatively later.
    WaitControllerTime(DtmRecurringControllerTimeWait<'runtime, S, CAPACITY>),
    /// The recurring item reached scheduler `RUN` and re-enters active completion.
    Running(DtmActiveCompletion<'runtime, S, CAPACITY>),
    /// A pre-`RUN` transition returned an unchanged, safely retryable owner.
    Retryable(DtmRecurringRetry<'runtime, S, CAPACITY>),
    /// A fail-closed transition retained every owner without exposing reconstruction.
    Fault(DtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Parked recurring Controller-time owner outside any executor future.
#[must_use = "await one cooperative recheck opportunity, resume, or retain the exact owner"]
pub struct DtmRecurringControllerTimeWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: DtmRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> DtmRecurringControllerTimeWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Consume the parked owner before one later bounded Controller-time recheck.
    pub fn resume(self) -> DtmRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }

    /// Cancel the exact private time request instead of dropping its affine owner.
    pub fn cancel(self) -> DtmRecurringRunnerCancel<'runtime, S, CAPACITY> {
        self.runner.cancel()
    }
}

/// Finite reason a recurring owner can be retried without reconstruction.
#[must_use = "inspect the retry cause before advancing the unchanged runner"]
pub enum DtmRecurringRetryCause<E> {
    /// CPU-owned preparation rejected after returning the unchanged active role.
    Preparation(DtmControllerEventPreparationError),
    /// The prepared merge remained CPU-owned because head publication was rejected.
    HeadPublication(SchedulerHeadPublicationError),
    /// Dynamic interrupt preparation rejected the unchanged published head.
    SchedulerStart(E),
}

/// Opaque retry owner retaining the exact role-consistent task and graph.
#[must_use = "retry or retain the exact pre-RUN owner"]
pub struct DtmRecurringRetry<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: DtmRecurringRetryCause<S::Error>,
    runner: DtmRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize> DtmRecurringRetry<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Exact finite rejection paired with the unchanged retry owner.
    pub const fn cause(&self) -> &DtmRecurringRetryCause<S::Error> {
        &self.cause
    }

    /// Recover the unchanged runner for an explicit caller-selected retry.
    pub fn retry(self) -> DtmRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }

    /// Stop recurrence without first advancing the retained retry owner.
    ///
    /// Preparation and head-publication rejection retain a cancellable CPU
    /// owner. Scheduler-start rejection retains an already published head, so
    /// the returned cancellation disposition reports `HeadPublished` instead
    /// of fabricating rollback. The terminal quiescence runner uses that
    /// distinction to finish exactly the hardware-visible event.
    pub(crate) fn cancel_for_quiescence(self) -> DtmRecurringRunnerCancel<'runtime, S, CAPACITY> {
        self.runner.cancel()
    }
}

/// Fail-closed reason recurring ownership cannot advance normally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRecurringFaultCause {
    SchedulerEpochUnavailable,
    SchedulerCurrentBegin(ControllerSchedulerCurrentBeginError),
    SchedulerCurrent(ControllerSchedulerCurrentError),
    Preparation(DtmControllerEventPreparationError),
    UnexpectedPreparationOutcome,
}

#[allow(
    dead_code,
    reason = "opaque recurring faults deliberately retain all graph and Controller owners"
)]
enum DtmRecurringFaultOwner<'runtime, S, const CAPACITY: usize> {
    TransmitterEpochUnavailable {
        task: Task<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    ReceiverEpochUnavailable {
        task: Task<'runtime, S, CAPACITY>,
        owner: DtmActiveReceiverCpuOwned,
        metadata: DtmRecurringReceiverMetadata,
    },
    TransmitterCurrentBegin {
        failure: ControllerSchedulerCurrentBeginFailure<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    TransmitterCurrent {
        failure: ControllerSchedulerCurrentFailure<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    TransmitterOrphanDrain {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    ReceiverOrphanDrain {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: DtmActiveReceiverCpuOwned,
        metadata: DtmRecurringReceiverMetadata,
    },
    TransmitterPreparation {
        task: Task<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    ReceiverPreparation {
        task: Task<'runtime, S, CAPACITY>,
        owner: DtmActiveReceiverCpuOwned,
        metadata: DtmRecurringReceiverMetadata,
    },
    UnexpectedPreparation {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        outcome: DtmControllerPreparationOutcome,
        receiver_metadata: Option<DtmRecurringReceiverMetadata>,
    },
}

enum DtmRecurringCancellationDrainPhase<'runtime, S, const CAPACITY: usize> {
    Transmitter {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: DtmActiveTransmitterCpuOwned,
    },
    Receiver {
        epoch: ControllerSchedulerEpochRetained<'runtime, S, CAPACITY>,
        owner: DtmActiveReceiverCpuOwned,
        metadata: DtmRecurringReceiverMetadata,
    },
}

/// Cancelled recurring time request whose abandoned latch must be drained.
#[must_use = "drain the exact orphan before reusing this Controller epoch"]
pub struct DtmRecurringCancellationDrain<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: DtmRecurringCancellationDrainPhase<'runtime, S, CAPACITY>,
}

/// One bounded orphan-drain observation after recurring cancellation.
#[must_use = "retain Waiting, recovered CPU ownership or the opaque fail-stop owner"]
pub enum DtmRecurringCancellationDrainStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Waiting(DtmRecurringCancellationDrain<'runtime, S, CAPACITY>),
    CpuOwned(DtmActiveCpuOwned<'runtime, S, CAPACITY>),
    Fault(DtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Lossless disposition of explicit recurring cancellation.
#[must_use = "retain recovered ownership, drain the orphan or preserve an irreversible head"]
pub enum DtmRecurringRunnerCancel<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// No Controller-time request or prepared merge remains.
    CpuOwned(DtmActiveCpuOwned<'runtime, S, CAPACITY>),
    /// A cancelled time request must be drained before the Controller is reusable.
    NeedsControllerTimeDrain(DtmRecurringCancellationDrain<'runtime, S, CAPACITY>),
    /// Lower empty-list identity rejected pre-head cancellation unchanged.
    CancellationRejected(DtmRecurringRunner<'runtime, S, CAPACITY>),
    /// The scheduler head is already visible and CPU cancellation is impossible.
    HeadPublished(DtmRecurringRunner<'runtime, S, CAPACITY>),
    /// Cancellation found a fail-stop ownership mismatch.
    Fault(DtmRecurringFault<'runtime, S, CAPACITY>),
}

/// Opaque fail-stop owner for a recurring invariant or timing failure.
#[must_use = "retain the exact fail-stop owner for diagnostic shutdown"]
pub struct DtmRecurringFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    role: DtmRole,
    cause: DtmRecurringFaultCause,
    _owner: DtmRecurringFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> DtmRecurringFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn role(&self) -> DtmRole {
        self.role
    }

    pub const fn cause(&self) -> DtmRecurringFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmActiveCpuOwned<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Begin the exact role-specific recurring transaction without HCI policy.
    pub fn begin_recurring(self) -> DtmRecurringRunner<'runtime, S, CAPACITY> {
        let phase = match self {
            Self::Transmitter(ready) => DtmRecurringPhase::TransmitterCpu(ready),
            Self::Receiver(ready) => DtmRecurringPhase::ReceiverCpu(ready),
        };
        DtmRecurringRunner { phase }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmRecurringRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) fn is_event_boundary(&self) -> bool {
        matches!(
            self.phase,
            DtmRecurringPhase::TransmitterCpu(_) | DtmRecurringPhase::ReceiverCpu(_)
        )
    }

    /// Execute exactly one ownership, Controller-time, publication or RUN transition.
    pub fn step(self) -> DtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        match self.phase {
            DtmRecurringPhase::TransmitterCpu(ready) => {
                let (task, owner) = ready.into_parts();
                match task.retain_scheduler_epoch() {
                    Ok(epoch) => {
                        Self::continue_with(DtmRecurringPhase::TransmitterEpoch { epoch, owner })
                    }
                    Err(unavailable) => DtmRecurringRunnerStep::Fault(DtmRecurringFault {
                        role: DtmRole::Transmitter,
                        cause: DtmRecurringFaultCause::SchedulerEpochUnavailable,
                        _owner: DtmRecurringFaultOwner::TransmitterEpochUnavailable {
                            task: unavailable.into_task_service(),
                            owner,
                        },
                    }),
                }
            }
            DtmRecurringPhase::ReceiverCpu(ready) => {
                let (task, owner, status, outcome) = ready.into_parts();
                let metadata = DtmRecurringReceiverMetadata { status, outcome };
                match task.retain_scheduler_epoch() {
                    Ok(epoch) => Self::continue_with(DtmRecurringPhase::ReceiverEpoch {
                        epoch,
                        owner,
                        metadata,
                    }),
                    Err(unavailable) => DtmRecurringRunnerStep::Fault(DtmRecurringFault {
                        role: DtmRole::Receiver,
                        cause: DtmRecurringFaultCause::SchedulerEpochUnavailable,
                        _owner: DtmRecurringFaultOwner::ReceiverEpochUnavailable {
                            task: unavailable.into_task_service(),
                            owner,
                            metadata,
                        },
                    }),
                }
            }
            DtmRecurringPhase::TransmitterEpoch { epoch, owner } => {
                match epoch.begin_fresh_scheduler_current() {
                    Ok(pending) => {
                        Self::wait_with(DtmRecurringPhase::TransmitterCurrent { pending, owner })
                    }
                    Err(failure) => {
                        let cause = DtmRecurringFaultCause::SchedulerCurrentBegin(failure.error());
                        DtmRecurringRunnerStep::Fault(DtmRecurringFault {
                            role: DtmRole::Transmitter,
                            cause,
                            _owner: DtmRecurringFaultOwner::TransmitterCurrentBegin {
                                failure,
                                owner,
                            },
                        })
                    }
                }
            }
            DtmRecurringPhase::ReceiverEpoch {
                epoch,
                owner,
                metadata,
            } => match epoch.begin_dtm_receiver_recurring_item(owner) {
                Ok(pending) => Self::continue_with(DtmRecurringPhase::ReceiverPreparation {
                    pending,
                    metadata,
                }),
                Err(terminal) => finish_receiver_recurring(terminal, metadata),
            },
            DtmRecurringPhase::TransmitterCurrent { pending, owner } => match pending.recheck() {
                Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                    Self::wait_with(DtmRecurringPhase::TransmitterCurrent { pending, owner })
                }
                Ok(ControllerSchedulerCurrentStep::Ready(current)) => {
                    Self::continue_with(DtmRecurringPhase::TransmitterNow { current, owner })
                }
                Err(failure) => {
                    let cause = DtmRecurringFaultCause::SchedulerCurrent(failure.error());
                    DtmRecurringRunnerStep::Fault(DtmRecurringFault {
                        role: DtmRole::Transmitter,
                        cause,
                        _owner: DtmRecurringFaultOwner::TransmitterCurrent { failure, owner },
                    })
                }
            },
            DtmRecurringPhase::TransmitterNow { current, owner } => {
                match current.begin_dtm_transmitter_recurring_item(owner) {
                    Ok(pending) => {
                        Self::continue_with(DtmRecurringPhase::TransmitterPreparation(pending))
                    }
                    Err(terminal) => finish_transmitter_recurring(terminal),
                }
            }
            DtmRecurringPhase::TransmitterPreparation(pending) => match pending.recheck() {
                DtmControllerPreparationStep::Continue(pending) => {
                    Self::continue_with(DtmRecurringPhase::TransmitterPreparation(pending))
                }
                DtmControllerPreparationStep::Pending(pending) => {
                    Self::wait_with(DtmRecurringPhase::TransmitterPreparation(pending))
                }
                DtmControllerPreparationStep::Terminal(terminal) => {
                    finish_transmitter_recurring(terminal)
                }
            },
            DtmRecurringPhase::ReceiverPreparation { pending, metadata } => {
                match pending.recheck() {
                    DtmControllerPreparationStep::Continue(pending) => {
                        Self::continue_with(DtmRecurringPhase::ReceiverPreparation {
                            pending,
                            metadata,
                        })
                    }
                    DtmControllerPreparationStep::Pending(pending) => {
                        Self::wait_with(DtmRecurringPhase::ReceiverPreparation {
                            pending,
                            metadata,
                        })
                    }
                    DtmControllerPreparationStep::Terminal(terminal) => {
                        finish_receiver_recurring(terminal, metadata)
                    }
                }
            }
            DtmRecurringPhase::TransmitterPrepared { mut task, merged } => {
                match task.publish_dtm_scheduler_head(merged) {
                    Ok(head) => {
                        Self::continue_with(DtmRecurringPhase::TransmitterHead { task, head })
                    }
                    Err(failure) => {
                        let cause = DtmRecurringRetryCause::HeadPublication(failure.error());
                        Self::retry_with(
                            DtmRecurringPhase::TransmitterPrepared {
                                task,
                                merged: failure.into_merged(),
                            },
                            cause,
                        )
                    }
                }
            }
            DtmRecurringPhase::ReceiverPrepared {
                mut task,
                merged,
                metadata,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => Self::continue_with(DtmRecurringPhase::ReceiverHead { task, head }),
                Err(failure) => {
                    let cause = DtmRecurringRetryCause::HeadPublication(failure.error());
                    Self::retry_with(
                        DtmRecurringPhase::ReceiverPrepared {
                            task,
                            merged: failure.into_merged(),
                            metadata,
                        },
                        cause,
                    )
                }
            },
            DtmRecurringPhase::TransmitterHead { mut task, head } => {
                match task.start_dtm_scheduler(head) {
                    Ok(running) => DtmRecurringRunnerStep::Running(
                        DtmActiveCompletion::from_transmitter_running(task, running),
                    ),
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::retry_with(
                            DtmRecurringPhase::TransmitterHead { task, head },
                            DtmRecurringRetryCause::SchedulerStart(error),
                        )
                    }
                }
            }
            DtmRecurringPhase::ReceiverHead { mut task, head } => {
                match task.start_dtm_scheduler(head) {
                    Ok(running) => DtmRecurringRunnerStep::Running(
                        DtmActiveCompletion::from_receiver_running(task, running),
                    ),
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::retry_with(
                            DtmRecurringPhase::ReceiverHead { task, head },
                            DtmRecurringRetryCause::SchedulerStart(error),
                        )
                    }
                }
            }
        }
    }

    fn continue_with(
        phase: DtmRecurringPhase<'runtime, S, CAPACITY>,
    ) -> DtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        DtmRecurringRunnerStep::Continue(Self { phase })
    }

    fn wait_with(
        phase: DtmRecurringPhase<'runtime, S, CAPACITY>,
    ) -> DtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        DtmRecurringRunnerStep::WaitControllerTime(DtmRecurringControllerTimeWait {
            runner: Self { phase },
        })
    }

    fn retry_with(
        phase: DtmRecurringPhase<'runtime, S, CAPACITY>,
        cause: DtmRecurringRetryCause<S::Error>,
    ) -> DtmRecurringRunnerStep<'runtime, S, CAPACITY> {
        DtmRecurringRunnerStep::Retryable(DtmRecurringRetry {
            cause,
            runner: Self { phase },
        })
    }

    /// Cancel only while the graph is still CPU-owned or owns a cancellable time request.
    ///
    /// Once `HEAD` has been published this returns `HeadPublished` unchanged;
    /// there is no fabricated rollback from hardware-visible ownership.
    pub fn cancel(self) -> DtmRecurringRunnerCancel<'runtime, S, CAPACITY> {
        match self.phase {
            DtmRecurringPhase::TransmitterCpu(ready) => {
                DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Transmitter(ready))
            }
            DtmRecurringPhase::ReceiverCpu(ready) => {
                DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Receiver(ready))
            }
            DtmRecurringPhase::TransmitterEpoch { epoch, owner } => {
                DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Transmitter(
                    DtmActiveTransmitterReady {
                        _task: epoch.into_task_service(),
                        owner,
                    },
                ))
            }
            DtmRecurringPhase::ReceiverEpoch {
                epoch,
                owner,
                metadata,
            } => DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Receiver(
                DtmActiveReceiverReady {
                    _task: epoch.into_task_service(),
                    owner,
                    status: metadata.status,
                    outcome: metadata.outcome,
                },
            )),
            DtmRecurringPhase::TransmitterCurrent { pending, owner } => match pending.cancel() {
                Ok(epoch) => DtmRecurringRunnerCancel::NeedsControllerTimeDrain(
                    DtmRecurringCancellationDrain {
                        phase: DtmRecurringCancellationDrainPhase::Transmitter { epoch, owner },
                    },
                ),
                Err(failure) => {
                    let cause = DtmRecurringFaultCause::SchedulerCurrent(failure.error());
                    DtmRecurringRunnerCancel::Fault(DtmRecurringFault {
                        role: DtmRole::Transmitter,
                        cause,
                        _owner: DtmRecurringFaultOwner::TransmitterCurrent { failure, owner },
                    })
                }
            },
            DtmRecurringPhase::TransmitterNow { current, owner } => {
                let epoch = current.into_retained_epoch();
                DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Transmitter(
                    DtmActiveTransmitterReady {
                        _task: epoch.into_task_service(),
                        owner,
                    },
                ))
            }
            DtmRecurringPhase::TransmitterPreparation(pending) => {
                cancel_transmitter_preparation(pending.cancel())
            }
            DtmRecurringPhase::ReceiverPreparation { pending, metadata } => {
                cancel_receiver_preparation(pending.cancel(), metadata)
            }
            DtmRecurringPhase::TransmitterPrepared { mut task, merged } => {
                match task.cancel_dtm_transmitter_recurring_item(merged) {
                    Ok(owner) => {
                        DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Transmitter(
                            DtmActiveTransmitterReady { _task: task, owner },
                        ))
                    }
                    Err(merged) => DtmRecurringRunnerCancel::CancellationRejected(Self {
                        phase: DtmRecurringPhase::TransmitterPrepared { task, merged },
                    }),
                }
            }
            DtmRecurringPhase::ReceiverPrepared {
                mut task,
                merged,
                metadata,
            } => match task.cancel_dtm_receiver_recurring_item(merged) {
                Ok(owner) => DtmRecurringRunnerCancel::CpuOwned(DtmActiveCpuOwned::Receiver(
                    DtmActiveReceiverReady {
                        _task: task,
                        owner,
                        status: metadata.status,
                        outcome: metadata.outcome,
                    },
                )),
                Err(merged) => DtmRecurringRunnerCancel::CancellationRejected(Self {
                    phase: DtmRecurringPhase::ReceiverPrepared {
                        task,
                        merged,
                        metadata,
                    },
                }),
            },
            phase @ (DtmRecurringPhase::TransmitterHead { .. }
            | DtmRecurringPhase::ReceiverHead { .. }) => {
                DtmRecurringRunnerCancel::HeadPublished(Self { phase })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> DtmRecurringCancellationDrain<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Perform one bounded orphan-drain observation.
    pub fn step(mut self) -> DtmRecurringCancellationDrainStep<'runtime, S, CAPACITY> {
        match &mut self.phase {
            DtmRecurringCancellationDrainPhase::Transmitter { epoch, .. }
            | DtmRecurringCancellationDrainPhase::Receiver { epoch, .. } => {
                match epoch.drain_abandoned_controller_time() {
                    Ok(ControllerTimeOrphanDrainStep::Waiting) => {
                        DtmRecurringCancellationDrainStep::Waiting(self)
                    }
                    Ok(ControllerTimeOrphanDrainStep::Idle)
                    | Ok(ControllerTimeOrphanDrainStep::Drained) => self.into_cpu_owned(),
                    Err(error) => self.into_fault(error),
                }
            }
        }
    }

    fn into_cpu_owned(self) -> DtmRecurringCancellationDrainStep<'runtime, S, CAPACITY> {
        let ready = match self.phase {
            DtmRecurringCancellationDrainPhase::Transmitter { epoch, owner } => {
                DtmActiveCpuOwned::Transmitter(DtmActiveTransmitterReady {
                    _task: epoch.into_task_service(),
                    owner,
                })
            }
            DtmRecurringCancellationDrainPhase::Receiver {
                epoch,
                owner,
                metadata,
            } => DtmActiveCpuOwned::Receiver(DtmActiveReceiverReady {
                _task: epoch.into_task_service(),
                owner,
                status: metadata.status,
                outcome: metadata.outcome,
            }),
        };
        DtmRecurringCancellationDrainStep::CpuOwned(ready)
    }

    fn into_fault(
        self,
        error: ControllerSchedulerCurrentError,
    ) -> DtmRecurringCancellationDrainStep<'runtime, S, CAPACITY> {
        let (role, owner) = match self.phase {
            DtmRecurringCancellationDrainPhase::Transmitter { epoch, owner } => (
                DtmRole::Transmitter,
                DtmRecurringFaultOwner::TransmitterOrphanDrain { epoch, owner },
            ),
            DtmRecurringCancellationDrainPhase::Receiver {
                epoch,
                owner,
                metadata,
            } => (
                DtmRole::Receiver,
                DtmRecurringFaultOwner::ReceiverOrphanDrain {
                    epoch,
                    owner,
                    metadata,
                },
            ),
        };
        DtmRecurringCancellationDrainStep::Fault(DtmRecurringFault {
            role,
            cause: DtmRecurringFaultCause::SchedulerCurrent(error),
            _owner: owner,
        })
    }
}

fn cancel_transmitter_preparation<'runtime, S, const CAPACITY: usize>(
    terminal: DtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
) -> DtmRecurringRunnerCancel<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        DtmControllerPreparationOutcome::TransmitterRecurring(Err(failure)) => {
            DtmRecurringRunnerCancel::NeedsControllerTimeDrain(DtmRecurringCancellationDrain {
                phase: DtmRecurringCancellationDrainPhase::Transmitter {
                    epoch,
                    owner: failure.into_owner(),
                },
            })
        }
        outcome => DtmRecurringRunnerCancel::Fault(DtmRecurringFault {
            role: DtmRole::Transmitter,
            cause: DtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: DtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: None,
            },
        }),
    }
}

fn cancel_receiver_preparation<'runtime, S, const CAPACITY: usize>(
    terminal: DtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
    metadata: DtmRecurringReceiverMetadata,
) -> DtmRecurringRunnerCancel<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        DtmControllerPreparationOutcome::ReceiverRecurring(Err(failure)) => {
            DtmRecurringRunnerCancel::NeedsControllerTimeDrain(DtmRecurringCancellationDrain {
                phase: DtmRecurringCancellationDrainPhase::Receiver {
                    epoch,
                    owner: failure.into_owner(),
                    metadata,
                },
            })
        }
        outcome => DtmRecurringRunnerCancel::Fault(DtmRecurringFault {
            role: DtmRole::Receiver,
            cause: DtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: DtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: Some(metadata),
            },
        }),
    }
}

fn finish_transmitter_recurring<'runtime, S, const CAPACITY: usize>(
    terminal: DtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
) -> DtmRecurringRunnerStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        DtmControllerPreparationOutcome::TransmitterRecurring(Ok(merged)) => {
            DtmRecurringRunner::continue_with(DtmRecurringPhase::TransmitterPrepared {
                task: epoch.into_task_service(),
                merged,
            })
        }
        DtmControllerPreparationOutcome::TransmitterRecurring(Err(failure)) => {
            let error = failure.error();
            let task = epoch.into_task_service();
            let owner = failure.into_owner();
            if matches!(error, DtmControllerEventPreparationError::ControllerTime(_)) {
                DtmRecurringRunnerStep::Fault(DtmRecurringFault {
                    role: DtmRole::Transmitter,
                    cause: DtmRecurringFaultCause::Preparation(error),
                    _owner: DtmRecurringFaultOwner::TransmitterPreparation { task, owner },
                })
            } else {
                DtmRecurringRunner::retry_with(
                    DtmRecurringPhase::TransmitterCpu(DtmActiveTransmitterReady {
                        _task: task,
                        owner,
                    }),
                    DtmRecurringRetryCause::Preparation(error),
                )
            }
        }
        outcome => DtmRecurringRunnerStep::Fault(DtmRecurringFault {
            role: DtmRole::Transmitter,
            cause: DtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: DtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: None,
            },
        }),
    }
}

fn finish_receiver_recurring<'runtime, S, const CAPACITY: usize>(
    terminal: DtmControllerPreparationTerminal<'runtime, S, CAPACITY>,
    metadata: DtmRecurringReceiverMetadata,
) -> DtmRecurringRunnerStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (epoch, outcome) = terminal.into_parts();
    match outcome {
        DtmControllerPreparationOutcome::ReceiverRecurring(Ok(merged)) => {
            DtmRecurringRunner::continue_with(DtmRecurringPhase::ReceiverPrepared {
                task: epoch.into_task_service(),
                merged,
                metadata,
            })
        }
        DtmControllerPreparationOutcome::ReceiverRecurring(Err(failure)) => {
            let error = failure.error();
            let task = epoch.into_task_service();
            let owner = failure.into_owner();
            if matches!(error, DtmControllerEventPreparationError::ControllerTime(_)) {
                DtmRecurringRunnerStep::Fault(DtmRecurringFault {
                    role: DtmRole::Receiver,
                    cause: DtmRecurringFaultCause::Preparation(error),
                    _owner: DtmRecurringFaultOwner::ReceiverPreparation {
                        task,
                        owner,
                        metadata,
                    },
                })
            } else {
                DtmRecurringRunner::retry_with(
                    DtmRecurringPhase::ReceiverCpu(DtmActiveReceiverReady {
                        _task: task,
                        owner,
                        status: metadata.status,
                        outcome: metadata.outcome,
                    }),
                    DtmRecurringRetryCause::Preparation(error),
                )
            }
        }
        outcome => DtmRecurringRunnerStep::Fault(DtmRecurringFault {
            role: DtmRole::Receiver,
            cause: DtmRecurringFaultCause::UnexpectedPreparationOutcome,
            _owner: DtmRecurringFaultOwner::UnexpectedPreparation {
                epoch,
                outcome,
                receiver_metadata: Some(metadata),
            },
        }),
    }
}
