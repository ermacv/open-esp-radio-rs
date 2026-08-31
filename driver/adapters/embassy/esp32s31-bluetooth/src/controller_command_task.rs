//! Sole Embassy owner for the ESP32-S31 LE Controller command lifecycle.
//!
//! This actor composes the chip-owned idle command transaction, the bounded
//! first-event runner and the active-session actor. It does not interpret HCI
//! commands or reproduce radio policy. Every awaited future borrows an owner
//! retained in the actor's affine state slot; cancellation therefore leaves
//! the exact lower transaction available to the next `run` call.

#![forbid(unsafe_code)]

use crate::EmbassyBluetoothDtmSessionRetry;

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerIdleCommandIntake, BluetoothControllerIdleCommandTask,
    BluetoothControllerIdleResetBarrier, BluetoothControllerIdleResetCompletion,
    BluetoothControllerIdleResponsePending, BluetoothControllerIdleResponsePublication,
    BluetoothControllerSchedulerCurrentError, BluetoothDtmActiveCommandMismatch,
    BluetoothDtmActiveSessionFault, BluetoothDtmFirstPreparationCleanup,
    BluetoothDtmFirstPreparationCleanupStep, BluetoothDtmFirstPreparationCompletion,
    BluetoothDtmFirstPreparationFailStop, BluetoothDtmFirstRunnerFailure,
    BluetoothDtmFirstRunnerRetry, BluetoothDtmIdleCommandMismatch, BluetoothDtmIdleCommandRoute,
    BluetoothDtmOrderReady, BluetoothDtmResetCompletionReady, BluetoothDtmResetCompletionStart,
    BluetoothDtmResetResponsePending, BluetoothDtmResetResponsePublication,
    BluetoothDtmResetRestoreFailure, BluetoothDtmResetRestoreStep, BluetoothDtmResetStoppingFault,
    BluetoothDtmResetStoppingRunner, BluetoothDtmResetStoppingStep, BluetoothDtmResetStoppingWait,
    BluetoothDtmResponsePending, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::{
    EmbassyBluetoothDtmControllerTimeRecheck, EmbassyBluetoothDtmControllerTimeRecheckStatus,
    EmbassyBluetoothDtmFirstControllerTimeWait, EmbassyBluetoothDtmFirstDrive,
    EmbassyBluetoothDtmFirstResume, EmbassyBluetoothDtmSessionBoundary,
    EmbassyBluetoothDtmSessionTask, EmbassyBluetoothRuntimeWakers, drive_dtm_first_ready,
};

/// Observable phase of the sole Controller command actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerCommandPhase {
    Idle,
    IdleReset,
    IdleResponse,
    FirstEvent,
    Active,
    ResetStopping,
    ResetRestore,
    ResetCompletion,
    ResetResponse,
    UnownedFinishedList,
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production reducer is executed only by the S31 target"
    )
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControllerCommandStimulus {
    Retain,
    IdleReset,
    IdleResponse,
    FirstEvent,
    Active,
    ResetStopping,
    ResetRestore,
    ResetCompletion,
    ResetResponse,
    IdleRestored,
    UnownedFinishedList,
    Terminal,
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production reducer is executed only by the S31 target"
    )
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControllerCommandAction {
    Retain,
    Advance(EmbassyBluetoothControllerCommandPhase),
    Terminal,
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production reducer is executed only by the S31 target"
    )
)]
const fn reduce_controller_command_transition(
    phase: EmbassyBluetoothControllerCommandPhase,
    stimulus: ControllerCommandStimulus,
) -> ControllerCommandAction {
    use ControllerCommandAction::{Advance, Retain, Terminal};
    use ControllerCommandStimulus::{
        Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, ResetCompletion, ResetResponse,
        ResetRestore, ResetStopping, UnownedFinishedList,
    };
    use EmbassyBluetoothControllerCommandPhase::{
        Active as ActivePhase, FirstEvent as FirstEventPhase, Idle, IdleReset as IdleResetPhase,
        IdleResponse as IdleResponsePhase, ResetCompletion as ResetCompletionPhase,
        ResetResponse as ResetResponsePhase, ResetRestore as ResetRestorePhase,
        ResetStopping as ResetStoppingPhase, UnownedFinishedList as UnownedFinishedListPhase,
    };

    match (phase, stimulus) {
        (_, ControllerCommandStimulus::Retain) => Retain,
        (Idle, IdleReset) => Advance(IdleResetPhase),
        (Idle, IdleResponse) => Advance(IdleResponsePhase),
        (Idle, FirstEvent) => Advance(FirstEventPhase),
        (Idle | FirstEventPhase, Active) => Advance(ActivePhase),
        (IdleResetPhase, IdleResponse) => Advance(IdleResponsePhase),
        (FirstEventPhase, IdleResponse) => Advance(IdleResponsePhase),
        (ActivePhase, ResetStopping) => Advance(ResetStoppingPhase),
        (ActivePhase | ResetStoppingPhase, UnownedFinishedList) => {
            Advance(UnownedFinishedListPhase)
        }
        (UnownedFinishedListPhase, UnownedFinishedList) => Retain,
        (ResetStoppingPhase, ResetRestore) => Advance(ResetRestorePhase),
        (ResetStoppingPhase | ResetRestorePhase, ResetCompletion) => Advance(ResetCompletionPhase),
        (ResetCompletionPhase, ResetResponse) => Advance(ResetResponsePhase),
        (IdleResponsePhase | ActivePhase | ResetResponsePhase, IdleRestored) => Advance(Idle),
        (
            Idle | FirstEventPhase | ActivePhase | ResetStoppingPhase,
            ControllerCommandStimulus::Terminal,
        ) => Terminal,
        _ => panic!("invalid Controller command actor transition"),
    }
}

#[cfg(target_arch = "riscv32")]
enum EmbassyBluetoothUnownedFinishedListOwner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Active {
        _task: EmbassyBluetoothDtmSessionTask<'runtime, S, CAPACITY>,
        index: BluetoothSchedulerHardwareListIndex,
    },
    ResetStopping {
        _runner: BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> EmbassyBluetoothUnownedFinishedListOwner<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        match self {
            Self::Active { index, .. } => *index,
            Self::ResetStopping { observed, .. } => observed.index(),
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy)]
enum FirstCleanupReadiness {
    Ready,
    RecheckRequired,
}

#[cfg(target_arch = "riscv32")]
enum EmbassyBluetoothControllerCommandState<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Idle(BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>),
    IdleReset(BluetoothControllerIdleResetBarrier<'runtime, S, CAPACITY>),
    IdleResponse {
        pending: BluetoothControllerIdleResponsePending<'runtime, S, CAPACITY>,
        completion: EmbassyBluetoothControllerIdleCompletion,
    },
    FirstEvent(EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, CAPACITY>),
    FirstRetry(BluetoothDtmFirstRunnerRetry<'runtime, S, CAPACITY>),
    FirstCleanup {
        cleanup: BluetoothDtmFirstPreparationCleanup<'runtime, S, CAPACITY>,
        readiness: FirstCleanupReadiness,
    },
    Active(EmbassyBluetoothDtmSessionTask<'runtime, S, CAPACITY>),
    ResetStopping(BluetoothDtmResetStoppingRunner<'runtime, S, CAPACITY>),
    ResetRestore(BluetoothDtmResetRestoreFailure<'runtime, S, CAPACITY>),
    ResetCompletion(BluetoothDtmResetCompletionReady<'runtime, S, CAPACITY>),
    ResetResponse(BluetoothDtmResetResponsePending<'runtime, S, CAPACITY>),
    UnownedFinishedList(EmbassyBluetoothUnownedFinishedListOwner<'runtime, S, CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> EmbassyBluetoothControllerCommandState<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    const fn phase(&self) -> EmbassyBluetoothControllerCommandPhase {
        match self {
            Self::Idle(_) => EmbassyBluetoothControllerCommandPhase::Idle,
            Self::IdleReset(_) => EmbassyBluetoothControllerCommandPhase::IdleReset,
            Self::IdleResponse { .. } => EmbassyBluetoothControllerCommandPhase::IdleResponse,
            Self::FirstEvent(_) | Self::FirstRetry(_) | Self::FirstCleanup { .. } => {
                EmbassyBluetoothControllerCommandPhase::FirstEvent
            }
            Self::Active(_) => EmbassyBluetoothControllerCommandPhase::Active,
            Self::ResetStopping(_) => EmbassyBluetoothControllerCommandPhase::ResetStopping,
            Self::ResetRestore(_) => EmbassyBluetoothControllerCommandPhase::ResetRestore,
            Self::ResetCompletion(_) => EmbassyBluetoothControllerCommandPhase::ResetCompletion,
            Self::ResetResponse(_) => EmbassyBluetoothControllerCommandPhase::ResetResponse,
            Self::UnownedFinishedList(_) => {
                EmbassyBluetoothControllerCommandPhase::UnownedFinishedList
            }
        }
    }
}

/// Completion that returned the actor to its sole idle command owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerIdleCompletion {
    ImmediateResponse,
    DtmStartRejected,
    TestEnd,
    Reset,
}

/// Recoverable retry boundary while the complete owner remains in the actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerRetry {
    FirstEvent,
    Active(EmbassyBluetoothDtmSessionRetry),
    ResetStopping,
    ResetRestore,
}

/// One lossless externally meaningful boundary from the sole Controller actor.
#[cfg(target_arch = "riscv32")]
#[must_use = "handle the observation or retain the exact terminal lower owner"]
pub enum EmbassyBluetoothControllerCommandBoundary<
    'runtime,
    'epoch,
    'packet,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// A command lifecycle completed and the actor again owns the idle command token.
    IdleRestored(EmbassyBluetoothControllerIdleCompletion),
    /// A non-command Host frame remains bound to its source HCI epoch and buffer.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
    /// The supplied endpoint does not match the retained transaction.
    EndpointMismatch,
    /// HCI failed while the actor retained the complete transaction.
    HciFault(HciChannelError),
    /// A lower owner remained intact and requires an explicit retry.
    Retryable(EmbassyBluetoothControllerRetry),
    /// The absolute Controller-time schedule is exhausted; the actor retains its owner.
    ControllerTimeExhausted,
    /// No installed role owns this scheduler list; its exact owner is quarantined in the actor.
    UnownedFinishedList(BluetoothSchedulerHardwareListIndex),
    /// A non-retryable initial transition failed before scheduler `RUN`.
    ///
    /// Safe lower retries remain stored in the actor and are reported through
    /// [`EmbassyBluetoothControllerRetry::FirstEvent`]. The only automatic
    /// failure response is the separate, typed CleanTask edge after preparation
    /// cleanup has proved the graph idle again.
    FirstEventFailed(BluetoothDtmFirstRunnerFailure<'runtime, S, CAPACITY>),
    /// Preparation cleanup faulted before it could prove a clean idle task.
    FirstPreparationCleanupFault {
        cleanup: BluetoothDtmFirstPreparationCleanup<'runtime, S, CAPACITY>,
        error: BluetoothControllerSchedulerCurrentError,
    },
    /// The runtime rejected the exact graph during preparation-failure restore.
    FirstPreparationRestoreRejected(BluetoothDtmFirstPreparationCleanup<'runtime, S, CAPACITY>),
    /// Chip policy classified the restored failure as poisoned and forbade reuse.
    FirstPreparationFailStop(BluetoothDtmFirstPreparationFailStop<'runtime, S, CAPACITY>),
    /// Idle intake found an impossible post-classification endpoint mismatch.
    IdleCommandEndpointMismatch(BluetoothDtmIdleCommandMismatch<'runtime, 'epoch, S, CAPACITY>),
    /// Active intake found an impossible post-classification endpoint mismatch.
    ActiveCommandEndpointMismatch(BluetoothDtmActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>),
    /// Active radio failed while its response axis was still pending.
    PendingRadioFault(
        BluetoothDtmActiveSessionFault<
            'runtime,
            S,
            CAPACITY,
            BluetoothDtmResponsePending<'runtime>,
        >,
    ),
    /// Active radio failed after command order became ready.
    CommandReadyRadioFault(
        BluetoothDtmActiveSessionFault<'runtime, S, CAPACITY, BluetoothDtmOrderReady<'runtime>>,
    ),
    /// Test End quiescence failed closed with its exact transaction.
    TestEndStoppingFault(
        open_esp_radio_esp32s31_bluetooth::BluetoothDtmStoppingFault<'runtime, S, CAPACITY>,
    ),
    /// Reset quiescence failed closed with its exact transaction.
    ResetStoppingFault(BluetoothDtmResetStoppingFault<'runtime, S, CAPACITY>),
}

#[cfg(any(target_arch = "riscv32", test))]
struct ControllerOwnerSlot<State> {
    state: Option<State>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<State> ControllerOwnerSlot<State> {
    const fn new(state: State) -> Self {
        Self { state: Some(state) }
    }

    fn current(&self) -> &State {
        self.state
            .as_ref()
            .expect("a live Controller command actor retains one affine owner")
    }

    fn current_mut(&mut self) -> &mut State {
        self.state
            .as_mut()
            .expect("a live Controller command actor retains one affine owner")
    }

    fn take(&mut self) -> State {
        self.state
            .take()
            .expect("a Controller transition consumes its owner exactly once")
    }

    fn store(&mut self, state: State) {
        assert!(
            self.state.replace(state).is_none(),
            "a Controller transition cannot overwrite an affine owner"
        );
    }

    const fn is_empty(&self) -> bool {
        self.state.is_none()
    }
}

/// Sole executor-side owner of the idle, first-event and active DTM lifecycle.
#[cfg(target_arch = "riscv32")]
#[must_use = "run the Controller actor until it returns a terminal lower owner"]
pub struct EmbassyBluetoothControllerCommandTask<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    owner: ControllerOwnerSlot<EmbassyBluetoothControllerCommandState<'runtime, S, CAPACITY>>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Start the sole actor from the final runtime's affine idle command task.
    pub const fn new(idle: BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>) -> Self {
        Self {
            owner: ControllerOwnerSlot::new(EmbassyBluetoothControllerCommandState::Idle(idle)),
        }
    }

    /// Current retained lifecycle phase.
    pub fn phase(&self) -> EmbassyBluetoothControllerCommandPhase {
        self.owner.current().phase()
    }

    /// Whether a terminal boundary transferred the lower owner out of the actor.
    pub const fn is_empty(&self) -> bool {
        self.owner.is_empty()
    }

    fn store_transition(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        stimulus: ControllerCommandStimulus,
        state: EmbassyBluetoothControllerCommandState<'runtime, S, CAPACITY>,
    ) {
        match reduce_controller_command_transition(from, stimulus) {
            ControllerCommandAction::Advance(expected) if state.phase() == expected => {
                self.owner.store(state);
            }
            _ => unreachable!("the Controller reducer rejected a stored successor"),
        }
    }

    fn store_retained_state(
        &mut self,
        phase: EmbassyBluetoothControllerCommandPhase,
        state: EmbassyBluetoothControllerCommandState<'runtime, S, CAPACITY>,
    ) {
        assert_eq!(state.phase(), phase);
        assert_eq!(
            reduce_controller_command_transition(phase, ControllerCommandStimulus::Retain),
            ControllerCommandAction::Retain,
        );
        self.owner.store(state);
    }

    fn retain_boundary<'epoch, 'packet>(
        &self,
        boundary: EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        assert_eq!(
            reduce_controller_command_transition(self.phase(), ControllerCommandStimulus::Retain,),
            ControllerCommandAction::Retain,
        );
        boundary
    }

    fn store_unowned_finished_list<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        owner: EmbassyBluetoothUnownedFinishedListOwner<'runtime, S, CAPACITY>,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let index = owner.index();
        self.store_transition(
            from,
            ControllerCommandStimulus::UnownedFinishedList,
            EmbassyBluetoothControllerCommandState::UnownedFinishedList(owner),
        );
        EmbassyBluetoothControllerCommandBoundary::UnownedFinishedList(index)
    }

    fn retained_unowned_finished_list<'epoch, 'packet>(
        &self,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let EmbassyBluetoothControllerCommandState::UnownedFinishedList(owner) =
            self.owner.current()
        else {
            unreachable!("the selected unowned-list quarantine did not change")
        };
        assert_eq!(
            reduce_controller_command_transition(
                EmbassyBluetoothControllerCommandPhase::UnownedFinishedList,
                ControllerCommandStimulus::UnownedFinishedList,
            ),
            ControllerCommandAction::Retain,
        );
        EmbassyBluetoothControllerCommandBoundary::UnownedFinishedList(owner.index())
    }

    fn terminal_boundary<'epoch, 'packet>(
        &self,
        from: EmbassyBluetoothControllerCommandPhase,
        boundary: EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        assert!(self.owner.is_empty());
        assert_eq!(
            reduce_controller_command_transition(from, ControllerCommandStimulus::Terminal,),
            ControllerCommandAction::Terminal,
        );
        boundary
    }

    fn store_first_failure<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        failure: BluetoothDtmFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match failure {
            BluetoothDtmFirstRunnerFailure::PreparationRejected(cleanup) => {
                let state = EmbassyBluetoothControllerCommandState::FirstCleanup {
                    cleanup,
                    readiness: FirstCleanupReadiness::Ready,
                };
                if from == EmbassyBluetoothControllerCommandPhase::FirstEvent {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::FirstEvent, state);
                }
                None
            }
            BluetoothDtmFirstRunnerFailure::Retryable(retry) => {
                let state = EmbassyBluetoothControllerCommandState::FirstRetry(retry);
                if from == EmbassyBluetoothControllerCommandPhase::FirstEvent {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::FirstEvent, state);
                }
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::FirstEvent,
                    )),
                )
            }
            failure => Some(self.terminal_boundary(
                from,
                EmbassyBluetoothControllerCommandBoundary::FirstEventFailed(failure),
            )),
        }
    }

    fn store_first_drive<'epoch, 'packet>(
        &mut self,
        drive: EmbassyBluetoothDtmFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match drive {
            EmbassyBluetoothDtmFirstDrive::Wait(wait) => {
                self.store_retained_state(
                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                    EmbassyBluetoothControllerCommandState::FirstEvent(wait),
                );
                None
            }
            EmbassyBluetoothDtmFirstDrive::Active(session) => {
                self.store_transition(
                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                    ControllerCommandStimulus::Active,
                    EmbassyBluetoothControllerCommandState::Active(
                        EmbassyBluetoothDtmSessionTask::new(session),
                    ),
                );
                None
            }
            EmbassyBluetoothDtmFirstDrive::Failed(failure) => self
                .store_first_failure(EmbassyBluetoothControllerCommandPhase::FirstEvent, failure),
        }
    }

    async fn wait_reset_stopping<WakeMutex, Recheck>(
        &mut self,
        wakers: &EmbassyBluetoothRuntimeWakers<WakeMutex>,
        recheck: &mut Recheck,
    ) where
        WakeMutex: RawMutex,
        Recheck: EmbassyBluetoothDtmControllerTimeRecheck,
    {
        let EmbassyBluetoothControllerCommandState::ResetStopping(runner) = self.owner.current()
        else {
            unreachable!("the selected Reset-stopping phase did not change")
        };
        match runner.wait() {
            Some(BluetoothDtmResetStoppingWait::Scheduler(wake)) => {
                wakers.wait_scheduler_ready(wake).await;
            }
            Some(BluetoothDtmResetStoppingWait::PostUnlink(wake)) => {
                let _ = wakers
                    .wait_post_unlink_or_recheck(wake, recheck.wait_until_absolute_recheck())
                    .await;
            }
            Some(BluetoothDtmResetStoppingWait::ControllerTime) => {
                recheck.wait_until_absolute_recheck().await;
            }
            None => {}
        }
    }

    /// Run until an externally meaningful observation or terminal lower owner.
    ///
    /// `packet` is the caller's sole reusable Host-to-Controller scratch buffer.
    /// A returned [`EmbassyBluetoothControllerCommandBoundary::NonCommand`]
    /// borrows it. Every other recoverable boundary leaves the complete actor
    /// owner stored in `self`. Cancellation of any await has the same property.
    pub async fn run<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        Recheck: EmbassyBluetoothDtmControllerTimeRecheck,
    >(
        &mut self,
        wakers: &EmbassyBluetoothRuntimeWakers<WakeMutex>,
        controller: &mut LeControllerCommandEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        packet: &'packet mut [u8],
        recheck: &mut Recheck,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let mut packet = Some(packet);
        loop {
            if recheck.status() == EmbassyBluetoothDtmControllerTimeRecheckStatus::TimelineExhausted
            {
                return self.retain_boundary(
                    EmbassyBluetoothControllerCommandBoundary::ControllerTimeExhausted,
                );
            }
            match self.phase() {
                EmbassyBluetoothControllerCommandPhase::Idle => {
                    let EmbassyBluetoothControllerCommandState::Idle(idle) = self.owner.current()
                    else {
                        unreachable!("the selected idle phase did not change")
                    };
                    if idle.wait_command_available(controller).await.is_err() {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                        );
                    }

                    let EmbassyBluetoothControllerCommandState::Idle(idle) = self.owner.take()
                    else {
                        unreachable!("the awaited idle phase did not change")
                    };
                    let buffer = packet
                        .take()
                        .expect("idle command intake retains its sole scratch buffer");
                    match idle.try_route_idle_controller_command_with_buffer(controller, buffer) {
                        BluetoothControllerIdleCommandIntake::Routed { route, buffer } => {
                            packet = Some(buffer);
                            match route {
                                BluetoothDtmIdleCommandRoute::Start(runner) => {
                                    match drive_dtm_first_ready(runner) {
                                        EmbassyBluetoothDtmFirstDrive::Wait(wait) => {
                                            self.store_transition(
                                                EmbassyBluetoothControllerCommandPhase::Idle,
                                                ControllerCommandStimulus::FirstEvent,
                                                EmbassyBluetoothControllerCommandState::FirstEvent(
                                                    wait,
                                                ),
                                            );
                                        }
                                        EmbassyBluetoothDtmFirstDrive::Active(session) => {
                                            self.store_transition(
                                                EmbassyBluetoothControllerCommandPhase::Idle,
                                                ControllerCommandStimulus::Active,
                                                EmbassyBluetoothControllerCommandState::Active(
                                                    EmbassyBluetoothDtmSessionTask::new(session),
                                                ),
                                            );
                                        }
                                        EmbassyBluetoothDtmFirstDrive::Failed(failure) => {
                                            if let Some(boundary) = self.store_first_failure(
                                                EmbassyBluetoothControllerCommandPhase::Idle,
                                                failure,
                                            ) {
                                                return boundary;
                                            }
                                        }
                                    }
                                }
                                BluetoothDtmIdleCommandRoute::StartFailed(failure) => {
                                    if let Some(boundary) = self.store_first_failure(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothDtmIdleCommandRoute::ResponsePending(pending) => {
                                    self.store_transition(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        ControllerCommandStimulus::IdleResponse,
                                        EmbassyBluetoothControllerCommandState::IdleResponse {
                                            pending,
                                            completion: EmbassyBluetoothControllerIdleCompletion::ImmediateResponse,
                                        },
                                    );
                                }
                                BluetoothDtmIdleCommandRoute::ResetBarrier(barrier) => {
                                    self.store_transition(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        ControllerCommandStimulus::IdleReset,
                                        EmbassyBluetoothControllerCommandState::IdleReset(barrier),
                                    );
                                }
                                BluetoothDtmIdleCommandRoute::EndpointMismatch(mismatch) => {
                                    return self.terminal_boundary(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        EmbassyBluetoothControllerCommandBoundary::IdleCommandEndpointMismatch(mismatch),
                                    );
                                }
                            }
                        }
                        BluetoothControllerIdleCommandIntake::Empty { task, buffer } => {
                            packet = Some(buffer);
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::Idle(task));
                        }
                        BluetoothControllerIdleCommandIntake::EndpointMismatch {
                            task,
                            buffer: _,
                        } => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::Idle(task));
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        BluetoothControllerIdleCommandIntake::Channel {
                            task,
                            buffer: _,
                            error,
                        } => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::Idle(task));
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                        BluetoothControllerIdleCommandIntake::NonCommand { task, frame } => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::Idle(task));
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::IdleReset => {
                    let EmbassyBluetoothControllerCommandState::IdleReset(barrier) =
                        self.owner.take()
                    else {
                        unreachable!("the selected idle-Reset phase did not change")
                    };
                    match barrier.complete(controller) {
                        BluetoothControllerIdleResetCompletion::ResponsePending(pending) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::IdleReset,
                                ControllerCommandStimulus::IdleResponse,
                                EmbassyBluetoothControllerCommandState::IdleResponse {
                                    pending,
                                    completion: EmbassyBluetoothControllerIdleCompletion::Reset,
                                },
                            );
                        }
                        BluetoothControllerIdleResetCompletion::EndpointMismatch(barrier) => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::IdleReset(barrier));
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::IdleResponse => {
                    let EmbassyBluetoothControllerCommandState::IdleResponse { pending, .. } =
                        self.owner.current()
                    else {
                        unreachable!("the selected idle-response phase did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                        );
                    }
                    let EmbassyBluetoothControllerCommandState::IdleResponse {
                        pending,
                        completion,
                    } = self.owner.take()
                    else {
                        unreachable!("the awaited idle-response phase did not change")
                    };
                    match pending.try_publish(controller) {
                        BluetoothControllerIdleResponsePublication::Published(idle) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::IdleResponse,
                                ControllerCommandStimulus::IdleRestored,
                                EmbassyBluetoothControllerCommandState::Idle(idle),
                            );
                            return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                completion,
                            );
                        }
                        BluetoothControllerIdleResponsePublication::Pending(pending) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::IdleResponse {
                                    pending,
                                    completion,
                                },
                            );
                        }
                        BluetoothControllerIdleResponsePublication::EndpointMismatch(pending) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::IdleResponse {
                                    pending,
                                    completion,
                                },
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        BluetoothControllerIdleResponsePublication::Fault { pending, error } => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::IdleResponse {
                                    pending,
                                    completion,
                                },
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::FirstEvent => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::FirstRetry(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::FirstRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected first-event retry did not change")
                        };
                        let (_, runner) = retry.into_parts();
                        if let Some(boundary) =
                            self.store_first_drive(drive_dtm_first_ready(runner))
                        {
                            return boundary;
                        }
                        continue;
                    }
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::FirstEvent(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::FirstEvent(wait) =
                            self.owner.current_mut()
                        else {
                            unreachable!("the selected first-event wait did not change")
                        };
                        wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                            .await;
                        let EmbassyBluetoothControllerCommandState::FirstEvent(wait) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited first-event wait did not change")
                        };
                        match wait.resume() {
                            EmbassyBluetoothDtmFirstResume::Ready(drive) => {
                                if let Some(boundary) = self.store_first_drive(drive) {
                                    return boundary;
                                }
                            }
                            EmbassyBluetoothDtmFirstResume::NotReady(wait) => self
                                .store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                    EmbassyBluetoothControllerCommandState::FirstEvent(wait),
                                ),
                        }
                    } else {
                        let EmbassyBluetoothControllerCommandState::FirstCleanup {
                            cleanup,
                            readiness,
                        } = self.owner.current()
                        else {
                            unreachable!("the selected first-event cleanup did not change")
                        };
                        if matches!(readiness, FirstCleanupReadiness::RecheckRequired) {
                            let _retained_owner = cleanup;
                            recheck.wait_until_absolute_recheck().await;
                        }
                        let EmbassyBluetoothControllerCommandState::FirstCleanup {
                            cleanup, ..
                        } = self.owner.take()
                        else {
                            unreachable!("the awaited first-event cleanup did not change")
                        };
                        match cleanup.step() {
                            BluetoothDtmFirstPreparationCleanupStep::Waiting(cleanup) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                    EmbassyBluetoothControllerCommandState::FirstCleanup {
                                        cleanup,
                                        readiness: FirstCleanupReadiness::RecheckRequired,
                                    },
                                );
                            }
                            BluetoothDtmFirstPreparationCleanupStep::Continue(cleanup) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                    EmbassyBluetoothControllerCommandState::FirstCleanup {
                                        cleanup,
                                        readiness: FirstCleanupReadiness::Ready,
                                    },
                                );
                            }
                            BluetoothDtmFirstPreparationCleanupStep::CleanTask(clean) => {
                                match clean.into_completion() {
                                    BluetoothDtmFirstPreparationCompletion::ResponsePending(
                                        pending,
                                    ) => self.store_transition(
                                        EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                        ControllerCommandStimulus::IdleResponse,
                                        EmbassyBluetoothControllerCommandState::IdleResponse {
                                            pending,
                                            completion: EmbassyBluetoothControllerIdleCompletion::DtmStartRejected,
                                        },
                                    ),
                                    BluetoothDtmFirstPreparationCompletion::FailStop(fail_stop) => {
                                        return self.terminal_boundary(
                                            EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                            EmbassyBluetoothControllerCommandBoundary::FirstPreparationFailStop(fail_stop),
                                        );
                                    }
                                }
                            }
                            BluetoothDtmFirstPreparationCleanupStep::Fault { cleanup, error } => {
                                return self.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                    EmbassyBluetoothControllerCommandBoundary::FirstPreparationCleanupFault {
                                        cleanup,
                                        error,
                                    },
                                );
                            }
                            BluetoothDtmFirstPreparationCleanupStep::RestoreRejected(cleanup) => {
                                return self.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::FirstEvent,
                                    EmbassyBluetoothControllerCommandBoundary::FirstPreparationRestoreRejected(cleanup),
                                );
                            }
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::Active => {
                    let buffer = packet
                        .take()
                        .expect("the active session retains its sole scratch buffer");
                    let EmbassyBluetoothControllerCommandState::Active(active) =
                        self.owner.current_mut()
                    else {
                        unreachable!("the selected active phase did not change")
                    };
                    let boundary = active.run(wakers, controller, buffer, recheck).await;
                    match boundary {
                        EmbassyBluetoothDtmSessionBoundary::UnownedFinishedList(index) => {
                            let EmbassyBluetoothControllerCommandState::Active(active) =
                                self.owner.take()
                            else {
                                unreachable!("unowned list retained the selected active task")
                            };
                            return self.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                EmbassyBluetoothUnownedFinishedListOwner::Active {
                                    _task: active,
                                    index,
                                },
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::ResetBarrier(barrier) => {
                            let EmbassyBluetoothControllerCommandState::Active(active) =
                                self.owner.take()
                            else {
                                unreachable!("active Reset transferred the selected session")
                            };
                            debug_assert!(active.is_empty());
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                ControllerCommandStimulus::ResetStopping,
                                EmbassyBluetoothControllerCommandState::ResetStopping(
                                    barrier.begin_quiescence(),
                                ),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::NonCommand(frame) => {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::ControllerCommandEndpointMismatch(
                            mismatch,
                        ) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                EmbassyBluetoothControllerCommandBoundary::ActiveCommandEndpointMismatch(mismatch),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::EndpointMismatch => {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::HciFault(error) => {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::Retryable(retry) => {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::Retryable(
                                    EmbassyBluetoothControllerRetry::Active(retry),
                                ),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::ControllerTimeExhausted => {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::ControllerTimeExhausted,
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::PendingRadioFault(fault) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                EmbassyBluetoothControllerCommandBoundary::PendingRadioFault(fault),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::CommandReadyRadioFault(fault) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                EmbassyBluetoothControllerCommandBoundary::CommandReadyRadioFault(
                                    fault,
                                ),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::StoppingFault(fault) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                EmbassyBluetoothControllerCommandBoundary::TestEndStoppingFault(
                                    fault,
                                ),
                            );
                        }
                        EmbassyBluetoothDtmSessionBoundary::Complete(idle) => {
                            let _empty = self.owner.take();
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::Active,
                                ControllerCommandStimulus::IdleRestored,
                                EmbassyBluetoothControllerCommandState::Idle(idle),
                            );
                            return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                EmbassyBluetoothControllerIdleCompletion::TestEnd,
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::ResetStopping => {
                    self.wait_reset_stopping(wakers, recheck).await;
                    let EmbassyBluetoothControllerCommandState::ResetStopping(runner) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited Reset-stopping phase did not change")
                    };
                    match runner.step() {
                        BluetoothDtmResetStoppingStep::Continue(runner)
                        | BluetoothDtmResetStoppingStep::Waiting(runner) => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::ResetStopping(
                                    runner,
                                ))
                        }
                        BluetoothDtmResetStoppingStep::UnrelatedList { runner, observed } => {
                            return self.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::ResetStopping,
                                EmbassyBluetoothUnownedFinishedListOwner::ResetStopping {
                                    _runner: runner,
                                    observed,
                                },
                            );
                        }
                        BluetoothDtmResetStoppingStep::Retryable(runner) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::ResetStopping(runner),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::Retryable(
                                    EmbassyBluetoothControllerRetry::ResetStopping,
                                ),
                            );
                        }
                        BluetoothDtmResetStoppingStep::CompletionReady(ready) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::ResetStopping,
                                ControllerCommandStimulus::ResetCompletion,
                                EmbassyBluetoothControllerCommandState::ResetCompletion(ready),
                            );
                        }
                        BluetoothDtmResetStoppingStep::RestoreFailed(failure) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::ResetStopping,
                                ControllerCommandStimulus::ResetRestore,
                                EmbassyBluetoothControllerCommandState::ResetRestore(failure),
                            );
                        }
                        BluetoothDtmResetStoppingStep::Fault(fault) => {
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::ResetStopping,
                                EmbassyBluetoothControllerCommandBoundary::ResetStoppingFault(
                                    fault,
                                ),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::ResetRestore => {
                    let EmbassyBluetoothControllerCommandState::ResetRestore(failure) =
                        self.owner.take()
                    else {
                        unreachable!("the selected Reset-restore phase did not change")
                    };
                    match failure.retry_restore() {
                        BluetoothDtmResetRestoreStep::CompletionReady(ready) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::ResetRestore,
                                ControllerCommandStimulus::ResetCompletion,
                                EmbassyBluetoothControllerCommandState::ResetCompletion(ready),
                            );
                        }
                        BluetoothDtmResetRestoreStep::Rejected(failure) => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::ResetRestore(
                                    failure,
                                ));
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::Retryable(
                                    EmbassyBluetoothControllerRetry::ResetRestore,
                                ),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::ResetCompletion => {
                    let EmbassyBluetoothControllerCommandState::ResetCompletion(ready) =
                        self.owner.take()
                    else {
                        unreachable!("the selected Reset-completion phase did not change")
                    };
                    match ready.complete(controller) {
                        BluetoothDtmResetCompletionStart::ResponsePending(pending) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::ResetCompletion,
                                ControllerCommandStimulus::ResetResponse,
                                EmbassyBluetoothControllerCommandState::ResetResponse(pending),
                            );
                        }
                        BluetoothDtmResetCompletionStart::EndpointMismatch(ready) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::ResetCompletion(ready),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::ResetResponse => {
                    let EmbassyBluetoothControllerCommandState::ResetResponse(pending) =
                        self.owner.current()
                    else {
                        unreachable!("the selected Reset-response phase did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                        );
                    }
                    let EmbassyBluetoothControllerCommandState::ResetResponse(pending) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited Reset-response phase did not change")
                    };
                    match pending.try_publish(controller) {
                        BluetoothDtmResetResponsePublication::Completed(complete) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::ResetResponse,
                                ControllerCommandStimulus::IdleRestored,
                                EmbassyBluetoothControllerCommandState::Idle(
                                    complete.into_idle_command_task(),
                                ),
                            );
                            return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                EmbassyBluetoothControllerIdleCompletion::Reset,
                            );
                        }
                        BluetoothDtmResetResponsePublication::Pending(pending) => {
                            self.owner
                                .store(EmbassyBluetoothControllerCommandState::ResetResponse(
                                    pending,
                                ))
                        }
                        BluetoothDtmResetResponsePublication::EndpointMismatch(pending) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::ResetResponse(pending),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        BluetoothDtmResetResponsePublication::Fault { pending, error } => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::ResetResponse(pending),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::UnownedFinishedList => {
                    return self.retained_unowned_finished_list();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{future::pending, pin::Pin, task::Context};

    use std::boxed::Box;

    use super::{
        ControllerCommandAction, ControllerCommandStimulus, ControllerOwnerSlot,
        EmbassyBluetoothControllerCommandPhase, reduce_controller_command_transition,
    };

    #[test]
    fn reducer_closes_start_test_end_and_reset_paths_back_to_idle() {
        use ControllerCommandAction::{Advance, Retain, Terminal};
        use ControllerCommandStimulus::{
            Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, ResetCompletion,
            ResetResponse, ResetRestore, ResetStopping, UnownedFinishedList,
        };
        use EmbassyBluetoothControllerCommandPhase::{
            Active as ActivePhase, FirstEvent as FirstEventPhase, Idle,
            IdleReset as IdleResetPhase, IdleResponse as IdleResponsePhase,
            ResetCompletion as ResetCompletionPhase, ResetResponse as ResetResponsePhase,
            ResetRestore as ResetRestorePhase, ResetStopping as ResetStoppingPhase,
            UnownedFinishedList as UnownedFinishedListPhase,
        };

        assert_eq!(
            reduce_controller_command_transition(Idle, IdleResponse),
            Advance(IdleResponsePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(IdleResponsePhase, IdleRestored),
            Advance(Idle)
        );
        assert_eq!(
            reduce_controller_command_transition(Idle, IdleReset),
            Advance(IdleResetPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(IdleResetPhase, IdleResponse),
            Advance(IdleResponsePhase)
        );

        assert_eq!(
            reduce_controller_command_transition(Idle, FirstEvent),
            Advance(FirstEventPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(Idle, Active),
            Advance(ActivePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(FirstEventPhase, Active),
            Advance(ActivePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(FirstEventPhase, IdleResponse),
            Advance(IdleResponsePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(ActivePhase, IdleRestored),
            Advance(Idle)
        );
        assert_eq!(
            reduce_controller_command_transition(ActivePhase, ResetStopping),
            Advance(ResetStoppingPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(ActivePhase, UnownedFinishedList),
            Advance(UnownedFinishedListPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(ResetStoppingPhase, UnownedFinishedList),
            Advance(UnownedFinishedListPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(UnownedFinishedListPhase, UnownedFinishedList),
            Retain
        );
        assert_eq!(
            reduce_controller_command_transition(ResetStoppingPhase, ResetRestore),
            Advance(ResetRestorePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(ResetRestorePhase, ResetCompletion),
            Advance(ResetCompletionPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(ResetCompletionPhase, ResetResponse),
            Advance(ResetResponsePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(ResetResponsePhase, IdleRestored),
            Advance(Idle)
        );
        assert_eq!(
            reduce_controller_command_transition(
                FirstEventPhase,
                ControllerCommandStimulus::Terminal,
            ),
            Terminal
        );
    }

    #[test]
    fn retained_observation_does_not_empty_or_replace_owner_slot() {
        let slot = ControllerOwnerSlot::new(37_u8);
        assert_eq!(*slot.current(), 37);
        assert!(!slot.is_empty());
        assert_eq!(
            reduce_controller_command_transition(
                EmbassyBluetoothControllerCommandPhase::FirstEvent,
                ControllerCommandStimulus::Retain,
            ),
            ControllerCommandAction::Retain
        );
        assert_eq!(
            reduce_controller_command_transition(
                EmbassyBluetoothControllerCommandPhase::Active,
                ControllerCommandStimulus::Retain,
            ),
            ControllerCommandAction::Retain
        );
    }

    #[test]
    #[should_panic(expected = "invalid Controller command actor transition")]
    fn response_backpressure_cannot_be_misclassified_as_terminal() {
        let _ = reduce_controller_command_transition(
            EmbassyBluetoothControllerCommandPhase::ResetResponse,
            ControllerCommandStimulus::Terminal,
        );
    }

    #[test]
    fn owner_slot_transfers_exactly_once() {
        let mut slot = ControllerOwnerSlot::new(41_u8);
        assert_eq!(slot.take(), 41);
        assert!(slot.is_empty());
        slot.store(43);
        assert_eq!(*slot.current_mut(), 43);
    }

    #[test]
    fn cancelling_borrowed_wait_leaves_exact_actor_owner_in_slot() {
        async fn wait_forever(owner: &u8) {
            let _retained_owner = owner;
            pending::<()>().await;
        }

        let slot = ControllerOwnerSlot::new(47_u8);
        let mut future = Box::pin(wait_forever(slot.current()));
        let mut context = Context::from_waker(std::task::Waker::noop());
        assert!(Pin::as_mut(&mut future).poll(&mut context).is_pending());
        drop(future);

        assert_eq!(*slot.current(), 47);
        assert!(!slot.is_empty());
    }
}
