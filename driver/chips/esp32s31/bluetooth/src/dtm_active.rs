//! Executor-neutral completion of one active DTM scheduler event.
//!
//! Every operation advances exactly one lower ownership transition. Waiting
//! states retain the complete Controller and graph outside an executor future;
//! interrupt service remains the responsibility of the disjoint published ISR
//! endpoint.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemCompletionStatus;

use crate::dtm_runner::BluetoothDtmFirstActiveParts;
use crate::{
    BluetoothControllerPublishedTaskService, BluetoothDtmActiveReceiverCpuOwned,
    BluetoothDtmActiveTransmitterCpuOwned, BluetoothDtmFirstActive, BluetoothDtmPostUnlinkArmStep,
    BluetoothDtmPostUnlinkAwaiting, BluetoothDtmReceiverEvent, BluetoothDtmRole,
    BluetoothDtmRxCompletionOutcome, BluetoothDtmSchedulerCompletionObserved,
    BluetoothDtmSchedulerCompletionObservedDrainStep, BluetoothDtmSchedulerCompletionStep,
    BluetoothDtmSchedulerFinishedListDrainPending, BluetoothDtmSchedulerFinishedListDrainState,
    BluetoothDtmSchedulerHardwareHeadEmptyObserved,
    BluetoothDtmSchedulerHardwareHeadRetirementStep, BluetoothDtmSchedulerRecycleStep,
    BluetoothDtmSchedulerRunning, BluetoothDtmSchedulerRunningDrainStep,
    BluetoothDtmSchedulerRxSuccessRecycleStep, BluetoothDtmSchedulerSoftwareListRemovalReady,
    BluetoothDtmSoftwareListRemovalPublishedStep, BluetoothDtmTransmitterEvent,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

enum BluetoothDtmRoleCompletionPhase<'runtime, S, const CAPACITY: usize, Role> {
    Running {
        task: Task<'runtime, S, CAPACITY>,
        running: BluetoothDtmSchedulerRunning<Role>,
    },
    RunningDrain {
        task: Task<'runtime, S, CAPACITY>,
        pending: BluetoothDtmSchedulerFinishedListDrainPending<BluetoothDtmSchedulerRunning<Role>>,
    },
    CompletionDrain {
        task: Task<'runtime, S, CAPACITY>,
        pending: BluetoothDtmSchedulerFinishedListDrainPending<
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
                BluetoothDtmRoleCompletionPhase::Running { task, .. },
            )
            | BluetoothDtmActiveCompletionPhase::Receiver(
                BluetoothDtmRoleCompletionPhase::Running { task, .. },
            ) => task.scheduler_wake(),
            _ => unreachable!("scheduler wait retains a running graph"),
        }
    }

    /// Consume the parked wait before performing one later bounded recheck.
    pub fn resume(self) -> BluetoothDtmActiveCompletion<'runtime, S, CAPACITY> {
        self.completion
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
#[must_use = "the active role must recur or enter the proven Test End path"]
pub enum BluetoothDtmActiveCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter(BluetoothDtmActiveTransmitterReady<'runtime, S, CAPACITY>),
    Receiver(BluetoothDtmActiveReceiverReady<'runtime, S, CAPACITY>),
}

/// Exact Controller and active TX graph at the only CPU-owned command boundary.
#[must_use = "the transmitter must recur or enter the proven Test End path"]
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
#[must_use = "the receiver must recur or enter the proven Test End path"]
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
    ) {
        (self._task, self.owner)
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
    drain: BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerRunning<Role>>,
    wait_when_drained: bool,
) -> BluetoothDtmRoleCompletionAdvance<'runtime, S, CAPACITY, Role> {
    match drain {
        BluetoothDtmSchedulerFinishedListDrainState::Drained(running) => {
            let phase = BluetoothDtmRoleCompletionPhase::Running { task, running };
            if wait_when_drained {
                BluetoothDtmRoleCompletionAdvance::WaitScheduler(phase)
            } else {
                BluetoothDtmRoleCompletionAdvance::Continue(phase)
            }
        }
        BluetoothDtmSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothDtmRoleCompletionAdvance::Continue(
                BluetoothDtmRoleCompletionPhase::RunningDrain { task, pending },
            )
        }
    }
}

fn drained_or_pending_completed<'runtime, S, const CAPACITY: usize, Role>(
    task: Task<'runtime, S, CAPACITY>,
    drain: BluetoothDtmSchedulerFinishedListDrainState<
        BluetoothDtmSchedulerCompletionObserved<Role>,
    >,
) -> BluetoothDtmRoleCompletionPhase<'runtime, S, CAPACITY, Role> {
    match drain {
        BluetoothDtmSchedulerFinishedListDrainState::Drained(completed) => {
            BluetoothDtmRoleCompletionPhase::CompletionObserved { task, completed }
        }
        BluetoothDtmSchedulerFinishedListDrainState::Pending(pending) => {
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
        BluetoothDtmRoleCompletionPhase::Running { mut task, running } => {
            let step = task.observe_dtm_completion(running);
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
                        BluetoothDtmRoleCompletionPhase::Running { task, running },
                    )
                }
                BluetoothDtmSchedulerCompletionStep::UnrelatedList { drain, observed } => {
                    let advance = drained_or_pending_running(task, drain, false);
                    let phase = match advance {
                        BluetoothDtmRoleCompletionAdvance::Continue(phase) => phase,
                        _ => unreachable!(),
                    };
                    BluetoothDtmRoleCompletionAdvance::UnrelatedList { phase, observed }
                }
                BluetoothDtmSchedulerCompletionStep::StillInFlight(drain) => {
                    drained_or_pending_running(task, drain, true)
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
                    let advance = drained_or_pending_running(task, drain, false);
                    let phase = match advance {
                        BluetoothDtmRoleCompletionAdvance::Continue(phase) => phase,
                        _ => unreachable!(),
                    };
                    BluetoothDtmRoleCompletionAdvance::UnrelatedList { phase, observed }
                }
                BluetoothDtmSchedulerRunningDrainStep::StillInFlight(drain) => {
                    drained_or_pending_running(task, drain, true)
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
                    BluetoothDtmRoleCompletionAdvance::WaitPostUnlink(
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
                BluetoothDtmSoftwareListRemovalPublishedStep::Waiting(awaiting) => {
                    BluetoothDtmRoleCompletionAdvance::WaitPostUnlink(
                        BluetoothDtmRoleCompletionPhase::PostUnlinkAwaiting { task, awaiting },
                    )
                }
                BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWork {
                    awaiting,
                    epoch: _,
                }
                | BluetoothDtmSoftwareListRemovalPublishedStep::Pending { awaiting } => {
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
            }
        }
        BluetoothDtmRoleCompletionPhase::RemovalReady { task, ready } => {
            BluetoothDtmRoleCompletionAdvance::Recycle { task, ready }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmFirstActive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Consume the role-erased first-event endpoint into its exact completion owner.
    pub fn into_completion(self) -> BluetoothDtmActiveCompletion<'runtime, S, CAPACITY> {
        let phase = match self.into_parts() {
            BluetoothDtmFirstActiveParts::Transmitter { task, running } => {
                BluetoothDtmActiveCompletionPhase::Transmitter(
                    BluetoothDtmRoleCompletionPhase::Running { task, running },
                )
            }
            BluetoothDtmFirstActiveParts::Receiver { task, running } => {
                BluetoothDtmActiveCompletionPhase::Receiver(
                    BluetoothDtmRoleCompletionPhase::Running { task, running },
                )
            }
        };
        BluetoothDtmActiveCompletion { phase }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
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
