//! Affine Controller state, owner slots, and retained transition results.

#[cfg(target_arch = "riscv32")]
use super::*;

#[cfg(target_arch = "riscv32")]
pub(super) enum UnownedFinishedListOwner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    LegacyAdvertising {
        _session: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyAdvertisingPending {
        _pending: LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyAdvertisingStopping {
        _stopping: LegacyAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingInitialPending {
        _pending: LegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingActive {
        _active: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingPending {
        _pending: LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    LegacyConnectableAdvertisingStopping {
        _stopping: LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    PassiveScan {
        _session: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    PassiveScanPending {
        _pending: PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    PassiveScanStopping {
        _stopping: PassiveScanHciStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Active {
        _task: DtmSessionTask<'runtime, S, CAPACITY>,
        index: BluetoothSchedulerHardwareListIndex,
    },
    ResetStopping {
        _runner: DtmResetStoppingRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> UnownedFinishedListOwner<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
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
    pub(super) const fn disable_completion(self) -> ControllerIdleCompletion {
        match self {
            Self::LegacyAdvertising => ControllerIdleCompletion::LegacyAdvertisingDisable,
            Self::LegacyConnectableAdvertising => {
                ControllerIdleCompletion::LegacyConnectableAdvertisingDisable
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(super) enum ControllerCommandState<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Idle(ControllerIdleCommandTask<'runtime, S, CAPACITY>),
    IdleReset(ControllerIdleResetBarrier<'runtime, S, CAPACITY>),
    IdleResponse {
        pending: ControllerIdleResponsePending<'runtime, S, CAPACITY>,
        completion: ControllerIdleCompletion,
    },
    FirstEvent(DtmFirstControllerTimeWait<'runtime, S, CAPACITY>),
    FirstRetry(DtmFirstRunnerRetry<'runtime, S, CAPACITY>),
    FirstCleanup {
        cleanup: DtmFirstPreparationCleanup<'runtime, S, CAPACITY>,
        readiness: FirstCleanupReadiness,
    },
    LegacyAdvertisingFirst(LegacyAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>),
    LegacyAdvertisingRetry(LegacyAdvertisingFirstRunnerRetry<'runtime, S, CAPACITY>),
    LegacyAdvertisingResponse(LegacyAdvertisingResponsePendingSession<'runtime, S, CAPACITY>),
    LegacyAdvertisingActive(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    LegacyAdvertisingActiveResponse(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    LegacyAdvertisingStopping(LegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    LegacyAdvertisingCpuOwned(LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    LegacyAdvertisingCpuResponse(LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    LegacyAdvertisingDisableRestore(LegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
    LegacyAdvertisingDisableResponse {
        pending: LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        origin: LegacyAdvertisingStopOrigin,
    },
    LegacyAdvertisingResetRestore(LegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
    LegacyAdvertisingResetCompletion {
        ready: LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
        origin: LegacyAdvertisingStopOrigin,
    },
    LegacyAdvertisingResetResponse {
        pending: LegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>,
        origin: LegacyAdvertisingStopOrigin,
    },
    LegacyAdvertisingRecurring(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    LegacyAdvertisingRecurringRetry(LegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>),
    LegacyAdvertisingRecurringStopRestore(
        LegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingFirst(
        LegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingRetry(
        LegacyConnectableAdvertisingFirstRunnerRetry<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingResponse(
        LegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingActive(
        LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingActiveResponse(
        LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyConnectableAdvertisingStopping(
        LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
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
        LegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, CAPACITY>,
    ),
    PeripheralConnectionFirst(
        LegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    PeripheralConnectionFirstRetry(PeripheralFirstSessionRetry<'runtime, S, CAPACITY>),
    PeripheralConnectionActive(PeripheralConnectionActiveSession<'runtime, S, CAPACITY>),
    PassiveScanFirst(PassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
    PassiveScanRetry(PassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
    PassiveScanResponse(PassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    PassiveScanActive(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    PassiveScanActiveResponse(PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    PassiveScanStopping(PassiveScanHciStopping<'runtime, S, CAPACITY>),
    PassiveScanReports(PassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    PassiveScanComplete(PassiveScanHciReportsComplete<'runtime, S, CAPACITY>),
    PassiveScanCpuResponse(PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    PassiveScanRecurring(PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    PassiveScanRecurringRetry(PassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
    Active(DtmSessionTask<'runtime, S, CAPACITY>),
    ResetStopping(DtmResetStoppingRunner<'runtime, S, CAPACITY>),
    ResetRestore(DtmResetRestoreFailure<'runtime, S, CAPACITY>),
    ResetCompletion(DtmResetCompletionReady<'runtime, S, CAPACITY>),
    ResetResponse(DtmResetResponsePending<'runtime, S, CAPACITY>),
    UnownedFinishedList(UnownedFinishedListOwner<'runtime, S, CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> ControllerCommandState<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) const fn phase(&self) -> ControllerCommandPhase {
        match self {
            Self::Idle(_) => ControllerCommandPhase::Idle,
            Self::IdleReset(_) => ControllerCommandPhase::IdleReset,
            Self::IdleResponse { .. } => ControllerCommandPhase::IdleResponse,
            Self::FirstEvent(_) | Self::FirstRetry(_) | Self::FirstCleanup { .. } => {
                ControllerCommandPhase::FirstEvent
            }
            Self::LegacyAdvertisingFirst(_) | Self::LegacyAdvertisingRetry(_) => {
                ControllerCommandPhase::LegacyAdvertisingFirst
            }
            Self::LegacyAdvertisingResponse(_) => ControllerCommandPhase::LegacyAdvertisingResponse,
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
                ControllerCommandPhase::LegacyAdvertisingActive
            }
            Self::LegacyAdvertisingDisableResponse { .. }
            | Self::LegacyAdvertisingResetCompletion { .. }
            | Self::LegacyAdvertisingResetResponse { .. } => {
                ControllerCommandPhase::LegacyAdvertisingStopCompletion
            }
            Self::LegacyConnectableAdvertisingFirst(_)
            | Self::LegacyConnectableAdvertisingRetry(_) => {
                ControllerCommandPhase::LegacyConnectableAdvertisingFirst
            }
            Self::LegacyConnectableAdvertisingResponse(_) => {
                ControllerCommandPhase::LegacyConnectableAdvertisingResponse
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
                ControllerCommandPhase::LegacyConnectableAdvertisingActive
            }
            Self::PeripheralConnectionFirst(_) | Self::PeripheralConnectionFirstRetry(_) => {
                ControllerCommandPhase::PeripheralConnectionFirst
            }
            Self::PeripheralConnectionActive(_) => {
                ControllerCommandPhase::PeripheralConnectionActive
            }
            Self::PassiveScanFirst(_) | Self::PassiveScanRetry(_) => {
                ControllerCommandPhase::PassiveScanFirst
            }
            Self::PassiveScanResponse(_) => ControllerCommandPhase::PassiveScanResponse,
            Self::PassiveScanActive(_)
            | Self::PassiveScanActiveResponse(_)
            | Self::PassiveScanStopping(_)
            | Self::PassiveScanReports(_)
            | Self::PassiveScanComplete(_)
            | Self::PassiveScanCpuResponse(_)
            | Self::PassiveScanRecurring(_)
            | Self::PassiveScanRecurringRetry(_) => ControllerCommandPhase::PassiveScanActive,
            Self::Active(_) => ControllerCommandPhase::Active,
            Self::ResetStopping(_) => ControllerCommandPhase::ResetStopping,
            Self::ResetRestore(_) => ControllerCommandPhase::ResetRestore,
            Self::ResetCompletion(_) => ControllerCommandPhase::ResetCompletion,
            Self::ResetResponse(_) => ControllerCommandPhase::ResetResponse,
            Self::UnownedFinishedList(_) => ControllerCommandPhase::UnownedFinishedList,
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
pub struct ControllerCommandTask<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) advertising_rejected_packets: Option<u32>,
    pub(super) advertising_last_receive_rejection: Option<(
        u8,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableConnectionRequestRejection,
    )>,
    pub(super) advertising_completion: Option<
        oer_esp32s31_bluetooth::memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    >,
    pub(super) owner: ControllerOwnerSlot<ControllerCommandState<'runtime, S, CAPACITY>>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize> ControllerCommandTask<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Start the sole actor from the final runtime's affine idle command task.
    pub const fn new(idle: ControllerIdleCommandTask<'runtime, S, CAPACITY>) -> Self {
        Self {
            owner: ControllerOwnerSlot::new(ControllerCommandState::Idle(idle)),
            advertising_completion: None,
            advertising_rejected_packets: Some(0),
            advertising_last_receive_rejection: None,
        }
    }

    /// Boot-lifetime count of received advertising PDUs rejected by LL admission.
    /// None means that the diagnostic count overflowed.
    pub const fn advertising_rejected_packets(&self) -> Option<u32> {
        self.advertising_rejected_packets
    }

    /// Last received PDU rejected by portable connection-request admission.
    pub const fn advertising_last_receive_rejection(
        &self,
    ) -> Option<(
        u8,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableConnectionRequestRejection,
    )> {
        self.advertising_last_receive_rejection
    }

    /// Latest reclaimed no-connection item; this does not prove RF success.
    pub const fn advertising_completion(
        &self,
    ) -> Option<
        oer_esp32s31_bluetooth::memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    > {
        self.advertising_completion
    }

    /// Current retained lifecycle phase.
    pub fn phase(&self) -> ControllerCommandPhase {
        self.owner.current().phase()
    }

    /// Whether a terminal boundary transferred the lower owner out of the actor.
    pub const fn is_empty(&self) -> bool {
        self.owner.is_empty()
    }

    pub(super) fn store_transition(
        &mut self,
        from: ControllerCommandPhase,
        stimulus: ControllerCommandStimulus,
        state: ControllerCommandState<'runtime, S, CAPACITY>,
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
        phase: ControllerCommandPhase,
        state: ControllerCommandState<'runtime, S, CAPACITY>,
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
        boundary: ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
    ) -> ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        assert_eq!(
            reduce_controller_command_transition(self.phase(), ControllerCommandStimulus::Retain,),
            ControllerCommandAction::Retain,
        );
        boundary
    }

    pub(super) fn store_unowned_finished_list<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        owner: UnownedFinishedListOwner<'runtime, S, CAPACITY>,
    ) -> ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let index = owner.index();
        self.store_transition(
            from,
            ControllerCommandStimulus::UnownedFinishedList,
            ControllerCommandState::UnownedFinishedList(owner),
        );
        ControllerCommandBoundary::UnownedFinishedList(index)
    }

    pub(super) fn retained_unowned_finished_list<'epoch, 'packet>(
        &self,
    ) -> ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let ControllerCommandState::UnownedFinishedList(owner) = self.owner.current() else {
            unreachable!("the selected unowned-list quarantine did not change")
        };
        assert_eq!(
            reduce_controller_command_transition(
                ControllerCommandPhase::UnownedFinishedList,
                ControllerCommandStimulus::UnownedFinishedList,
            ),
            ControllerCommandAction::Retain,
        );
        ControllerCommandBoundary::UnownedFinishedList(owner.index())
    }

    pub(super) fn terminal_boundary<'epoch, 'packet>(
        &self,
        from: ControllerCommandPhase,
        boundary: ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>,
    ) -> ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        assert!(self.owner.is_empty());
        assert_eq!(
            reduce_controller_command_transition(from, ControllerCommandStimulus::Terminal,),
            ControllerCommandAction::Terminal,
        );
        boundary
    }

    pub(super) fn store_first_failure<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        failure: DtmFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match failure {
            DtmFirstRunnerFailure::PreparationRejected(cleanup) => {
                let state = ControllerCommandState::FirstCleanup {
                    cleanup,
                    readiness: FirstCleanupReadiness::Ready,
                };
                if from == ControllerCommandPhase::FirstEvent {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::FirstEvent, state);
                }
                None
            }
            DtmFirstRunnerFailure::Retryable(retry) => {
                let state = ControllerCommandState::FirstRetry(retry);
                if from == ControllerCommandPhase::FirstEvent {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::FirstEvent, state);
                }
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::FirstEvent,
                )))
            }
            failure => Some(
                self.terminal_boundary(from, ControllerCommandBoundary::FirstEventFailed(failure)),
            ),
        }
    }

    pub(super) fn store_first_drive<'epoch, 'packet>(
        &mut self,
        drive: DtmFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match drive {
            DtmFirstDrive::Wait(wait) => {
                self.store_retained_state(
                    ControllerCommandPhase::FirstEvent,
                    ControllerCommandState::FirstEvent(wait),
                );
                None
            }
            DtmFirstDrive::Active(session) => {
                self.store_transition(
                    ControllerCommandPhase::FirstEvent,
                    ControllerCommandStimulus::Active,
                    ControllerCommandState::Active(DtmSessionTask::new(session)),
                );
                None
            }
            DtmFirstDrive::Failed(failure) => {
                self.store_first_failure(ControllerCommandPhase::FirstEvent, failure)
            }
        }
    }

    pub(super) fn store_legacy_advertising_failure<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        failure: LegacyAdvertisingFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match failure.into_hardware_failure_response() {
            Ok(pending) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleResponse,
                    ControllerCommandState::IdleResponse {
                        pending,
                        completion: ControllerIdleCompletion::LegacyAdvertisingStartRejected,
                    },
                );
                None
            }
            Err(LegacyAdvertisingFirstRunnerFailure::Retryable(retry)) => {
                let state = ControllerCommandState::LegacyAdvertisingRetry(retry);
                if from == ControllerCommandPhase::LegacyAdvertisingFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::LegacyAdvertisingFirst,
                        state,
                    );
                }
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyAdvertisingFirst,
                )))
            }
            Err(_) => unreachable!("only a pre-RUN retry lacks recovered idle ownership"),
        }
    }

    pub(super) fn store_legacy_advertising_drive<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        drive: LegacyAdvertisingFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match drive {
            LegacyAdvertisingFirstDrive::Wait(wait) => {
                let state = ControllerCommandState::LegacyAdvertisingFirst(wait);
                if from == ControllerCommandPhase::LegacyAdvertisingFirst {
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
            LegacyAdvertisingFirstDrive::Running(running) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::LegacyAdvertisingResponse,
                    ControllerCommandState::LegacyAdvertisingResponse(
                        running.into_response_pending_session(),
                    ),
                );
                None
            }
            LegacyAdvertisingFirstDrive::Failed(failure) => {
                self.store_legacy_advertising_failure(from, failure)
            }
        }
    }

    pub(super) fn store_legacy_connectable_advertising_failure<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        failure: LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match failure {
            LegacyConnectableAdvertisingFirstRunnerFailure::Recovered(recovered) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleResponse,
                    ControllerCommandState::IdleResponse {
                        pending: recovered.into_hardware_failure_response(),
                        completion:
                            ControllerIdleCompletion::LegacyConnectableAdvertisingStartRejected,
                    },
                );
                None
            }
            LegacyConnectableAdvertisingFirstRunnerFailure::RetryablePrePublication(retry) => {
                let state = ControllerCommandState::LegacyConnectableAdvertisingRetry(retry);
                if from == ControllerCommandPhase::LegacyConnectableAdvertisingFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::LegacyConnectableAdvertisingFirst,
                        state,
                    );
                }
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyConnectableAdvertisingFirst,
                )))
            }
            LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(failure) => {
                Some(self.terminal_boundary(
                    from,
                    ControllerCommandBoundary::LegacyConnectableAdvertisingFailStop(failure),
                ))
            }
        }
    }

    pub(super) fn store_legacy_connectable_advertising_drive<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        drive: LegacyConnectableAdvertisingFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match drive {
            LegacyConnectableAdvertisingFirstDrive::Wait(wait) => {
                let state = ControllerCommandState::LegacyConnectableAdvertisingFirst(wait);
                if from == ControllerCommandPhase::LegacyConnectableAdvertisingFirst {
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
            LegacyConnectableAdvertisingFirstDrive::Running(running) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::LegacyConnectableAdvertisingResponse,
                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                        running.into_response_pending(),
                    ),
                );
                Some(ControllerCommandBoundary::LegacyConnectableAdvertisingActive)
            }
            LegacyConnectableAdvertisingFirstDrive::Failed(failure) => {
                self.store_legacy_connectable_advertising_failure(from, failure)
            }
        }
    }

    pub(super) fn store_peripheral_connection_first_drive<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        drive: LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match drive {
            ControlFlow::Break(failure) => Some(self.terminal_boundary(
                from,
                ControllerCommandBoundary::PeripheralConnectionFirstFailStop(failure),
            )),
            ControlFlow::Continue(LegacyConnectablePeripheralFirstDrive::WaitControllerTime(
                wait,
            )) => {
                let state = ControllerCommandState::PeripheralConnectionFirst(wait);
                if from == ControllerCommandPhase::PeripheralConnectionFirst {
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
            ControlFlow::Continue(LegacyConnectablePeripheralFirstDrive::Retry(retry)) => {
                let state = ControllerCommandState::PeripheralConnectionFirstRetry(retry);
                if from == ControllerCommandPhase::PeripheralConnectionFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(
                        from,
                        ControllerCommandStimulus::PeripheralConnectionFirst,
                        state,
                    );
                }
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::PeripheralConnectionFirst,
                )))
            }
            ControlFlow::Continue(LegacyConnectablePeripheralFirstDrive::Running(running)) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::PeripheralConnectionActive,
                    ControllerCommandState::PeripheralConnectionActive(
                        PeripheralConnectionActiveSession::from_first(running),
                    ),
                );
                Some(ControllerCommandBoundary::PeripheralConnectionActive)
            }
        }
    }

    pub(super) fn store_peripheral_connection_stopping_step<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        step: LegacyConnectablePeripheralFirstStoppingStep<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match step {
            ControlFlow::Continue(drive) => {
                self.store_peripheral_connection_first_drive(from, drive)
            }
            ControlFlow::Break(LegacyConnectablePeripheralFirstHciResetOutcome::Ready(ready)) => {
                let (reset, _reset_evidence) = ready.into_parts();
                // The evidence has discharged its purpose once exact cancellation
                // produced the idle Reset barrier; no runtime owner is discarded.
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleReset,
                    ControllerCommandState::IdleReset(reset),
                );
                None
            }
            ControlFlow::Break(LegacyConnectablePeripheralFirstHciResetOutcome::FailStop(
                failure,
            )) => Some(self.terminal_boundary(
                from,
                ControllerCommandBoundary::PeripheralConnectionResetFailStop(failure),
            )),
        }
    }

    pub(super) fn store_passive_scan_failure<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        failure: PassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match failure.into_hardware_failure_response() {
            Ok(pending) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::IdleResponse,
                    ControllerCommandState::IdleResponse {
                        pending,
                        completion: ControllerIdleCompletion::PassiveScanStartRejected,
                    },
                );
                None
            }
            Err(failure) if failure.retry_cause().is_some() => {
                let state = ControllerCommandState::PassiveScanRetry(failure);
                if from == ControllerCommandPhase::PassiveScanFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::PassiveScanFirst, state);
                }
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::PassiveScanFirst,
                )))
            }
            Err(_) => unreachable!("only a retryable pre-RUN edge lacks idle ownership"),
        }
    }

    pub(super) fn store_passive_scan_drive<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        drive: PassiveScanFirstDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match drive {
            PassiveScanFirstDrive::Wait(wait) => {
                let state = ControllerCommandState::PassiveScanFirst(wait);
                if from == ControllerCommandPhase::PassiveScanFirst {
                    self.store_retained_state(from, state);
                } else {
                    self.store_transition(from, ControllerCommandStimulus::PassiveScanFirst, state);
                }
                None
            }
            PassiveScanFirstDrive::Running(running) => {
                self.store_transition(
                    from,
                    ControllerCommandStimulus::PassiveScanResponse,
                    ControllerCommandState::PassiveScanResponse(
                        running.into_response_pending_session(),
                    ),
                );
                None
            }
            PassiveScanFirstDrive::Failed(failure) => {
                self.store_passive_scan_failure(from, failure)
            }
        }
    }

    pub(super) fn store_passive_scan_recurring_drive<'epoch, 'packet>(
        &mut self,
        drive: PassiveScanRecurringDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let phase = ControllerCommandPhase::PassiveScanActive;
        match drive {
            PassiveScanRecurringDrive::Wait(runner) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::PassiveScanRecurring(runner),
                );
                None
            }
            PassiveScanRecurringDrive::Active(active) => {
                self.store_retained_state(phase, ControllerCommandState::PassiveScanActive(active));
                None
            }
            PassiveScanRecurringDrive::Failed(failure) if failure.retry_cause().is_some() => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::PassiveScanRecurringRetry(failure),
                );
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::PassiveScanRecurring,
                )))
            }
            PassiveScanRecurringDrive::Failed(failure) => Some(self.terminal_boundary(
                phase,
                ControllerCommandBoundary::PassiveScanRecurringFault(failure),
            )),
        }
    }

    pub(super) fn store_legacy_advertising_recurring_drive<'epoch, 'packet>(
        &mut self,
        drive: LegacyAdvertisingRecurringDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let phase = ControllerCommandPhase::LegacyAdvertisingActive;
        match drive {
            LegacyAdvertisingRecurringDrive::Wait(runner) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::LegacyAdvertisingRecurring(runner),
                );
                None
            }
            LegacyAdvertisingRecurringDrive::Active(active) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::LegacyAdvertisingActive(active),
                );
                None
            }
            LegacyAdvertisingRecurringDrive::ActiveResponsePending(pending) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::LegacyAdvertisingActiveResponse(pending),
                );
                None
            }
            LegacyAdvertisingRecurringDrive::Stopping(stopping) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::LegacyAdvertisingStopping(stopping),
                );
                None
            }
            LegacyAdvertisingRecurringDrive::Retryable(retry) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::LegacyAdvertisingRecurringRetry(retry),
                );
                Some(self.retain_boundary(ControllerCommandBoundary::Retryable(
                    ControllerRetry::LegacyAdvertisingRecurring,
                )))
            }
            LegacyAdvertisingRecurringDrive::Fault(fault) => Some(self.terminal_boundary(
                phase,
                ControllerCommandBoundary::LegacyAdvertisingRecurringFault(fault),
            )),
        }
    }

    pub(super) fn store_connectable_recurring_stop_drive<'epoch, 'packet>(
        &mut self,
        from: ControllerCommandPhase,
        drive: ConnectableRecurringStopDrive<'runtime, S, CAPACITY>,
    ) -> Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let phase = from;
        match drive {
            ConnectableRecurringStopDrive::Disable(pending) => {
                self.store_transition(
                    phase,
                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                    ControllerCommandState::LegacyAdvertisingDisableResponse {
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
                    ControllerCommandState::LegacyAdvertisingResetCompletion {
                        ready,
                        origin: LegacyAdvertisingStopOrigin::LegacyConnectableAdvertising,
                    },
                );
                None
            }
            ConnectableRecurringStopDrive::Wait(wait) => {
                self.store_retained_state(
                    phase,
                    ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(wait),
                );
                None
            }
            ConnectableRecurringStopDrive::FailStop(failure) => Some(self.terminal_boundary(
                phase,
                ControllerCommandBoundary::LegacyConnectableAdvertisingRecurringFailStop(
                    AdvertisingRecurringFailStop {
                        _owner: recurring::FailStopOwner::Stopping(failure),
                    },
                ),
            )),
        }
    }
}
