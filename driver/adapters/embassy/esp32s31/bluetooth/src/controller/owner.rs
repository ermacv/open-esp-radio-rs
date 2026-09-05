//! Affine Controller state, owner slots, and retained transition results.

#[cfg(target_arch = "riscv32")]
use super::*;

#[cfg(target_arch = "riscv32")]
pub(super) enum EmbassyBluetoothUnownedFinishedListOwner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    LegacyAdvertising {
        _session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyAdvertisingPending {
        _pending: BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyAdvertisingStopping {
        _stopping: BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingInitialPending {
        _pending: BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingActive {
        _active: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingPending {
        _pending: BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingStopping {
        _stopping: BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    PassiveScan {
        _session: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    PassiveScanPending {
        _pending: BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    PassiveScanStopping {
        _stopping: BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
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
    pub(super) const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        match self {
            Self::LegacyAdvertising { observed, .. }
            | Self::LegacyAdvertisingPending { observed, .. }
            | Self::LegacyAdvertisingStopping { observed, .. }
            | Self::LegacyConnectableAdvertisingInitialPending { observed, .. }
            | Self::LegacyConnectableAdvertisingActive { observed, .. }
            | Self::LegacyConnectableAdvertisingPending { observed, .. }
            | Self::LegacyConnectableAdvertisingStopping { observed, .. }
            | Self::PassiveScan { observed, .. }
            | Self::PassiveScanPending { observed, .. }
            | Self::PassiveScanStopping { observed, .. } => observed.index(),
            Self::Active { index, .. } => *index,
            Self::ResetStopping { observed, .. } => observed.index(),
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy)]
pub(super) enum FirstCleanupReadiness {
    Ready,
    RecheckRequired,
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy)]
pub(super) enum LegacyAdvertisingStopOrigin {
    LegacyAdvertising,
    LegacyConnectableAdvertising,
}

#[cfg(target_arch = "riscv32")]
impl LegacyAdvertisingStopOrigin {
    pub(super) const fn disable_completion(self) -> EmbassyBluetoothControllerIdleCompletion {
        match self {
            Self::LegacyAdvertising => {
                EmbassyBluetoothControllerIdleCompletion::LegacyAdvertisingDisable
            }
            Self::LegacyConnectableAdvertising => {
                EmbassyBluetoothControllerIdleCompletion::LegacyConnectableAdvertisingDisable
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(super) enum EmbassyBluetoothControllerCommandState<'runtime, S, const CAPACITY: usize>
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
    LegacyAdvertisingFirst(
        EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingRetry(BluetoothLegacyAdvertisingFirstRunnerRetry<'runtime, S, CAPACITY>),
    LegacyAdvertisingResponse(
        BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingActive(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    LegacyAdvertisingActiveResponse(
        BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingStopping(BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    LegacyAdvertisingCpuOwned(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    LegacyAdvertisingCpuResponse(
        BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingDisableRestore(
        BluetoothLegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingDisableResponse {
        pending: BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        origin: LegacyAdvertisingStopOrigin,
    },
    LegacyAdvertisingResetRestore(BluetoothLegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
    LegacyAdvertisingResetCompletion {
        ready: BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
        origin: LegacyAdvertisingStopOrigin,
    },
    LegacyAdvertisingResetResponse {
        pending: BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>,
        origin: LegacyAdvertisingStopOrigin,
    },
    LegacyAdvertisingRecurring(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    LegacyAdvertisingRecurringRetry(
        BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingRecurringStopRestore(
        BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingFirst(
        EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRetry(
        BluetoothLegacyConnectableAdvertisingFirstRunnerRetry<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingResponse(
        BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingActive(
        BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingActiveResponse(
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingStopping(
        BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringCommandWait(
        ConnectableRecurringCommandWait<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringCommandGraphPrepared(
        ConnectableRecurringCommandGraphPrepared<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringCommandCandidate(
        ConnectableRecurringCommandCandidate<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringCommandPrepared(
        ConnectableRecurringCommandPrepared<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringCommandMerged(
        ConnectableRecurringCommandMerged<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringResponseWait(
        ConnectableRecurringResponseWait<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringResponseGraphPrepared(
        ConnectableRecurringResponseGraphPrepared<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringResponseCandidate(
        ConnectableRecurringResponseCandidate<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringResponsePrepared(
        ConnectableRecurringResponsePrepared<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringResponseMerged(
        ConnectableRecurringResponseMerged<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRecurringCancellation(
        EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait<
            'runtime,
            S,
            CAPACITY,
        >,
    ),
    PeripheralConnectionFirst(
        EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    PeripheralConnectionFirstRetry(
        EmbassyBluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>,
    ),
    PeripheralConnectionActive(
        BluetoothLegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>,
    ),
    PassiveScanFirst(EmbassyBluetoothPassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
    PassiveScanRetry(BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
    PassiveScanResponse(BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    PassiveScanActive(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    PassiveScanActiveResponse(BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    PassiveScanStopping(BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>),
    PassiveScanReports(BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    PassiveScanComplete(BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>),
    PassiveScanCpuResponse(BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    PassiveScanRecurring(BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    PassiveScanRecurringRetry(BluetoothPassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
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
    pub(super) const fn phase(&self) -> EmbassyBluetoothControllerCommandPhase {
        match self {
            Self::Idle(_) => EmbassyBluetoothControllerCommandPhase::Idle,
            Self::IdleReset(_) => EmbassyBluetoothControllerCommandPhase::IdleReset,
            Self::IdleResponse { .. } => EmbassyBluetoothControllerCommandPhase::IdleResponse,
            Self::FirstEvent(_) | Self::FirstRetry(_) | Self::FirstCleanup { .. } => {
                EmbassyBluetoothControllerCommandPhase::FirstEvent
            }
            Self::LegacyAdvertisingFirst(_) | Self::LegacyAdvertisingRetry(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst
            }
            Self::LegacyAdvertisingResponse(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingResponse
            }
            Self::LegacyAdvertisingActive(_)
            | Self::LegacyAdvertisingActiveResponse(_)
            | Self::LegacyAdvertisingStopping(_)
            | Self::LegacyAdvertisingCpuOwned(_)
            | Self::LegacyAdvertisingCpuResponse(_)
            | Self::LegacyAdvertisingDisableRestore(_)
            | Self::LegacyAdvertisingResetRestore(_)
            | Self::LegacyAdvertisingRecurring(_)
            | Self::LegacyAdvertisingRecurringRetry(_)
            | Self::LegacyAdvertisingRecurringStopRestore(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive
            }
            Self::LegacyAdvertisingDisableResponse { .. }
            | Self::LegacyAdvertisingResetCompletion { .. }
            | Self::LegacyAdvertisingResetResponse { .. } => {
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingStopCompletion
            }
            Self::LegacyConnectableAdvertisingFirst(_)
            | Self::LegacyConnectableAdvertisingRetry(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst
            }
            Self::LegacyConnectableAdvertisingResponse(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse
            }
            Self::LegacyConnectableAdvertisingActive(_)
            | Self::LegacyConnectableAdvertisingActiveResponse(_)
            | Self::LegacyConnectableAdvertisingStopping(_)
            | Self::LegacyConnectableAdvertisingRecurringCommandWait(_)
            | Self::LegacyConnectableAdvertisingRecurringCommandGraphPrepared(_)
            | Self::LegacyConnectableAdvertisingRecurringCommandCandidate(_)
            | Self::LegacyConnectableAdvertisingRecurringCommandPrepared(_)
            | Self::LegacyConnectableAdvertisingRecurringCommandMerged(_)
            | Self::LegacyConnectableAdvertisingRecurringResponseWait(_)
            | Self::LegacyConnectableAdvertisingRecurringResponseGraphPrepared(_)
            | Self::LegacyConnectableAdvertisingRecurringResponseCandidate(_)
            | Self::LegacyConnectableAdvertisingRecurringResponsePrepared(_)
            | Self::LegacyConnectableAdvertisingRecurringResponseMerged(_)
            | Self::LegacyConnectableAdvertisingRecurringCancellation(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive
            }
            Self::PeripheralConnectionFirst(_) | Self::PeripheralConnectionFirstRetry(_) => {
                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst
            }
            Self::PeripheralConnectionActive(_) => {
                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionActive
            }
            Self::PassiveScanFirst(_) | Self::PassiveScanRetry(_) => {
                EmbassyBluetoothControllerCommandPhase::PassiveScanFirst
            }
            Self::PassiveScanResponse(_) => {
                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse
            }
            Self::PassiveScanActive(_)
            | Self::PassiveScanActiveResponse(_)
            | Self::PassiveScanStopping(_)
            | Self::PassiveScanReports(_)
            | Self::PassiveScanComplete(_)
            | Self::PassiveScanCpuResponse(_)
            | Self::PassiveScanRecurring(_)
            | Self::PassiveScanRecurringRetry(_) => {
                EmbassyBluetoothControllerCommandPhase::PassiveScanActive
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

#[cfg(any(target_arch = "riscv32", test))]
pub(super) struct ControllerOwnerSlot<State> {
    state: Option<State>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<State> ControllerOwnerSlot<State> {
    pub(super) const fn new(state: State) -> Self {
        Self { state: Some(state) }
    }

    pub(super) fn current(&self) -> &State {
        self.state
            .as_ref()
            .expect("a live Controller command actor retains one affine owner")
    }

    pub(super) fn current_mut(&mut self) -> &mut State {
        self.state
            .as_mut()
            .expect("a live Controller command actor retains one affine owner")
    }

    pub(super) fn take(&mut self) -> State {
        self.state
            .take()
            .expect("a Controller transition consumes its owner exactly once")
    }

    pub(super) fn store(&mut self, state: State) {
        assert!(
            self.state.replace(state).is_none(),
            "a Controller transition cannot overwrite an affine owner"
        );
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.state.is_none()
    }
}

/// Sole executor-side owner of the idle and active radio lifecycles.
#[cfg(target_arch = "riscv32")]
#[must_use = "run the Controller actor until it returns a terminal lower owner"]
pub struct EmbassyBluetoothControllerCommandTask<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) owner:
        ControllerOwnerSlot<EmbassyBluetoothControllerCommandState<'runtime, S, CAPACITY>>,
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

    pub(super) fn store_transition(
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

    pub(super) fn store_retained_state(
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

    pub(super) fn retain_boundary<'epoch, 'packet>(
        &self,
        boundary: EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
    ) -> EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        assert_eq!(
            reduce_controller_command_transition(self.phase(), ControllerCommandStimulus::Retain,),
            ControllerCommandAction::Retain,
        );
        boundary
    }

    pub(super) fn store_unowned_finished_list<'epoch, 'packet>(
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

    pub(super) fn retained_unowned_finished_list<'epoch, 'packet>(
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

    pub(super) fn terminal_boundary<'epoch, 'packet>(
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

    pub(super) fn store_first_failure<'epoch, 'packet>(
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

    pub(super) fn store_first_drive<'epoch, 'packet>(
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

    pub(super) fn store_legacy_advertising_failure<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        failure: BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match failure.into_hardware_failure_response() {
            Ok(pending) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleResponse,
                    EmbassyBluetoothControllerCommandState::IdleResponse {
                        pending,
                        completion:
                            EmbassyBluetoothControllerIdleCompletion::LegacyAdvertisingStartRejected,
                    },
                );
                None
            }
            Err(BluetoothLegacyAdvertisingFirstRunnerFailure::Retryable(retry)) => {
                let state = EmbassyBluetoothControllerCommandState::LegacyAdvertisingRetry(retry);
                if from == EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::LegacyAdvertisingFirst,
                        state,
                    );
                }
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyAdvertisingFirst,
                    )),
                )
            }
            Err(_) => unreachable!("only a pre-RUN retry lacks recovered idle ownership"),
        }
    }

    pub(super) fn store_legacy_advertising_drive<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        drive: EmbassyBluetoothLegacyAdvertisingFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match drive {
            EmbassyBluetoothLegacyAdvertisingFirstDrive::Wait(wait) => {
                let state = EmbassyBluetoothControllerCommandState::LegacyAdvertisingFirst(wait);
                if from == EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::LegacyAdvertisingFirst,
                        state,
                    );
                }
                None
            }
            EmbassyBluetoothLegacyAdvertisingFirstDrive::Running(running) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::LegacyAdvertisingResponse,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResponse(
                        running.into_response_pending_session(),
                    ),
                );
                None
            }
            EmbassyBluetoothLegacyAdvertisingFirstDrive::Failed(failure) => {
                self.store_legacy_advertising_failure(from, failure)
            }
        }
    }

    pub(super) fn store_legacy_connectable_advertising_failure<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        failure: BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match failure {
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::Recovered(recovered) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleResponse,
                    EmbassyBluetoothControllerCommandState::IdleResponse {
                        pending: recovered.into_hardware_failure_response(),
                        completion:
                            EmbassyBluetoothControllerIdleCompletion::LegacyConnectableAdvertisingStartRejected,
                    },
                );
                None
            }
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::RetryablePrePublication(
                retry,
            ) => {
                let state =
                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRetry(
                        retry,
                    );
                if from == EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst
                {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::LegacyConnectableAdvertisingFirst,
                        state,
                    );
                }
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyConnectableAdvertisingFirst,
                    )),
                )
            }
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(failure) => {
                Some(self.terminal_boundary(
                    from,
                    EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingFailStop(
                        failure,
                    ),
                ))
            }
        }
    }

    pub(super) fn store_legacy_connectable_advertising_drive<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        drive: EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match drive {
            EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive::Wait(wait) => {
                let state =
                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingFirst(wait);
                if from == EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst
                {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::LegacyConnectableAdvertisingFirst,
                        state,
                    );
                }
                None
            }
            EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive::Running(running) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::LegacyConnectableAdvertisingResponse,
                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(
                        running.into_response_pending(),
                    ),
                );
                None
            }
            EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive::Failed(failure) => {
                self.store_legacy_connectable_advertising_failure(from, failure)
            }
        }
    }

    pub(super) fn store_peripheral_connection_first_drive<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        drive: EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match drive {
            ControlFlow::Break(failure) => Some(self.terminal_boundary(
                from,
                EmbassyBluetoothControllerCommandBoundary::PeripheralConnectionFirstFailStop(
                    failure,
                ),
            )),
            ControlFlow::Continue(
                EmbassyBluetoothLegacyConnectablePeripheralFirstDrive::WaitControllerTime(wait),
            ) => {
                let state = EmbassyBluetoothControllerCommandState::PeripheralConnectionFirst(wait);
                if from == EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::PeripheralConnectionFirst,
                        state,
                    );
                }
                None
            }
            ControlFlow::Continue(
                EmbassyBluetoothLegacyConnectablePeripheralFirstDrive::Retry(retry),
            ) => {
                let state =
                    EmbassyBluetoothControllerCommandState::PeripheralConnectionFirstRetry(retry);
                if from == EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::PeripheralConnectionFirst,
                        state,
                    );
                }
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::PeripheralConnectionFirst,
                    )),
                )
            }
            ControlFlow::Continue(
                EmbassyBluetoothLegacyConnectablePeripheralFirstDrive::Running(running),
            ) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::PeripheralConnectionActive,
                    EmbassyBluetoothControllerCommandState::PeripheralConnectionActive(running),
                );
                Some(EmbassyBluetoothControllerCommandBoundary::PeripheralConnectionActive)
            }
        }
    }

    pub(super) fn store_peripheral_connection_stopping_step<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        step: EmbassyBluetoothLegacyConnectablePeripheralFirstStoppingStep<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match step {
            ControlFlow::Continue(drive) => {
                self.store_peripheral_connection_first_drive(from, drive)
            }
            ControlFlow::Break(
                BluetoothLegacyConnectablePeripheralFirstHciResetOutcome::Ready(ready),
            ) => {
                let (reset, _reset_evidence) = ready.into_parts();
                // The evidence has discharged its purpose once exact cancellation
                // produced the idle Reset barrier; no runtime owner is discarded.
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleReset,
                    EmbassyBluetoothControllerCommandState::IdleReset(reset),
                );
                None
            }
            ControlFlow::Break(
                BluetoothLegacyConnectablePeripheralFirstHciResetOutcome::FailStop(failure),
            ) => Some(self.terminal_boundary(
                from,
                EmbassyBluetoothControllerCommandBoundary::PeripheralConnectionResetFailStop(
                    failure,
                ),
            )),
        }
    }

    pub(super) fn store_passive_scan_failure<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        failure: BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match failure.into_hardware_failure_response() {
            Ok(pending) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleResponse,
                    EmbassyBluetoothControllerCommandState::IdleResponse {
                        pending,
                        completion:
                            EmbassyBluetoothControllerIdleCompletion::PassiveScanStartRejected,
                    },
                );
                None
            }
            Err(failure) if failure.retry_cause().is_some() => {
                let state = EmbassyBluetoothControllerCommandState::PassiveScanRetry(failure);
                if from == EmbassyBluetoothControllerCommandPhase::PassiveScanFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::PassiveScanFirst, state);
                }
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::PassiveScanFirst,
                    )),
                )
            }
            Err(_) => unreachable!("only a retryable pre-RUN edge lacks idle ownership"),
        }
    }

    pub(super) fn store_passive_scan_drive<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        drive: EmbassyBluetoothPassiveScanFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        match drive {
            EmbassyBluetoothPassiveScanFirstDrive::Wait(wait) => {
                let state = EmbassyBluetoothControllerCommandState::PassiveScanFirst(wait);
                if from == EmbassyBluetoothControllerCommandPhase::PassiveScanFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::PassiveScanFirst, state);
                }
                None
            }
            EmbassyBluetoothPassiveScanFirstDrive::Running(running) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::PassiveScanResponse,
                    EmbassyBluetoothControllerCommandState::PassiveScanResponse(
                        running.into_response_pending_session(),
                    ),
                );
                None
            }
            EmbassyBluetoothPassiveScanFirstDrive::Failed(failure) => {
                self.store_passive_scan_failure(from, failure)
            }
        }
    }

    pub(super) fn store_passive_scan_recurring_drive<'epoch, 'packet>(
        &mut self,
        drive: EmbassyBluetoothPassiveScanRecurringDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        let phase = EmbassyBluetoothControllerCommandPhase::PassiveScanActive;
        match drive {
            EmbassyBluetoothPassiveScanRecurringDrive::Wait(runner) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::PassiveScanRecurring(runner),
                );
                None
            }
            EmbassyBluetoothPassiveScanRecurringDrive::Active(active) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::PassiveScanActive(active),
                );
                None
            }
            EmbassyBluetoothPassiveScanRecurringDrive::Failed(failure)
                if failure.retry_cause().is_some() =>
            {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::PassiveScanRecurringRetry(failure),
                );
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::PassiveScanRecurring,
                    )),
                )
            }
            EmbassyBluetoothPassiveScanRecurringDrive::Failed(failure) => {
                Some(self.terminal_boundary(
                    phase,
                    EmbassyBluetoothControllerCommandBoundary::PassiveScanRecurringFault(failure),
                ))
            }
        }
    }

    pub(super) fn store_legacy_advertising_recurring_drive<'epoch, 'packet>(
        &mut self,
        drive: EmbassyBluetoothLegacyAdvertisingRecurringDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        let phase = EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive;
        match drive {
            EmbassyBluetoothLegacyAdvertisingRecurringDrive::Wait(runner) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(runner),
                );
                None
            }
            EmbassyBluetoothLegacyAdvertisingRecurringDrive::Active(active) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(active),
                );
                None
            }
            EmbassyBluetoothLegacyAdvertisingRecurringDrive::ActiveResponsePending(pending) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                        pending,
                    ),
                );
                None
            }
            EmbassyBluetoothLegacyAdvertisingRecurringDrive::Stopping(stopping) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingStopping(stopping),
                );
                None
            }
            EmbassyBluetoothLegacyAdvertisingRecurringDrive::Retryable(retry) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringRetry(retry),
                );
                Some(
                    self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::Retryable(
                        EmbassyBluetoothControllerRetry::LegacyAdvertisingRecurring,
                    )),
                )
            }
            EmbassyBluetoothLegacyAdvertisingRecurringDrive::Fault(fault) => {
                Some(self.terminal_boundary(
                    phase,
                    EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringFault(
                        fault,
                    ),
                ))
            }
        }
    }

    pub(super) fn store_connectable_recurring_stop_drive<'epoch, 'packet>(
        &mut self,
        from: EmbassyBluetoothControllerCommandPhase,
        drive: ConnectableRecurringStopDrive<'runtime, S, CAPACITY>,
    ) -> Option<EmbassyBluetoothControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>
    {
        let phase = from;
        match drive {
            ConnectableRecurringStopDrive::Disable(pending) => {
                self.store_transition(
                    phase,
                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                        pending,
                        origin: LegacyAdvertisingStopOrigin::LegacyConnectableAdvertising,
                    },
                );
                None
            }
            ConnectableRecurringStopDrive::Reset(ready) => {
                self.store_transition(
                    phase,
                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion {
                        ready,
                        origin: LegacyAdvertisingStopOrigin::LegacyConnectableAdvertising,
                    },
                );
                None
            }
            ConnectableRecurringStopDrive::Wait(wait) => {
                self.store_retained_state(
                    phase,
                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(
                        wait,
                    ),
                );
                None
            }
            ConnectableRecurringStopDrive::FailStop(failure) => {
                Some(self.terminal_boundary(
                    phase,
                    EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingRecurringFailStop(
                        EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop {
                            _owner: recurring::FailStopOwner::Stopping(failure),
                        },
                    ),
                ))
            }
        }
    }
}
