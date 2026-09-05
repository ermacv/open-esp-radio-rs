//! Command-phase reduction and the complete borrowed Controller run loop.

#[cfg(target_arch = "riscv32")]
use super::*;

/// Observable phase of the sole Controller command actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerCommandPhase {
    Idle,
    IdleReset,
    IdleResponse,
    FirstEvent,
    LegacyAdvertisingFirst,
    LegacyAdvertisingResponse,
    LegacyAdvertisingActive,
    LegacyAdvertisingStopCompletion,
    LegacyConnectableAdvertisingFirst,
    LegacyConnectableAdvertisingResponse,
    LegacyConnectableAdvertisingActive,
    PeripheralConnectionFirst,
    PeripheralConnectionActive,
    PassiveScanFirst,
    PassiveScanResponse,
    PassiveScanActive,
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
pub(super) enum ControllerCommandStimulus {
    Retain,
    IdleReset,
    IdleResponse,
    FirstEvent,
    LegacyAdvertisingFirst,
    LegacyAdvertisingResponse,
    LegacyAdvertisingActive,
    LegacyAdvertisingStopCompletion,
    LegacyConnectableAdvertisingFirst,
    LegacyConnectableAdvertisingResponse,
    LegacyConnectableAdvertisingActive,
    PeripheralConnectionFirst,
    PeripheralConnectionActive,
    PassiveScanFirst,
    PassiveScanResponse,
    PassiveScanActive,
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
pub(super) enum ControllerCommandAction {
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
pub(super) const fn reduce_controller_command_transition(
    phase: EmbassyBluetoothControllerCommandPhase,
    stimulus: ControllerCommandStimulus,
) -> ControllerCommandAction {
    use ControllerCommandAction::{Advance, Retain, Terminal};
    use ControllerCommandStimulus::{
        Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, LegacyAdvertisingActive,
        LegacyAdvertisingFirst, LegacyAdvertisingResponse, LegacyAdvertisingStopCompletion,
        LegacyConnectableAdvertisingActive, LegacyConnectableAdvertisingFirst,
        LegacyConnectableAdvertisingResponse, PassiveScanActive, PassiveScanFirst,
        PassiveScanResponse, PeripheralConnectionActive, PeripheralConnectionFirst,
        ResetCompletion, ResetResponse, ResetRestore, ResetStopping, UnownedFinishedList,
    };
    use EmbassyBluetoothControllerCommandPhase::{
        Active as ActivePhase, FirstEvent as FirstEventPhase, Idle, IdleReset as IdleResetPhase,
        IdleResponse as IdleResponsePhase, LegacyAdvertisingActive as LegacyAdvertisingActivePhase,
        LegacyAdvertisingFirst as LegacyAdvertisingFirstPhase,
        LegacyAdvertisingResponse as LegacyAdvertisingResponsePhase,
        LegacyAdvertisingStopCompletion as LegacyAdvertisingStopCompletionPhase,
        LegacyConnectableAdvertisingActive as LegacyConnectableAdvertisingActivePhase,
        LegacyConnectableAdvertisingFirst as LegacyConnectableAdvertisingFirstPhase,
        LegacyConnectableAdvertisingResponse as LegacyConnectableAdvertisingResponsePhase,
        PassiveScanActive as PassiveScanActivePhase, PassiveScanFirst as PassiveScanFirstPhase,
        PassiveScanResponse as PassiveScanResponsePhase,
        PeripheralConnectionActive as PeripheralConnectionActivePhase,
        PeripheralConnectionFirst as PeripheralConnectionFirstPhase,
        ResetCompletion as ResetCompletionPhase, ResetResponse as ResetResponsePhase,
        ResetRestore as ResetRestorePhase, ResetStopping as ResetStoppingPhase,
        UnownedFinishedList as UnownedFinishedListPhase,
    };

    match (phase, stimulus) {
        (_, ControllerCommandStimulus::Retain) => Retain,
        (Idle, IdleReset) => Advance(IdleResetPhase),
        (Idle, IdleResponse) => Advance(IdleResponsePhase),
        (Idle, FirstEvent) => Advance(FirstEventPhase),
        (Idle, LegacyAdvertisingFirst) => Advance(LegacyAdvertisingFirstPhase),
        (Idle, LegacyConnectableAdvertisingFirst) => {
            Advance(LegacyConnectableAdvertisingFirstPhase)
        }
        (Idle, PassiveScanFirst) => Advance(PassiveScanFirstPhase),
        (LegacyAdvertisingFirstPhase, LegacyAdvertisingResponse) => {
            Advance(LegacyAdvertisingResponsePhase)
        }
        (LegacyAdvertisingResponsePhase, LegacyAdvertisingActive) => {
            Advance(LegacyAdvertisingActivePhase)
        }
        (
            LegacyAdvertisingActivePhase | LegacyConnectableAdvertisingActivePhase,
            LegacyAdvertisingStopCompletion,
        ) => Advance(LegacyAdvertisingStopCompletionPhase),
        (LegacyConnectableAdvertisingFirstPhase, LegacyConnectableAdvertisingResponse) => {
            Advance(LegacyConnectableAdvertisingResponsePhase)
        }
        (LegacyConnectableAdvertisingResponsePhase, LegacyConnectableAdvertisingActive) => {
            Advance(LegacyConnectableAdvertisingActivePhase)
        }
        (
            LegacyConnectableAdvertisingResponsePhase | LegacyConnectableAdvertisingActivePhase,
            PeripheralConnectionFirst,
        ) => Advance(PeripheralConnectionFirstPhase),
        (
            LegacyConnectableAdvertisingResponsePhase
            | LegacyConnectableAdvertisingActivePhase
            | PeripheralConnectionFirstPhase,
            PeripheralConnectionActive,
        ) => Advance(PeripheralConnectionActivePhase),
        (PassiveScanFirstPhase, PassiveScanResponse) => Advance(PassiveScanResponsePhase),
        (PassiveScanResponsePhase, PassiveScanActive) => Advance(PassiveScanActivePhase),
        (PassiveScanFirstPhase, IdleResponse) => Advance(IdleResponsePhase),
        (PassiveScanActivePhase, IdleReset) => Advance(IdleResetPhase),
        (LegacyConnectableAdvertisingActivePhase, IdleReset) => Advance(IdleResetPhase),
        (PassiveScanActivePhase, IdleResponse) => Advance(IdleResponsePhase),
        (LegacyAdvertisingFirstPhase, IdleResponse) => Advance(IdleResponsePhase),
        (LegacyConnectableAdvertisingFirstPhase, IdleResponse) => Advance(IdleResponsePhase),
        (Idle | FirstEventPhase, Active) => Advance(ActivePhase),
        (IdleResetPhase, IdleResponse) => Advance(IdleResponsePhase),
        (FirstEventPhase, IdleResponse) => Advance(IdleResponsePhase),
        (ActivePhase, ResetStopping) => Advance(ResetStoppingPhase),
        (ActivePhase | ResetStoppingPhase, UnownedFinishedList) => {
            Advance(UnownedFinishedListPhase)
        }
        (
            LegacyAdvertisingActivePhase
            | LegacyConnectableAdvertisingActivePhase
            | LegacyConnectableAdvertisingResponsePhase
            | PassiveScanActivePhase,
            UnownedFinishedList,
        ) => Advance(UnownedFinishedListPhase),
        (UnownedFinishedListPhase, UnownedFinishedList) => Retain,
        (ResetStoppingPhase, ResetRestore) => Advance(ResetRestorePhase),
        (ResetStoppingPhase | ResetRestorePhase, ResetCompletion) => Advance(ResetCompletionPhase),
        (ResetCompletionPhase, ResetResponse) => Advance(ResetResponsePhase),
        (
            IdleResponsePhase
            | LegacyAdvertisingActivePhase
            | LegacyAdvertisingStopCompletionPhase
            | PassiveScanActivePhase
            | ActivePhase
            | ResetResponsePhase,
            IdleRestored,
        ) => Advance(Idle),
        (
            Idle
            | FirstEventPhase
            | LegacyAdvertisingFirstPhase
            | LegacyAdvertisingActivePhase
            | LegacyConnectableAdvertisingFirstPhase
            | LegacyConnectableAdvertisingResponsePhase
            | LegacyConnectableAdvertisingActivePhase
            | PeripheralConnectionFirstPhase
            | PeripheralConnectionActivePhase
            | PassiveScanFirstPhase
            | PassiveScanActivePhase
            | ActivePhase
            | ResetStoppingPhase,
            ControllerCommandStimulus::Terminal,
        ) => Terminal,
        _ => panic!("invalid Controller command actor transition"),
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothControllerCommandTask<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
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
        DelaySource: EmbassyBluetoothLegacyAdvertisingDelaySource,
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
        advertising_delay: &mut DelaySource,
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
                                BluetoothControllerIdleCommandRoute::Start(runner) => {
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
                                BluetoothControllerIdleCommandRoute::StartLegacyNonconnectableAdvertising(runner) => {
                                    if let Some(boundary) = self.store_legacy_advertising_drive(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        drive_legacy_advertising_first_ready(runner),
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::StartLegacyConnectableAdvertising(runner) => {
                                    if let Some(boundary) = self.store_legacy_connectable_advertising_drive(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        drive_legacy_connectable_advertising_first_ready(runner),
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::StartPassiveScanning(runner) => {
                                    if let Some(boundary) = self.store_passive_scan_drive(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        drive_passive_scan_first_ready(runner),
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::StartFailed(failure) => {
                                    if let Some(boundary) = self.store_first_failure(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::LegacyAdvertisingStartFailed(
                                    failure,
                                ) => {
                                    if let Some(boundary) = self.store_legacy_advertising_failure(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::LegacyConnectableAdvertisingStartFailed(
                                    failure,
                                ) => {
                                    if let Some(boundary) = self.store_legacy_connectable_advertising_failure(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::PassiveScanStartFailed(
                                    failure,
                                ) => {
                                    if let Some(boundary) = self.store_passive_scan_failure(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                BluetoothControllerIdleCommandRoute::ResponsePending(pending) => {
                                    self.store_transition(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        ControllerCommandStimulus::IdleResponse,
                                        EmbassyBluetoothControllerCommandState::IdleResponse {
                                            pending,
                                            completion: EmbassyBluetoothControllerIdleCompletion::ImmediateResponse,
                                        },
                                    );
                                }
                                BluetoothControllerIdleCommandRoute::ResetBarrier(barrier) => {
                                    self.store_transition(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        ControllerCommandStimulus::IdleReset,
                                        EmbassyBluetoothControllerCommandState::IdleReset(barrier),
                                    );
                                }
                                BluetoothControllerIdleCommandRoute::EndpointMismatch(mismatch) => {
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
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingRetry(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected advertising retry did not change")
                        };
                        if let Some(boundary) = self.store_legacy_advertising_drive(
                            EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst,
                            drive_legacy_advertising_first_ready(retry.retry()),
                        ) {
                            return boundary;
                        }
                        continue;
                    }
                    let EmbassyBluetoothControllerCommandState::LegacyAdvertisingFirst(wait) =
                        self.owner.current_mut()
                    else {
                        unreachable!("the selected advertising wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let EmbassyBluetoothControllerCommandState::LegacyAdvertisingFirst(wait) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited advertising wait did not change")
                    };
                    match wait.resume() {
                        EmbassyBluetoothLegacyAdvertisingFirstResume::Ready(drive) => {
                            if let Some(boundary) = self.store_legacy_advertising_drive(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst,
                                drive,
                            ) {
                                return boundary;
                            }
                        }
                        EmbassyBluetoothLegacyAdvertisingFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingFirst,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingFirst(
                                    wait,
                                ),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingResponse => {
                    let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResponse(pending) =
                        self.owner.current()
                    else {
                        unreachable!("the selected advertising response did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                        );
                    }
                    let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResponse(pending) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited advertising response did not change")
                    };
                    match pending.try_publish(controller) {
                        BluetoothLegacyAdvertisingResponsePublication::Published(active) => {
                            let index = active.hardware_list_index();
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingResponse,
                                ControllerCommandStimulus::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                    active,
                                ),
                            );
                            return EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingActive(
                                index,
                            );
                        }
                        BluetoothLegacyAdvertisingResponsePublication::Pending(pending) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingResponse,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingResponse(
                                    pending,
                                ),
                            );
                        }
                        BluetoothLegacyAdvertisingResponsePublication::EndpointMismatch(
                            pending,
                        ) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingResponse,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingResponse(
                                    pending,
                                ),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        BluetoothLegacyAdvertisingResponsePublication::Fault { pending, error } => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingResponse,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingResponse(
                                    pending,
                                ),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRetry(
                            _,
                        )
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRetry(
                            retry,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected connectable retry did not change")
                        };
                        if let Some(boundary) = self.store_legacy_connectable_advertising_drive(
                            EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                            drive_legacy_connectable_advertising_first_ready(retry.retry()),
                        ) {
                            return boundary;
                        }
                        continue;
                    }
                    let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingFirst(
                        wait,
                    ) = self.owner.current_mut()
                    else {
                        unreachable!("the selected connectable wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingFirst(
                        wait,
                    ) = self.owner.take()
                    else {
                        unreachable!("the awaited connectable wait did not change")
                    };
                    match wait.resume() {
                        EmbassyBluetoothLegacyConnectableAdvertisingFirstResume::Ready(drive) => {
                            if let Some(boundary) =
                                self.store_legacy_connectable_advertising_drive(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                                    drive,
                                )
                            {
                                return boundary;
                            }
                        }
                        EmbassyBluetoothLegacyConnectableAdvertisingFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                                EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingFirst(wait),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse => {
                    let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(
                        pending,
                    ) = self.owner.current()
                    else {
                        unreachable!("the selected connectable response did not change")
                    };
                    let radio_ready = match pending.radio_wait() {
                        Some(BluetoothLegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                            match select(
                                wakers.wait_scheduler_ready(wake),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        Some(BluetoothLegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                            match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        None => true,
                    };
                    let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(
                        pending,
                    ) = self.owner.take()
                    else {
                        unreachable!("the awaited connectable response did not change")
                    };
                    if radio_ready {
                        if let Some(boundary) =
                            drive_legacy_connectable_advertising_initial_pending_ready_with(
                                pending,
                                (&mut *self, &mut *advertising_delay),
                                EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations::new(
                                    |(actor, _): (&mut Self, &mut DelaySource), pending| {
                                        actor.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(pending),
                                );
                                        None
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource),
                                     pending,
                                     observed| {
                                        Some(actor.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                EmbassyBluetoothUnownedFinishedListOwner::LegacyConnectableAdvertisingInitialPending {
                                    _pending: pending,
                                    observed,
                                },
                            ))
                                    },
                                    |(actor, delay_source): (&mut Self, &mut DelaySource),
                                     completed| {
                                        let delay = delay_source.next_advertising_delay();
                                        recurring::begin_response(actor, EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse, completed, delay)
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource), accepted| {
                                        actor.store_peripheral_connection_first_drive(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                begin_legacy_connectable_peripheral_first_response_pending(accepted),
                            )
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource), fault| {
                                        Some(actor.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault),
                                    ))
                                    },
                                ),
                            )
                        {
                            return boundary;
                        }
                    } else {
                        match pending.try_publish(controller) {
                            BluetoothLegacyConnectableAdvertisingResponsePublication::Published(
                                active,
                            ) => {
                                self.store_transition(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandStimulus::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                );
                                return EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingActive;
                            }
                            BluetoothLegacyConnectableAdvertisingResponsePublication::Pending(
                                pending,
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(pending),
                            ),
                            BluetoothLegacyConnectableAdvertisingResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(pending),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyConnectableAdvertisingResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingResponse(pending),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                    }
                    continue;
                }
                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(wait) = self.owner.current() else {
                            unreachable!("the selected recurring cancellation did not change")
                        };
                        let ready = wait
                            .wait_for_recheck(recheck.wait_until_absolute_recheck())
                            .await;
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(wait) = self.owner.take() else {
                            unreachable!("the awaited recurring cancellation did not change")
                        };
                        if let Some(boundary) = self.store_connectable_recurring_stop_drive(
                            EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                            wait.resume_with(ready, &ConnectableRecurringStopDriveHandler),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    if matches!(self.owner.current(), EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(_)) {
                        let phase = EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive;
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait) = self.owner.current() else { unreachable!("selected recurrence wait is retained") };
                        let outcome = select(
                            wait.wait_for_recheck(recheck.wait_until_absolute_recheck()),
                            wait.wait_response_capacity(controller),
                        ).await;
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait) = self.owner.take() else { unreachable!("awaited recurrence wait is retained") };
                        match outcome {
                            Either::First(ready) => {
                                if let Some(boundary) = recurring::resume_response_wait(self, phase, wait, ready) { return boundary; }
                            }
                            Either::Second(Ok(())) => {
                                if let Some(boundary) = recurring::publish_response::<S, CAPACITY, ConnectableRecurringSequencePendingPhase, _, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>(self, wait.into_state(), controller) { return boundary; }
                            }
                            Either::Second(Err(_)) => {
                                self.store_retained_state(phase, EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait));
                                return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::EndpointMismatch);
                            }
                        }
                        continue;
                    }

                    if matches!(self.owner.current(), EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(_)) {
                        let phase = EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive;
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait) = self.owner.current() else { unreachable!("selected recurrence wait is retained") };
                        let outcome = select(
                            wait.wait_for_recheck(recheck.wait_until_absolute_recheck()),
                            wait.wait_command_available(controller),
                        ).await;
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait) = self.owner.take() else { unreachable!("awaited recurrence wait is retained") };
                        match outcome {
                            Either::First(ready) => {
                                if let Some(boundary) = recurring::resume_command_wait(self, phase, wait, ready) { return boundary; }
                            }
                            Either::Second(Ok(())) => {
                                let buffer = packet.take().expect("recurring command intake retains its scratch buffer");
                                let (buffer, boundary) = recurring::route_command::<S, CAPACITY, ConnectableRecurringSequencePendingPhase, _, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>(self, phase, wait.into_state(), controller, buffer).into_parts();
                                packet = buffer;
                                if let Some(boundary) = boundary { return boundary; }
                            }
                            Either::Second(Err(_)) => {
                                self.store_retained_state(phase, EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait));
                                return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::EndpointMismatch);
                            }
                        }
                        continue;
                    }

                    if recurring::is_retry(self.owner.current()) {
                        let buffer = packet
                            .take()
                            .expect("recurrence retains its scratch buffer");
                        let (buffer, boundary) =
                            recurring::retry_ready(self, controller, buffer).into_parts();
                        packet = buffer;
                        if let Some(boundary) = boundary {
                            return boundary;
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending) = self.owner.current() else {
                            unreachable!("the selected connectable response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(BluetoothLegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => match select(
                                wakers.wait_scheduler_ready(wake),
                                pending.wait_response_capacity(controller),
                            ).await {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                ),
                            },
                            Some(BluetoothLegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                pending.wait_response_capacity(controller),
                            ).await {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                ),
                            },
                            None => true,
                        };
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending) = self.owner.take() else {
                            unreachable!("the awaited connectable response did not change")
                        };
                        if radio_ready {
                            if let Some(boundary) = drive_legacy_connectable_advertising_pending_ready_with(
                                pending,
                                (&mut *self, &mut *advertising_delay),
                                EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations::new(
                                    |(actor, _): (&mut Self, &mut DelaySource), pending| {
                                    actor.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    None
                                },
                                |(actor, _): (&mut Self, &mut DelaySource), pending, observed| Some(actor.store_unowned_finished_list(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothUnownedFinishedListOwner::LegacyConnectableAdvertisingPending { _pending: pending, observed },
                                )),
                                |(actor, delay_source): (&mut Self, &mut DelaySource), completed| {
                                    let delay = delay_source.next_advertising_delay();
                                    recurring::begin_response(actor, EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive, completed, delay)
                                },
                                |(actor, _): (&mut Self, &mut DelaySource), accepted| actor.store_peripheral_connection_first_drive(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_response_pending(accepted),
                                ),
                                |(actor, _): (&mut Self, &mut DelaySource), fault| Some(actor.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault),
                                )),
                                ),
                            ) {
                                return boundary;
                            }
                        } else {
                            match pending.try_publish(controller) {
                                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Published(active) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                ),
                                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Pending(pending) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                ),
                                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(pending) => {
                                    self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::EndpointMismatch);
                                }
                                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Fault { pending, error } => {
                                    self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::HciFault(error));
                                }
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingStopping(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingStopping(stopping) = self.owner.current() else {
                            unreachable!("the selected connectable stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(BluetoothLegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => wakers.wait_scheduler_ready(wake).await,
                            Some(BluetoothLegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                                let _ = wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ).await;
                            }
                            None => {}
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingStopping(stopping) = self.owner.take() else {
                            unreachable!("the awaited connectable stopping owner did not change")
                        };
                        match drive_legacy_connectable_advertising_stopping_ready(stopping) {
                            BluetoothLegacyConnectableAdvertisingStoppingStep::Continue(stopping)
                            | BluetoothLegacyConnectableAdvertisingStoppingStep::Waiting(stopping) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingStopping(stopping),
                            ),
                            BluetoothLegacyConnectableAdvertisingStoppingStep::UnrelatedList { stopping, observed } => return self.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                EmbassyBluetoothUnownedFinishedListOwner::LegacyConnectableAdvertisingStopping { _stopping: stopping, observed },
                            ),
                            BluetoothLegacyConnectableAdvertisingStoppingStep::NoConnection(completed) => {
                                if let Some(boundary) = self.store_connectable_recurring_stop_drive(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    finish_legacy_connectable_advertising_no_connection_stopping_with(
                                        completed,
                                        &ConnectableRecurringStopDriveHandler,
                                    ),
                                ) {
                                    return boundary;
                                }
                            }
                            BluetoothLegacyConnectableAdvertisingStoppingStep::ConnectionAccepted(accepted) => {
                                if let Some(boundary) = self.store_peripheral_connection_stopping_step(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_stopping(accepted),
                                ) {
                                    return boundary;
                                }
                            }
                            BluetoothLegacyConnectableAdvertisingStoppingStep::FailStop(fault) => return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingStoppingFailStop(fault),
                            ),
                        }
                        continue;
                    }

                    let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(
                        active,
                    ) = self.owner.current()
                    else {
                        unreachable!("the selected connectable active owner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(BluetoothLegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                            match select(
                                wakers.wait_scheduler_ready(wake),
                                active.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        Some(BluetoothLegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                            match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                active.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        None => true,
                    };
                    let EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(
                        active,
                    ) = self.owner.take()
                    else {
                        unreachable!("the driven connectable active owner did not change")
                    };
                    if radio_ready {
                        match drive_legacy_connectable_advertising_active_ready(active) {
                            BluetoothLegacyConnectableAdvertisingHciActiveStep::Continue(active)
                            | BluetoothLegacyConnectableAdvertisingHciActiveStep::Waiting(active) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                            ),
                            BluetoothLegacyConnectableAdvertisingHciActiveStep::UnrelatedList { session: active, observed } => return self.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                EmbassyBluetoothUnownedFinishedListOwner::LegacyConnectableAdvertisingActive { _active: active, observed },
                            ),
                            BluetoothLegacyConnectableAdvertisingHciActiveStep::NoConnection(completed) => {
                                let delay = advertising_delay.next_advertising_delay();
                                if let Some(boundary) = recurring::begin_command(self, EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive, completed, delay) {
                                    return boundary;
                                }
                            }
                            BluetoothLegacyConnectableAdvertisingHciActiveStep::ConnectionAccepted(accepted) => {
                                if let Some(boundary) = self.store_peripheral_connection_first_drive(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_command_ready(accepted),
                                ) {
                                    return boundary;
                                }
                            }
                            BluetoothLegacyConnectableAdvertisingHciActiveStep::FailStop(fault) => return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingActiveFailStop(fault),
                            ),
                        }
                    } else {
                        let buffer = packet.take().expect(
                            "connectable advertising intake retains its sole scratch buffer",
                        );
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            BluetoothLegacyConnectableAdvertisingCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    BluetoothLegacyConnectableAdvertisingCommandRoute::ResponsePending(pending) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    ),
                                    BluetoothLegacyConnectableAdvertisingCommandRoute::Stopping(stopping) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingStopping(stopping),
                                    ),
                                    BluetoothLegacyConnectableAdvertisingCommandRoute::EndpointMismatch(mismatch) => return self.terminal_boundary(
                                        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        EmbassyBluetoothControllerCommandBoundary::LegacyConnectableAdvertisingCommandEndpointMismatch(mismatch),
                                    ),
                                }
                            }
                            BluetoothLegacyConnectableAdvertisingCommandIntake::Empty { active, buffer } => {
                                packet = Some(buffer);
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                );
                            }
                            BluetoothLegacyConnectableAdvertisingCommandIntake::EndpointMismatch { active, buffer: _ } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                );
                                return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::EndpointMismatch);
                            }
                            BluetoothLegacyConnectableAdvertisingCommandIntake::Channel { active, buffer: _, error } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                );
                                return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::HciFault(error));
                            }
                            BluetoothLegacyConnectableAdvertisingCommandIntake::NonCommand { active, frame } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                );
                                return self.retain_boundary(EmbassyBluetoothControllerCommandBoundary::NonCommand(frame));
                            }
                        }
                    }
                    continue;
                }
                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PeripheralConnectionFirstRetry(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PeripheralConnectionFirstRetry(
                            retry,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected peripheral retry did not change")
                        };
                        let retry = match retry.try_publish_response(controller) {
                            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::CommandReady(retry)
                            | EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Published(retry)
                            | EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Pending(retry) => retry,
                            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(retry) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                                    EmbassyBluetoothControllerCommandState::PeripheralConnectionFirstRetry(retry),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Fault { state: retry, error } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                                    EmbassyBluetoothControllerCommandState::PeripheralConnectionFirstRetry(retry),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        };
                        if let Some(boundary) = self.store_peripheral_connection_first_drive(
                            EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                            retry.retry(),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    let EmbassyBluetoothControllerCommandState::PeripheralConnectionFirst(wait) =
                        self.owner.current()
                    else {
                        unreachable!("the selected peripheral first wait did not change")
                    };
                    let controller_time_ready = match wait.hci_axis() {
                        BluetoothLegacyConnectablePeripheralFirstHciAxis::CommandReady => Some(
                            wait.wait_controller_time(recheck.wait_until_absolute_recheck())
                                .await,
                        ),
                        BluetoothLegacyConnectablePeripheralFirstHciAxis::ResponsePending => {
                            match select(
                                wait.wait_controller_time(recheck.wait_until_absolute_recheck()),
                                wait.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(ready) => Some(ready),
                                Either::Second(Ok(_)) => None,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                    };
                    let EmbassyBluetoothControllerCommandState::PeripheralConnectionFirst(wait) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited peripheral first wait did not change")
                    };
                    if let Some(ready) = controller_time_ready {
                        if let Some(boundary) = self.store_peripheral_connection_first_drive(
                            EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                            wait.resume_controller_time(ready),
                        ) {
                            return boundary;
                        }
                    } else {
                        match wait.try_publish_response(controller) {
                            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::CommandReady(wait)
                            | EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Published(wait)
                            | EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Pending(wait) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                                    EmbassyBluetoothControllerCommandState::PeripheralConnectionFirst(wait),
                                );
                            }
                            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(wait) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                                    EmbassyBluetoothControllerCommandState::PeripheralConnectionFirst(wait),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Fault { state: wait, error } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::PeripheralConnectionFirst,
                                    EmbassyBluetoothControllerCommandState::PeripheralConnectionFirst(wait),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                    }
                    continue;
                }
                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionActive => {
                    let EmbassyBluetoothControllerCommandState::PeripheralConnectionActive(running) =
                        self.owner.current()
                    else {
                        unreachable!("the selected peripheral active owner did not change")
                    };
                    if running.hci_axis()
                        == BluetoothLegacyConnectablePeripheralFirstHciAxis::CommandReady
                    {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::PeripheralConnectionActive,
                        );
                    }
                    if running.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                        );
                    }
                    let EmbassyBluetoothControllerCommandState::PeripheralConnectionActive(running) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited peripheral active owner did not change")
                    };
                    match running.try_publish_response(controller) {
                        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(running)
                        | BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Published(running)
                        | BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Pending(running) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionActive,
                                EmbassyBluetoothControllerCommandState::PeripheralConnectionActive(running),
                            );
                        }
                        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(running) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionActive,
                                EmbassyBluetoothControllerCommandState::PeripheralConnectionActive(running),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Fault { state: running, error } => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PeripheralConnectionActive,
                                EmbassyBluetoothControllerCommandState::PeripheralConnectionActive(running),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                    }
                    return self.retain_boundary(
                        EmbassyBluetoothControllerCommandBoundary::PeripheralConnectionActive,
                    );
                }
                EmbassyBluetoothControllerCommandPhase::PassiveScanFirst => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanRetry(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanRetry(failure) =
                            self.owner.take()
                        else {
                            unreachable!("the selected scanner retry did not change")
                        };
                        let runner = failure.retry().unwrap_or_else(|_| {
                            unreachable!("the retained scanner failure is retryable")
                        });
                        if let Some(boundary) = self.store_passive_scan_drive(
                            EmbassyBluetoothControllerCommandPhase::PassiveScanFirst,
                            drive_passive_scan_first_ready(runner),
                        ) {
                            return boundary;
                        }
                        continue;
                    }
                    let EmbassyBluetoothControllerCommandState::PassiveScanFirst(wait) =
                        self.owner.current_mut()
                    else {
                        unreachable!("the selected scanner wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let EmbassyBluetoothControllerCommandState::PassiveScanFirst(wait) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited scanner wait did not change")
                    };
                    match wait.resume() {
                        EmbassyBluetoothPassiveScanFirstResume::Ready(drive) => {
                            if let Some(boundary) = self.store_passive_scan_drive(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanFirst,
                                drive,
                            ) {
                                return boundary;
                            }
                        }
                        EmbassyBluetoothPassiveScanFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanFirst,
                                EmbassyBluetoothControllerCommandState::PassiveScanFirst(wait),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse => {
                    let EmbassyBluetoothControllerCommandState::PassiveScanResponse(pending) =
                        self.owner.current()
                    else {
                        unreachable!("the selected scanner response did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(
                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                        );
                    }
                    let EmbassyBluetoothControllerCommandState::PassiveScanResponse(pending) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited scanner response did not change")
                    };
                    match pending.try_publish(controller) {
                        BluetoothPassiveScanHciResponsePublication::Published(active) => {
                            self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse,
                                ControllerCommandStimulus::PassiveScanActive,
                                EmbassyBluetoothControllerCommandState::PassiveScanActive(active),
                            );
                            return EmbassyBluetoothControllerCommandBoundary::PassiveScanningActive;
                        }
                        BluetoothPassiveScanHciResponsePublication::Pending(pending) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse,
                                EmbassyBluetoothControllerCommandState::PassiveScanResponse(
                                    pending,
                                ),
                            );
                        }
                        BluetoothPassiveScanHciResponsePublication::EndpointMismatch(pending) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse,
                                EmbassyBluetoothControllerCommandState::PassiveScanResponse(
                                    pending,
                                ),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        BluetoothPassiveScanHciResponsePublication::Fault { pending, error } => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse,
                                EmbassyBluetoothControllerCommandState::PassiveScanResponse(
                                    pending,
                                ),
                            );
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::PassiveScanActive => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(pending) =
                            self.owner.current()
                        else {
                            unreachable!("the selected scanner response did not change")
                        };
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        let EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(pending) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited scanner response did not change")
                        };
                        match pending.try_publish(controller) {
                            BluetoothPassiveScanHciCpuResponsePublication::Published(completed) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanComplete(
                                        completed,
                                    ),
                                );
                            }
                            BluetoothPassiveScanHciCpuResponsePublication::Pending(pending) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(
                                        pending,
                                    ),
                                );
                            }
                            BluetoothPassiveScanHciCpuResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(
                                        pending,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothPassiveScanHciCpuResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(
                                        pending,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected active scanner response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(
                                open_esp_radio_esp32s31_bluetooth::BluetoothPassiveScanActiveWait::Scheduler(
                                    wake,
                                ),
                            ) => match select(
                                wakers.wait_scheduler_ready(wake),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            },
                            Some(
                                open_esp_radio_esp32s31_bluetooth::BluetoothPassiveScanActiveWait::PostUnlink(
                                    wake,
                                ),
                            ) => match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            },
                            None => true,
                        };
                        let EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited active scanner response did not change")
                        };
                        if radio_ready {
                            match pending.step_radio() {
                                BluetoothPassiveScanHciActivePendingRadioStep::Continue(pending)
                                | BluetoothPassiveScanHciActivePendingRadioStep::Waiting(
                                    pending,
                                ) => self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                                        pending,
                                    ),
                                ),
                                BluetoothPassiveScanHciActivePendingRadioStep::UnrelatedList {
                                    pending,
                                    observed,
                                } => {
                                    return self.store_unowned_finished_list(
                                        EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                        EmbassyBluetoothUnownedFinishedListOwner::PassiveScanPending {
                                            _pending: pending,
                                            observed,
                                        },
                                    );
                                }
                                BluetoothPassiveScanHciActivePendingRadioStep::CpuOwned(pending) => {
                                    self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(
                                            pending,
                                        ),
                                    )
                                }
                                BluetoothPassiveScanHciActivePendingRadioStep::Fault(fault) => {
                                    return self.terminal_boundary(
                                        EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                        EmbassyBluetoothControllerCommandBoundary::PassiveScanPendingFault(
                                            fault,
                                        ),
                                    );
                                }
                            }
                        } else {
                            match pending.try_publish(controller) {
                                BluetoothPassiveScanHciActiveResponsePublication::Published(
                                    active,
                                ) => self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActive(
                                        active,
                                    ),
                                ),
                                BluetoothPassiveScanHciActiveResponsePublication::Pending(
                                    pending,
                                ) => self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                                        pending,
                                    ),
                                ),
                                BluetoothPassiveScanHciActiveResponsePublication::EndpointMismatch(
                                    pending,
                                ) => {
                                    self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                                            pending,
                                        ),
                                    );
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                                BluetoothPassiveScanHciActiveResponsePublication::Fault {
                                    pending,
                                    error,
                                } => {
                                    self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                                            pending,
                                        ),
                                    );
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                    );
                                }
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanStopping(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanStopping(stopping) =
                            self.owner.current()
                        else {
                            unreachable!("the selected scanner stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(
                                open_esp_radio_esp32s31_bluetooth::BluetoothPassiveScanActiveWait::Scheduler(
                                    wake,
                                ),
                            ) => wakers.wait_scheduler_ready(wake).await,
                            Some(
                                open_esp_radio_esp32s31_bluetooth::BluetoothPassiveScanActiveWait::PostUnlink(
                                    wake,
                                ),
                            ) => {
                                let _ = wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            None => {}
                        }
                        let EmbassyBluetoothControllerCommandState::PassiveScanStopping(stopping) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited scanner stopping owner did not change")
                        };
                        match stopping.step() {
                            BluetoothPassiveScanHciStoppingStep::Continue(stopping)
                            | BluetoothPassiveScanHciStoppingStep::Waiting(stopping) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanStopping(
                                        stopping,
                                    ),
                                )
                            }
                            BluetoothPassiveScanHciStoppingStep::UnrelatedList {
                                stopping,
                                observed,
                            } => {
                                return self.store_unowned_finished_list(
                                    EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                    EmbassyBluetoothUnownedFinishedListOwner::PassiveScanStopping {
                                        _stopping: stopping,
                                        observed,
                                    },
                                );
                            }
                            BluetoothPassiveScanHciStoppingStep::Disable(pending) => {
                                self.store_transition(
                                    EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                    ControllerCommandStimulus::IdleResponse,
                                    EmbassyBluetoothControllerCommandState::IdleResponse {
                                        pending,
                                        completion:
                                            EmbassyBluetoothControllerIdleCompletion::PassiveScanDisable,
                                    },
                                )
                            }
                            BluetoothPassiveScanHciStoppingStep::Reset(barrier) => {
                                self.store_transition(
                                    EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                    ControllerCommandStimulus::IdleReset,
                                    EmbassyBluetoothControllerCommandState::IdleReset(barrier),
                                )
                            }
                            BluetoothPassiveScanHciStoppingStep::Fault(fault) => {
                                return self.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                    EmbassyBluetoothControllerCommandBoundary::PassiveScanStoppingFault(
                                        fault,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanComplete(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanComplete(completed) =
                            self.owner.take()
                        else {
                            unreachable!("the selected complete scanner did not change")
                        };
                        let buffer = packet
                            .take()
                            .expect("scanner command intake retains its sole scratch buffer");
                        match completed.try_route_controller_command_with_buffer(controller, buffer)
                        {
                            BluetoothPassiveScanHciCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    BluetoothPassiveScanHciCommandRoute::ResponsePending(
                                        pending,
                                    ) => self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanCpuResponse(
                                            pending,
                                        ),
                                    ),
                                    BluetoothPassiveScanHciCommandRoute::Disable(pending) => {
                                        self.store_transition(
                                            EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                            ControllerCommandStimulus::IdleResponse,
                                            EmbassyBluetoothControllerCommandState::IdleResponse {
                                                pending,
                                                completion:
                                                    EmbassyBluetoothControllerIdleCompletion::PassiveScanDisable,
                                            },
                                        );
                                    }
                                    BluetoothPassiveScanHciCommandRoute::Reset(barrier) => {
                                        self.store_transition(
                                            EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                            ControllerCommandStimulus::IdleReset,
                                            EmbassyBluetoothControllerCommandState::IdleReset(
                                                barrier,
                                            ),
                                        );
                                    }
                                    BluetoothPassiveScanHciCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                            EmbassyBluetoothControllerCommandBoundary::PassiveScanCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                            }
                            BluetoothPassiveScanHciCommandIntake::Empty { completed, buffer } => {
                                packet = Some(buffer);
                                match completed.begin_recurring() {
                                    Ok(runner) => {
                                        if let Some(boundary) = self
                                            .store_passive_scan_recurring_drive(
                                                drive_passive_scan_recurring_ready(runner),
                                            )
                                        {
                                            return boundary;
                                        }
                                    }
                                    Err(failure) => {
                                        if let Some(boundary) = self
                                            .store_passive_scan_recurring_drive(
                                                EmbassyBluetoothPassiveScanRecurringDrive::Failed(
                                                    failure,
                                                ),
                                            )
                                        {
                                            return boundary;
                                        }
                                    }
                                }
                            }
                            BluetoothPassiveScanHciCommandIntake::EndpointMismatch {
                                completed,
                                buffer: _,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanComplete(
                                        completed,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothPassiveScanHciCommandIntake::Channel {
                                completed,
                                buffer: _,
                                error,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanComplete(
                                        completed,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                            BluetoothPassiveScanHciCommandIntake::NonCommand {
                                completed,
                                frame,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanComplete(
                                        completed,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanRecurringRetry(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanRecurringRetry(
                            failure,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected scanner retry did not change")
                        };
                        match failure.retry() {
                            Ok(runner) => {
                                if let Some(boundary) = self.store_passive_scan_recurring_drive(
                                    drive_passive_scan_recurring_ready(runner),
                                ) {
                                    return boundary;
                                }
                            }
                            Err(failure) => {
                                return self.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                    EmbassyBluetoothControllerCommandBoundary::PassiveScanRecurringFault(
                                        failure,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanRecurring(_)
                    ) {
                        recheck.wait_until_absolute_recheck().await;
                        let EmbassyBluetoothControllerCommandState::PassiveScanRecurring(runner) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited recurring scanner did not change")
                        };
                        if let Some(boundary) = self.store_passive_scan_recurring_drive(
                            drive_passive_scan_recurring_ready(runner),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::PassiveScanReports(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::PassiveScanReports(reports) =
                            self.owner.current()
                        else {
                            unreachable!("the selected scanner reports did not change")
                        };
                        if reports.has_pending_event() {
                            match select(
                                reports.wait_report_capacity(controller),
                                reports.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(Ok(())) => {}
                                Either::Second(Ok(())) => {
                                    let EmbassyBluetoothControllerCommandState::PassiveScanReports(
                                        reports,
                                    ) = self.owner.take()
                                    else {
                                        unreachable!(
                                            "the command-ready scanner reports did not change"
                                        )
                                    };
                                    self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanComplete(
                                            reports.discard_remaining(),
                                        ),
                                    );
                                    continue;
                                }
                                Either::First(Err(_)) | Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        let EmbassyBluetoothControllerCommandState::PassiveScanReports(reports) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited scanner reports did not change")
                        };
                        match reports.step(controller) {
                            BluetoothPassiveScanHciReportStep::Published(reports)
                            | BluetoothPassiveScanHciReportStep::Masked(reports) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanReports(
                                        reports,
                                    ),
                                );
                            }
                            BluetoothPassiveScanHciReportStep::Pending { reports, error } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanReports(
                                        reports,
                                    ),
                                );
                                if error != HciChannelError::Full {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                    );
                                }
                            }
                            BluetoothPassiveScanHciReportStep::IgnoredMalformed {
                                reports,
                                error,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanReports(
                                        reports,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::PassiveScanMalformedPdu(
                                        error,
                                    ),
                                );
                            }
                            BluetoothPassiveScanHciReportStep::EncodingFault { reports, error } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanReports(
                                        reports,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::PassiveScanReportEncodingFault(
                                        error,
                                    ),
                                );
                            }
                            BluetoothPassiveScanHciReportStep::EndpointMismatch(reports) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanReports(
                                        reports,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothPassiveScanHciReportStep::Complete(completed) => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanComplete(
                                        completed,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    let EmbassyBluetoothControllerCommandState::PassiveScanActive(active) =
                        self.owner.current()
                    else {
                        unreachable!("the selected active scanner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(
                            open_esp_radio_esp32s31_bluetooth::BluetoothPassiveScanActiveWait::Scheduler(
                                wake,
                            ),
                        ) => match select(
                            wakers.wait_scheduler_ready(wake),
                            active.wait_command_available(controller),
                        )
                        .await
                        {
                            Either::First(()) => true,
                            Either::Second(Ok(())) => false,
                            Either::Second(Err(_)) => {
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                        },
                        Some(
                            open_esp_radio_esp32s31_bluetooth::BluetoothPassiveScanActiveWait::PostUnlink(
                                wake,
                            ),
                        ) => match select(
                            wakers.wait_post_unlink_or_recheck(
                                wake,
                                recheck.wait_until_absolute_recheck(),
                            ),
                            active.wait_command_available(controller),
                        )
                        .await
                        {
                            Either::First(_) => true,
                            Either::Second(Ok(())) => false,
                            Either::Second(Err(_)) => {
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                        },
                        None => true,
                    };
                    let EmbassyBluetoothControllerCommandState::PassiveScanActive(active) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited active scanner did not change")
                    };
                    if !radio_ready {
                        let buffer = packet
                            .take()
                            .expect("active scanner intake retains its sole scratch buffer");
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            BluetoothPassiveScanHciActiveCommandIntake::Routed {
                                route,
                                buffer,
                            } => {
                                packet = Some(buffer);
                                match route {
                                    BluetoothPassiveScanHciActiveCommandRoute::ResponsePending(
                                        pending,
                                    ) => self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanActiveResponse(
                                            pending,
                                        ),
                                    ),
                                    BluetoothPassiveScanHciActiveCommandRoute::Stopping(
                                        stopping,
                                    ) => self.owner.store(
                                        EmbassyBluetoothControllerCommandState::PassiveScanStopping(
                                            stopping,
                                        ),
                                    ),
                                    BluetoothPassiveScanHciActiveCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                            EmbassyBluetoothControllerCommandBoundary::PassiveScanActiveCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                            }
                            BluetoothPassiveScanHciActiveCommandIntake::Empty {
                                active,
                                buffer,
                            } => {
                                packet = Some(buffer);
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActive(
                                        active,
                                    ),
                                );
                            }
                            BluetoothPassiveScanHciActiveCommandIntake::EndpointMismatch {
                                active,
                                buffer: _,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActive(
                                        active,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothPassiveScanHciActiveCommandIntake::Channel {
                                active,
                                buffer: _,
                                error,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActive(
                                        active,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                            BluetoothPassiveScanHciActiveCommandIntake::NonCommand {
                                active,
                                frame,
                            } => {
                                self.owner.store(
                                    EmbassyBluetoothControllerCommandState::PassiveScanActive(
                                        active,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                                );
                            }
                        }
                        continue;
                    }
                    match drive_passive_scan_active_ready(active) {
                        EmbassyBluetoothPassiveScanActiveDrive::Waiting(active) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::PassiveScanActive(active),
                            )
                        }
                        EmbassyBluetoothPassiveScanActiveDrive::Reports(reports) => {
                            self.owner.store(
                                EmbassyBluetoothControllerCommandState::PassiveScanReports(reports),
                            )
                        }
                        EmbassyBluetoothPassiveScanActiveDrive::UnrelatedList {
                            session,
                            observed,
                        } => {
                            return self.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                EmbassyBluetoothUnownedFinishedListOwner::PassiveScan {
                                    _session: session,
                                    observed,
                                },
                            );
                        }
                        EmbassyBluetoothPassiveScanActiveDrive::Fault(fault) => {
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
                                EmbassyBluetoothControllerCommandBoundary::PassiveScanFault(fault),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive => {
                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected advertising response did not change")
                        };
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited advertising response did not change")
                        };
                        match pending.try_publish(controller) {
                            BluetoothLegacyAdvertisingCpuOwnedResponsePublication::Published(
                                completed,
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                                    completed,
                                ),
                            ),
                            BluetoothLegacyAdvertisingCpuOwnedResponsePublication::Pending(
                                pending,
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                                    pending,
                                ),
                            ),
                            BluetoothLegacyAdvertisingCpuOwnedResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                                        pending,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingCpuOwnedResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                                        pending,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringStopRestore(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                            restore,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected recurring stop restore did not change")
                        };
                        if restore.controller_time_drain_required() {
                            recheck.wait_until_absolute_recheck().await;
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                            restore,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited recurring stop restore did not change")
                        };
                        match restore.step() {
                            BluetoothLegacyAdvertisingRecurringStopRestoreStep::WaitControllerTime(
                                restore,
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                                    restore,
                                ),
                            ),
                            BluetoothLegacyAdvertisingRecurringStopRestoreStep::DisableResponse(
                                pending,
                            ) => self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                                    pending,
                                    origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                },
                            ),
                            BluetoothLegacyAdvertisingRecurringStopRestoreStep::ResetCompletion(
                                ready,
                            ) => self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion {
                                    ready,
                                    origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                },
                            ),
                            BluetoothLegacyAdvertisingRecurringStopRestoreStep::Rejected(
                                restore,
                            ) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                                        restore,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                                        EmbassyBluetoothControllerRetry::LegacyAdvertisingRecurringStopRestore,
                                    ),
                                );
                            }
                            BluetoothLegacyAdvertisingRecurringStopRestoreStep::Fault(fault) => {
                                return self.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringStopFault(
                                        fault,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableRestore(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableRestore(
                            restore,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected advertising Disable restore did not change")
                        };
                        match restore.restore() {
                            BluetoothLegacyAdvertisingDisableRestoreStep::ResponsePending(
                                pending,
                            ) => self.store_transition(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                                    pending,
                                    origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                },
                            ),
                            BluetoothLegacyAdvertisingDisableRestoreStep::Rejected(restore) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableRestore(
                                        restore,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                                        EmbassyBluetoothControllerRetry::LegacyAdvertisingDisableRestore,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetRestore(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetRestore(
                            restore,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected advertising Reset restore did not change")
                        };
                        match restore.restore() {
                            BluetoothLegacyAdvertisingResetRestoreStep::CompletionReady(ready) => {
                                self.store_transition(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion {
                                        ready,
                                        origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                    },
                                )
                            }
                            BluetoothLegacyAdvertisingResetRestoreStep::Rejected(restore) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetRestore(
                                        restore,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::Retryable(
                                        EmbassyBluetoothControllerRetry::LegacyAdvertisingResetRestore,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                            completed,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected completed advertising event did not change")
                        };
                        let buffer = packet
                            .take()
                            .expect("advertising command intake retains its sole scratch buffer");
                        let completed = match completed
                            .try_route_controller_command_with_buffer(controller, buffer)
                        {
                            BluetoothLegacyAdvertisingCpuOwnedCommandIntake::Routed {
                                route,
                                buffer,
                            } => {
                                packet = Some(buffer);
                                match route {
                                    BluetoothLegacyAdvertisingCpuOwnedCommandRoute::ResponsePending(
                                        pending,
                                    ) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                                            pending,
                                        ),
                                    ),
                                    BluetoothLegacyAdvertisingCpuOwnedCommandRoute::Disable(
                                        restore,
                                    ) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableRestore(
                                            restore,
                                        ),
                                    ),
                                    BluetoothLegacyAdvertisingCpuOwnedCommandRoute::ResetBarrier(
                                        barrier,
                                    ) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetRestore(
                                            barrier.begin_restore(),
                                        ),
                                    ),
                                    BluetoothLegacyAdvertisingCpuOwnedCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                            EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                                continue;
                            }
                            BluetoothLegacyAdvertisingCpuOwnedCommandIntake::Empty {
                                completed,
                                buffer,
                            } => {
                                packet = Some(buffer);
                                completed
                            }
                            BluetoothLegacyAdvertisingCpuOwnedCommandIntake::EndpointMismatch {
                                completed,
                                buffer: _,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                                        completed,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingCpuOwnedCommandIntake::Channel {
                                completed,
                                buffer: _,
                                error,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                                        completed,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                            BluetoothLegacyAdvertisingCpuOwnedCommandIntake::NonCommand {
                                completed,
                                frame,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                                        completed,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                                );
                            }
                        };
                        match completed.begin_recurring(advertising_delay.next_advertising_delay())
                        {
                            BluetoothLegacyAdvertisingRecurringStart::Runner(runner) => {
                                if let Some(boundary) = self
                                    .store_legacy_advertising_recurring_drive(
                                        drive_legacy_advertising_recurring_ready(runner),
                                    )
                                {
                                    return boundary;
                                }
                            }
                            BluetoothLegacyAdvertisingRecurringStart::SequenceExhausted(
                                completed,
                            ) => {
                                let index = completed.hardware_list_index();
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                                        completed,
                                    ),
                                );
                                return EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingSequenceExhausted(index);
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringRetry(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringRetry(
                            retry,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected recurring retry did not change")
                        };
                        if let Some(boundary) = self.store_legacy_advertising_recurring_drive(
                            drive_legacy_advertising_recurring_ready(retry.retry()),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                            runner,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected recurring wait did not change")
                        };
                        if runner.order_state()
                            == BluetoothLegacyAdvertisingRecurringOrderState::Stopping
                        {
                            let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                runner,
                            ) = self.owner.take()
                            else {
                                unreachable!("the stopping recurring owner did not change")
                            };
                            match runner.begin_stopping() {
                                BluetoothLegacyAdvertisingRecurringStopBegin::Restore(restore) => {
                                    self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                                            restore,
                                        ),
                                    );
                                }
                                BluetoothLegacyAdvertisingRecurringStopBegin::Published(runner) => {
                                    if let Some(boundary) = self
                                        .store_legacy_advertising_recurring_drive(
                                            drive_legacy_advertising_recurring_ready(runner),
                                        )
                                    {
                                        return boundary;
                                    }
                                }
                                BluetoothLegacyAdvertisingRecurringStopBegin::Fault(fault) => {
                                    return self.terminal_boundary(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringFault(
                                            fault,
                                        ),
                                    );
                                }
                            }
                            continue;
                        }
                        let order_progress = match select(
                            recheck.wait_until_absolute_recheck(),
                            runner.wait_order_progress(controller),
                        )
                        .await
                        {
                            Either::First(()) => None,
                            Either::Second(Ok(progress)) => Some(progress),
                            Either::Second(Err(_)) => {
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                        };
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                            runner,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited recurring owner did not change")
                        };
                        if let Some(progress) = order_progress {
                            match progress {
                                BluetoothLegacyAdvertisingRecurringOrderProgress::Command => {
                                    let buffer = packet.take().expect(
                                        "recurring command intake retains its sole scratch buffer",
                                    );
                                    match runner.try_route_controller_command_with_buffer(
                                        controller, buffer,
                                    ) {
                                        BluetoothLegacyAdvertisingRecurringCommandIntake::Routed {
                                            route,
                                            buffer,
                                        } => {
                                            packet = Some(buffer);
                                            match route {
                                                BluetoothLegacyAdvertisingRecurringCommandRoute::Continue(
                                                    runner,
                                                ) => self.store_retained_state(
                                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                        runner,
                                                    ),
                                                ),
                                                BluetoothLegacyAdvertisingRecurringCommandRoute::EndpointMismatch(
                                                    mismatch,
                                                ) => {
                                                    return self.terminal_boundary(
                                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                        EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingRecurringCommandEndpointMismatch(
                                                            mismatch,
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                        BluetoothLegacyAdvertisingRecurringCommandIntake::Empty {
                                            runner,
                                            buffer,
                                        } => {
                                            packet = Some(buffer);
                                            self.store_retained_state(
                                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                        }
                                        BluetoothLegacyAdvertisingRecurringCommandIntake::EndpointMismatch {
                                            runner,
                                            buffer: _,
                                        } => {
                                            self.store_retained_state(
                                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                            );
                                        }
                                        BluetoothLegacyAdvertisingRecurringCommandIntake::Channel {
                                            runner,
                                            buffer: _,
                                            error,
                                        } => {
                                            self.store_retained_state(
                                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                            );
                                        }
                                        BluetoothLegacyAdvertisingRecurringCommandIntake::NonCommand {
                                            runner,
                                            frame,
                                        } => {
                                            self.store_retained_state(
                                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                                            );
                                        }
                                    }
                                }
                                BluetoothLegacyAdvertisingRecurringOrderProgress::Response => {
                                    match runner.try_publish_response(controller) {
                                        BluetoothLegacyAdvertisingRecurringResponsePublication::Published(
                                            runner,
                                        )
                                        | BluetoothLegacyAdvertisingRecurringResponsePublication::Pending(
                                            runner,
                                        ) => self.store_retained_state(
                                            EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                            EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                runner,
                                            ),
                                        ),
                                        BluetoothLegacyAdvertisingRecurringResponsePublication::EndpointMismatch(
                                            runner,
                                        ) => {
                                            self.store_retained_state(
                                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                            );
                                        }
                                        BluetoothLegacyAdvertisingRecurringResponsePublication::Fault {
                                            runner,
                                            error,
                                        } => {
                                            self.store_retained_state(
                                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                            );
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                        if let Some(boundary) = self.store_legacy_advertising_recurring_drive(
                            drive_legacy_advertising_recurring_ready(runner),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected active advertising response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(BluetoothLegacyAdvertisingActiveWait::Scheduler(wake)) => {
                                match select(
                                    wakers.wait_scheduler_ready(wake),
                                    pending.wait_response_capacity(controller),
                                )
                                .await
                                {
                                    Either::First(()) => true,
                                    Either::Second(Ok(())) => false,
                                    Either::Second(Err(_)) => {
                                        return self.retain_boundary(
                                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                        );
                                    }
                                }
                            }
                            Some(BluetoothLegacyAdvertisingActiveWait::PostUnlink(wake)) => {
                                match select(
                                    wakers.wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    ),
                                    pending.wait_response_capacity(controller),
                                )
                                .await
                                {
                                    Either::First(_) => true,
                                    Either::Second(Ok(())) => false,
                                    Either::Second(Err(_)) => {
                                        return self.retain_boundary(
                                            EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                        );
                                    }
                                }
                            }
                            None => true,
                        };
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited active advertising response did not change")
                        };
                        if radio_ready {
                            match pending.step_radio() {
                                BluetoothLegacyAdvertisingActivePendingRadioStep::Continue(
                                    pending,
                                ) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                                        pending,
                                    ),
                                ),
                                BluetoothLegacyAdvertisingActivePendingRadioStep::Waiting(
                                    pending,
                                ) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                                        pending,
                                    ),
                                ),
                                BluetoothLegacyAdvertisingActivePendingRadioStep::UnrelatedList {
                                    pending,
                                    observed,
                                } => {
                                    return self.store_unowned_finished_list(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothUnownedFinishedListOwner::LegacyAdvertisingPending {
                                            _pending: pending,
                                            observed,
                                        },
                                    );
                                }
                                BluetoothLegacyAdvertisingActivePendingRadioStep::CpuOwned(
                                    pending,
                                ) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuResponse(
                                        pending,
                                    ),
                                ),
                                BluetoothLegacyAdvertisingActivePendingRadioStep::Fault(fault) => {
                                    return self.terminal_boundary(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingPendingFault(
                                            fault,
                                        ),
                                    );
                                }
                            }
                        } else {
                            match pending.try_publish(controller) {
                                BluetoothLegacyAdvertisingActiveResponsePublication::Published(
                                    active,
                                ) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                        active,
                                    ),
                                ),
                                BluetoothLegacyAdvertisingActiveResponsePublication::Pending(
                                    pending,
                                ) => self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                                        pending,
                                    ),
                                ),
                                BluetoothLegacyAdvertisingActiveResponsePublication::EndpointMismatch(
                                    pending,
                                ) => {
                                    self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    );
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                                BluetoothLegacyAdvertisingActiveResponsePublication::Fault {
                                    pending,
                                    error,
                                } => {
                                    self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    );
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                    );
                                }
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingStopping(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingStopping(
                            stopping,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected advertising stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(BluetoothLegacyAdvertisingActiveWait::Scheduler(wake)) => {
                                wakers.wait_scheduler_ready(wake).await;
                            }
                            Some(BluetoothLegacyAdvertisingActiveWait::PostUnlink(wake)) => {
                                let _ = wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            None => {}
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingStopping(
                            stopping,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited advertising stopping owner did not change")
                        };
                        match stopping.step() {
                            BluetoothLegacyAdvertisingStoppingStep::Continue(stopping)
                            | BluetoothLegacyAdvertisingStoppingStep::Waiting(stopping) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingStopping(
                                        stopping,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingStoppingStep::UnrelatedList {
                                stopping,
                                observed,
                            } => {
                                return self.store_unowned_finished_list(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothUnownedFinishedListOwner::LegacyAdvertisingStopping {
                                        _stopping: stopping,
                                        observed,
                                    },
                                );
                            }
                            BluetoothLegacyAdvertisingStoppingStep::DisableRestore(restore) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableRestore(
                                        restore,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingStoppingStep::ResetRestore(restore) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetRestore(
                                        restore,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingStoppingStep::Fault(fault) => {
                                return self.terminal_boundary(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingStoppingFault(
                                        fault,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    let EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(active) =
                        self.owner.current()
                    else {
                        unreachable!("the selected active advertising owner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(BluetoothLegacyAdvertisingActiveWait::Scheduler(wake)) => {
                            match select(
                                wakers.wait_scheduler_ready(wake),
                                active.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        Some(BluetoothLegacyAdvertisingActiveWait::PostUnlink(wake)) => {
                            match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                active.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        None => true,
                    };
                    let EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(active) =
                        self.owner.take()
                    else {
                        unreachable!("the driven active advertising owner did not change")
                    };
                    if !radio_ready {
                        let buffer = packet
                            .take()
                            .expect("active advertising intake retains its sole scratch buffer");
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            BluetoothLegacyAdvertisingActiveCommandIntake::Routed {
                                route,
                                buffer,
                            } => {
                                packet = Some(buffer);
                                match route {
                                    BluetoothLegacyAdvertisingActiveCommandRoute::ResponsePending(
                                        pending,
                                    ) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    ),
                                    BluetoothLegacyAdvertisingActiveCommandRoute::Stopping(
                                        stopping,
                                    ) => self.store_retained_state(
                                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingStopping(
                                            stopping,
                                        ),
                                    ),
                                    BluetoothLegacyAdvertisingActiveCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                            EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingActiveCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                            }
                            BluetoothLegacyAdvertisingActiveCommandIntake::Empty {
                                active,
                                buffer,
                            } => {
                                packet = Some(buffer);
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                        active,
                                    ),
                                );
                            }
                            BluetoothLegacyAdvertisingActiveCommandIntake::EndpointMismatch {
                                active,
                                buffer: _,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                        active,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingActiveCommandIntake::Channel {
                                active,
                                buffer: _,
                                error,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                        active,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                            BluetoothLegacyAdvertisingActiveCommandIntake::NonCommand {
                                active,
                                frame,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                        active,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::NonCommand(frame),
                                );
                            }
                        }
                        continue;
                    }
                    match drive_legacy_advertising_active_ready(active) {
                        EmbassyBluetoothLegacyAdvertisingActiveDrive::Waiting(active) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingActive(
                                    active,
                                ),
                            );
                        }
                        EmbassyBluetoothLegacyAdvertisingActiveDrive::CpuOwned(completed) => {
                            self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingCpuOwned(
                                    completed,
                                ),
                            );
                        }
                        EmbassyBluetoothLegacyAdvertisingActiveDrive::UnrelatedList {
                            session,
                            observed,
                        } => {
                            return self.store_unowned_finished_list(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothUnownedFinishedListOwner::LegacyAdvertising {
                                    _session: session,
                                    observed,
                                },
                            );
                        }
                        EmbassyBluetoothLegacyAdvertisingActiveDrive::Fault(fault) => {
                            return self.terminal_boundary(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandBoundary::LegacyAdvertisingFault(
                                    fault,
                                ),
                            );
                        }
                    }
                }
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingStopCompletion => {
                    let phase =
                        EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingStopCompletion;
                    if let EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                        pending,
                        ..
                    } = self.owner.current()
                    {
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                            pending,
                            origin,
                        } = self.owner.take()
                        else {
                            unreachable!("the awaited advertising Disable response did not change")
                        };
                        match pending.try_publish(controller) {
                            BluetoothLegacyAdvertisingDisableResponsePublication::Completed(idle) => {
                                self.store_transition(
                                    phase,
                                    ControllerCommandStimulus::IdleRestored,
                                    EmbassyBluetoothControllerCommandState::Idle(idle),
                                );
                                return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                    origin.disable_completion(),
                                );
                            }
                            BluetoothLegacyAdvertisingDisableResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin,
                                    },
                                );
                            }
                            BluetoothLegacyAdvertisingDisableResponsePublication::EndpointMismatch(pending) => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingDisableResponsePublication::Fault { pending, error } => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion { .. }
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion {
                            ready,
                            origin,
                        } = self.owner.take()
                        else {
                            unreachable!("the selected advertising Reset completion did not change")
                        };
                        match ready.complete(controller) {
                            BluetoothLegacyAdvertisingResetCompletion::ResponsePending(pending) => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                            }
                            BluetoothLegacyAdvertisingResetCompletion::EndpointMismatch(ready) => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion {
                                        ready,
                                        origin,
                                    },
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                        }
                        continue;
                    }

                    if let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse {
                        pending,
                        ..
                    } = self.owner.current()
                    {
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse {
                            pending,
                            origin,
                        } = self.owner.take()
                        else {
                            unreachable!("the awaited advertising Reset response did not change")
                        };
                        match pending.try_publish(controller) {
                            BluetoothLegacyAdvertisingResetResponsePublication::Completed(idle) => {
                                self.store_transition(
                                    phase,
                                    ControllerCommandStimulus::IdleRestored,
                                    EmbassyBluetoothControllerCommandState::Idle(idle),
                                );
                                return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                    EmbassyBluetoothControllerIdleCompletion::Reset,
                                );
                            }
                            BluetoothLegacyAdvertisingResetResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                            }
                            BluetoothLegacyAdvertisingResetResponsePublication::EndpointMismatch(pending) => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingResetResponsePublication::Fault { pending, error } => {
                                self.store_retained_state(
                                    phase,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                        continue;
                    }

                    unreachable!("the selected advertising stop completion did not change")
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
