//! Command-phase reduction and the complete borrowed Controller run loop.

#[cfg(target_arch = "riscv32")]
use super::*;

/// Observable phase of the sole Controller command actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerCommandPhase {
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
    Advance(ControllerCommandPhase),
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
    phase: ControllerCommandPhase,
    stimulus: ControllerCommandStimulus,
) -> ControllerCommandAction {
    use ControllerCommandAction::{Advance, Retain, Terminal};

    use ControllerCommandPhase::{
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

    use ControllerCommandStimulus::{
        Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, LegacyAdvertisingActive,
        LegacyAdvertisingFirst, LegacyAdvertisingResponse, LegacyAdvertisingStopCompletion,
        LegacyConnectableAdvertisingActive, LegacyConnectableAdvertisingFirst,
        LegacyConnectableAdvertisingResponse, PassiveScanActive, PassiveScanFirst,
        PassiveScanResponse, PeripheralConnectionActive, PeripheralConnectionFirst,
        ResetCompletion, ResetResponse, ResetRestore, ResetStopping, UnownedFinishedList,
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
impl<'runtime, S, const CAPACITY: usize> ControllerCommandTask<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Run until an externally meaningful observation or terminal lower owner.
    ///
    /// `packet` is the caller's sole reusable Host-to-Controller scratch buffer.
    /// A returned [`ControllerCommandBoundary::NonCommand`]
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
        Recheck: DtmControllerTimeRecheck,
        DelaySource: LegacyAdvertisingDelaySource,
    >(
        &mut self,
        wakers: &RuntimeNotifications<WakeMutex>,
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
    ) -> ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let mut packet = Some(packet);
        loop {
            if recheck.status() == DtmControllerTimeRecheckStatus::TimelineExhausted {
                return self.retain_boundary(ControllerCommandBoundary::ControllerTimeExhausted);
            }
            match self.phase() {
                ControllerCommandPhase::Idle => {
                    let ControllerCommandState::Idle(idle) = self.owner.current() else {
                        unreachable!("the selected idle phase did not change")
                    };
                    if idle.wait_command_available(controller).await.is_err() {
                        return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                    }

                    let ControllerCommandState::Idle(idle) = self.owner.take() else {
                        unreachable!("the awaited idle phase did not change")
                    };
                    let buffer = packet
                        .take()
                        .expect("idle command intake retains its sole scratch buffer");
                    match idle.try_route_idle_controller_command_with_buffer(controller, buffer) {
                        ControllerIdleCommandIntake::Routed { route, buffer } => {
                            packet = Some(buffer);
                            match route {
                                ControllerIdleCommandRoute::Start(runner) => {
                                    match drive_dtm_first_ready(runner) {
                                        DtmFirstDrive::Wait(wait) => {
                                            self.store_transition(
                                                ControllerCommandPhase::Idle,
                                                ControllerCommandStimulus::FirstEvent,
                                                ControllerCommandState::FirstEvent(
                                                    wait,
                                                ),
                                            );
                                        }
                                        DtmFirstDrive::Active(session) => {
                                            self.store_transition(
                                                ControllerCommandPhase::Idle,
                                                ControllerCommandStimulus::Active,
                                                ControllerCommandState::Active(
                                                    DtmSessionTask::new(session),
                                                ),
                                            );
                                        }
                                        DtmFirstDrive::Failed(failure) => {
                                            if let Some(boundary) = self.store_first_failure(
                                                ControllerCommandPhase::Idle,
                                                failure,
                                            ) {
                                                return boundary;
                                            }
                                        }
                                    }
                                }
                                ControllerIdleCommandRoute::StartLegacyNonconnectableAdvertising(runner) => {
                                    if let Some(boundary) = self.store_legacy_advertising_drive(
                                        ControllerCommandPhase::Idle,
                                        drive_legacy_advertising_first_ready(runner),
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::StartLegacyConnectableAdvertising(runner) => {
                                    if let Some(boundary) = self.store_legacy_connectable_advertising_drive(
                                        ControllerCommandPhase::Idle,
                                        drive_legacy_connectable_advertising_first_ready(runner),
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::StartPassiveScanning(runner) => {
                                    if let Some(boundary) = self.store_passive_scan_drive(
                                        ControllerCommandPhase::Idle,
                                        drive_passive_scan_first_ready(runner),
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::StartFailed(failure) => {
                                    if let Some(boundary) = self.store_first_failure(
                                        ControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::LegacyAdvertisingStartFailed(
                                    failure,
                                ) => {
                                    if let Some(boundary) = self.store_legacy_advertising_failure(
                                        ControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::LegacyConnectableAdvertisingStartFailed(
                                    failure,
                                ) => {
                                    if let Some(boundary) = self.store_legacy_connectable_advertising_failure(
                                        ControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::PassiveScanStartFailed(
                                    failure,
                                ) => {
                                    if let Some(boundary) = self.store_passive_scan_failure(
                                        ControllerCommandPhase::Idle,
                                        failure,
                                    ) {
                                        return boundary;
                                    }
                                }
                                ControllerIdleCommandRoute::ResponsePending(pending) => {
                                    self.store_transition(
                                        ControllerCommandPhase::Idle,
                                        ControllerCommandStimulus::IdleResponse,
                                        ControllerCommandState::IdleResponse {
                                            pending,
                                            completion: ControllerIdleCompletion::ImmediateResponse,
                                        },
                                    );
                                }
                                ControllerIdleCommandRoute::ResetBarrier(barrier) => {
                                    self.store_transition(
                                        ControllerCommandPhase::Idle,
                                        ControllerCommandStimulus::IdleReset,
                                        ControllerCommandState::IdleReset(barrier),
                                    );
                                }
                                ControllerIdleCommandRoute::EndpointMismatch(mismatch) => {
                                    return self.terminal_boundary(
                                        ControllerCommandPhase::Idle,
                                        ControllerCommandBoundary::IdleCommandEndpointMismatch(mismatch),
                                    );
                                }
                            }
                        }
                        ControllerIdleCommandIntake::Empty { task, buffer } => {
                            packet = Some(buffer);
                            self.owner.store(ControllerCommandState::Idle(task));
                        }
                        ControllerIdleCommandIntake::EndpointMismatch { task, buffer: _ } => {
                            self.owner.store(ControllerCommandState::Idle(task));
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        ControllerIdleCommandIntake::Channel {
                            task,
                            buffer: _,
                            error,
                        } => {
                            self.owner.store(ControllerCommandState::Idle(task));
                            return self
                                .retain_boundary(ControllerCommandBoundary::HciFault(error));
                        }
                        ControllerIdleCommandIntake::NonCommand { task, frame } => {
                            self.owner.store(ControllerCommandState::Idle(task));
                            return self
                                .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                        }
                    }
                }
                ControllerCommandPhase::IdleReset => {
                    let ControllerCommandState::IdleReset(barrier) = self.owner.take() else {
                        unreachable!("the selected idle-Reset phase did not change")
                    };
                    match barrier.complete(controller) {
                        ControllerIdleResetCompletion::ResponsePending(pending) => {
                            self.store_transition(
                                ControllerCommandPhase::IdleReset,
                                ControllerCommandStimulus::IdleResponse,
                                ControllerCommandState::IdleResponse {
                                    pending,
                                    completion: ControllerIdleCompletion::Reset,
                                },
                            );
                        }
                        ControllerIdleResetCompletion::EndpointMismatch(barrier) => {
                            self.owner.store(ControllerCommandState::IdleReset(barrier));
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                    }
                }
                ControllerCommandPhase::IdleResponse => {
                    let ControllerCommandState::IdleResponse { pending, .. } = self.owner.current()
                    else {
                        unreachable!("the selected idle-response phase did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                    }
                    let ControllerCommandState::IdleResponse {
                        pending,
                        completion,
                    } = self.owner.take()
                    else {
                        unreachable!("the awaited idle-response phase did not change")
                    };
                    match pending.try_publish(controller) {
                        ControllerIdleResponsePublication::Published(idle) => {
                            self.store_transition(
                                ControllerCommandPhase::IdleResponse,
                                ControllerCommandStimulus::IdleRestored,
                                ControllerCommandState::Idle(idle),
                            );
                            return ControllerCommandBoundary::IdleRestored(completion);
                        }
                        ControllerIdleResponsePublication::Pending(pending) => {
                            self.owner.store(ControllerCommandState::IdleResponse {
                                pending,
                                completion,
                            });
                        }
                        ControllerIdleResponsePublication::EndpointMismatch(pending) => {
                            self.owner.store(ControllerCommandState::IdleResponse {
                                pending,
                                completion,
                            });
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        ControllerIdleResponsePublication::Fault { pending, error } => {
                            self.owner.store(ControllerCommandState::IdleResponse {
                                pending,
                                completion,
                            });
                            return self
                                .retain_boundary(ControllerCommandBoundary::HciFault(error));
                        }
                    }
                }
                ControllerCommandPhase::LegacyAdvertisingFirst => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingRetry(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected advertising retry did not change")
                        };
                        if let Some(boundary) = self.store_legacy_advertising_drive(
                            ControllerCommandPhase::LegacyAdvertisingFirst,
                            drive_legacy_advertising_first_ready(retry.retry()),
                        ) {
                            return boundary;
                        }
                        continue;
                    }
                    let ControllerCommandState::LegacyAdvertisingFirst(wait) =
                        self.owner.current_mut()
                    else {
                        unreachable!("the selected advertising wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let ControllerCommandState::LegacyAdvertisingFirst(wait) = self.owner.take()
                    else {
                        unreachable!("the awaited advertising wait did not change")
                    };
                    match wait.resume() {
                        LegacyAdvertisingFirstResume::Ready(drive) => {
                            if let Some(boundary) = self.store_legacy_advertising_drive(
                                ControllerCommandPhase::LegacyAdvertisingFirst,
                                drive,
                            ) {
                                return boundary;
                            }
                        }
                        LegacyAdvertisingFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingFirst,
                                ControllerCommandState::LegacyAdvertisingFirst(wait),
                            );
                        }
                    }
                }
                ControllerCommandPhase::LegacyAdvertisingResponse => {
                    let ControllerCommandState::LegacyAdvertisingResponse(pending) =
                        self.owner.current()
                    else {
                        unreachable!("the selected advertising response did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                    }
                    let ControllerCommandState::LegacyAdvertisingResponse(pending) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited advertising response did not change")
                    };
                    match pending.try_publish(controller) {
                        LegacyAdvertisingResponsePublication::Published(active) => {
                            let index = active.hardware_list_index();
                            self.store_transition(
                                ControllerCommandPhase::LegacyAdvertisingResponse,
                                ControllerCommandStimulus::LegacyAdvertisingActive,
                                ControllerCommandState::LegacyAdvertisingActive(active),
                            );
                            return ControllerCommandBoundary::LegacyAdvertisingActive(index);
                        }
                        LegacyAdvertisingResponsePublication::Pending(pending) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingResponse,
                                ControllerCommandState::LegacyAdvertisingResponse(pending),
                            );
                        }
                        LegacyAdvertisingResponsePublication::EndpointMismatch(pending) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingResponse,
                                ControllerCommandState::LegacyAdvertisingResponse(pending),
                            );
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        LegacyAdvertisingResponsePublication::Fault { pending, error } => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingResponse,
                                ControllerCommandState::LegacyAdvertisingResponse(pending),
                            );
                            return self
                                .retain_boundary(ControllerCommandBoundary::HciFault(error));
                        }
                    }
                }
                ControllerCommandPhase::LegacyConnectableAdvertisingFirst => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRetry(_,)
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected connectable retry did not change")
                        };
                        if let Some(boundary) = self.store_legacy_connectable_advertising_drive(
                            ControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                            drive_legacy_connectable_advertising_first_ready(retry.retry()),
                        ) {
                            return boundary;
                        }
                        continue;
                    }
                    let ControllerCommandState::LegacyConnectableAdvertisingFirst(wait) =
                        self.owner.current_mut()
                    else {
                        unreachable!("the selected connectable wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let ControllerCommandState::LegacyConnectableAdvertisingFirst(wait) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited connectable wait did not change")
                    };
                    match wait.resume() {
                        LegacyConnectableAdvertisingFirstResume::Ready(drive) => {
                            if let Some(boundary) = self.store_legacy_connectable_advertising_drive(
                                ControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                                drive,
                            ) {
                                return boundary;
                            }
                        }
                        LegacyConnectableAdvertisingFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                                ControllerCommandState::LegacyConnectableAdvertisingFirst(wait),
                            );
                        }
                    }
                }
                ControllerCommandPhase::LegacyConnectableAdvertisingResponse => {
                    let ControllerCommandState::LegacyConnectableAdvertisingResponse(pending) =
                        self.owner.current()
                    else {
                        unreachable!("the selected connectable response did not change")
                    };
                    let radio_ready = match pending.radio_wait() {
                        Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        None => true,
                    };
                    let ControllerCommandState::LegacyConnectableAdvertisingResponse(pending) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited connectable response did not change")
                    };
                    if radio_ready {
                        if let Some(boundary) =
                            drive_legacy_connectable_advertising_initial_pending_ready_with(
                                pending,
                                (&mut *self, &mut *advertising_delay),
                                LegacyConnectableAdvertisingReadyContinuations::new(
                                    |(actor, _): (&mut Self, &mut DelaySource), pending| {
                                        actor.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(pending),
                                );
                                        None
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource),
                                     pending,
                                     observed| {
                                        Some(actor.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                UnownedFinishedListOwner::LegacyConnectableAdvertisingInitialPending {
                                    _pending: pending,
                                    observed,
                                },
                            ))
                                    },
                                    |(actor, delay_source): (&mut Self, &mut DelaySource),
                                     completed| {
                                        let delay = delay_source.next_advertising_delay();
                                        recurring::begin_response(actor, ControllerCommandPhase::LegacyConnectableAdvertisingResponse, completed, delay)
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource), accepted| {
                                        actor.store_peripheral_connection_first_drive(
                                ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                begin_legacy_connectable_peripheral_first_response_pending(accepted),
                            )
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource), fault| {
                                        Some(actor.terminal_boundary(
                                ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                ControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault),
                                    ))
                                    },
                                ),
                            )
                        {
                            return boundary;
                        }
                    } else {
                        match pending.try_publish(controller) {
                            LegacyConnectableAdvertisingResponsePublication::Published(active) => {
                                self.store_transition(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandStimulus::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                return ControllerCommandBoundary::LegacyConnectableAdvertisingActive;
                            }
                            LegacyConnectableAdvertisingResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                                        pending,
                                    ),
                                )
                            }
                            LegacyConnectableAdvertisingResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                                        pending,
                                    ),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyConnectableAdvertisingResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                                        pending,
                                    ),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                        }
                    }
                    continue;
                }
                ControllerCommandPhase::LegacyConnectableAdvertisingActive => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(
                            _
                        )
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(wait) = self.owner.current() else {
                            unreachable!("the selected recurring cancellation did not change")
                        };
                        let ready = wait
                            .wait_for_recheck(recheck.wait_until_absolute_recheck())
                            .await;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(wait) = self.owner.take() else {
                            unreachable!("the awaited recurring cancellation did not change")
                        };
                        if let Some(boundary) = self.store_connectable_recurring_stop_drive(
                            ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                            wait.resume_with(ready, &ConnectableRecurringStopDriveHandler),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(
                            _
                        )
                    ) {
                        let phase = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait) = self.owner.current() else { unreachable!("selected recurrence wait is retained") };
                        let outcome = select(
                            wait.wait_for_recheck(recheck.wait_until_absolute_recheck()),
                            wait.wait_response_capacity(controller),
                        )
                        .await;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait) = self.owner.take() else { unreachable!("awaited recurrence wait is retained") };
                        match outcome {
                            Either::First(ready) => {
                                if let Some(boundary) =
                                    recurring::resume_response_wait(self, phase, wait, ready)
                                {
                                    return boundary;
                                }
                            }
                            Either::Second(Ok(())) => {
                                if let Some(boundary) = recurring::publish_response::<
                                    S,
                                    CAPACITY,
                                    ConnectableRecurringSequencePendingPhase,
                                    _,
                                    HOST_TO_CONTROLLER_DEPTH,
                                    CONTROLLER_TO_HOST_DEPTH,
                                    PACKET_CAPACITY,
                                >(
                                    self, wait.into_state(), controller
                                ) {
                                    return boundary;
                                }
                            }
                            Either::Second(Err(_)) => {
                                self.store_retained_state(phase, ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(_)
                    ) {
                        let phase = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait) = self.owner.current() else { unreachable!("selected recurrence wait is retained") };
                        let outcome = select(
                            wait.wait_for_recheck(recheck.wait_until_absolute_recheck()),
                            wait.wait_command_available(controller),
                        )
                        .await;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait) = self.owner.take() else { unreachable!("awaited recurrence wait is retained") };
                        match outcome {
                            Either::First(ready) => {
                                if let Some(boundary) =
                                    recurring::resume_command_wait(self, phase, wait, ready)
                                {
                                    return boundary;
                                }
                            }
                            Either::Second(Ok(())) => {
                                let buffer = packet
                                    .take()
                                    .expect("recurring command intake retains its scratch buffer");
                                let (buffer, boundary) =
                                    recurring::route_command::<
                                        S,
                                        CAPACITY,
                                        ConnectableRecurringSequencePendingPhase,
                                        _,
                                        HOST_TO_CONTROLLER_DEPTH,
                                        CONTROLLER_TO_HOST_DEPTH,
                                        PACKET_CAPACITY,
                                    >(
                                        self, phase, wait.into_state(), controller, buffer
                                    )
                                    .into_parts();
                                packet = buffer;
                                if let Some(boundary) = boundary {
                                    return boundary;
                                }
                            }
                            Either::Second(Err(_)) => {
                                self.store_retained_state(phase, ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
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
                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(_)
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected connectable response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
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
                                            ControllerCommandBoundary::EndpointMismatch,
                                        );
                                    }
                                }
                            }
                            Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
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
                                            ControllerCommandBoundary::EndpointMismatch,
                                        );
                                    }
                                }
                            }
                            None => true,
                        };
                        let ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited connectable response did not change")
                        };
                        if radio_ready {
                            if let Some(boundary) =
                                drive_legacy_connectable_advertising_pending_ready_with(
                                    pending,
                                    (&mut *self, &mut *advertising_delay),
                                    LegacyConnectableAdvertisingReadyContinuations::new(
                                        |(actor, _): (&mut Self, &mut DelaySource), pending| {
                                            actor.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                            None
                                        },
                                        |(actor, _): (&mut Self, &mut DelaySource),
                                         pending,
                                         observed| {
                                            Some(actor.store_unowned_finished_list(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    UnownedFinishedListOwner::LegacyConnectableAdvertisingPending { _pending: pending, observed },
                                ))
                                        },
                                        |(actor, delay_source): (&mut Self, &mut DelaySource),
                                         completed| {
                                            let delay = delay_source.next_advertising_delay();
                                            recurring::begin_response(actor, ControllerCommandPhase::LegacyConnectableAdvertisingActive, completed, delay)
                                        },
                                        |(actor, _): (&mut Self, &mut DelaySource), accepted| {
                                            actor.store_peripheral_connection_first_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_response_pending(accepted),
                                )
                                        },
                                        |(actor, _): (&mut Self, &mut DelaySource), fault| {
                                            Some(actor.terminal_boundary(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault),
                                ))
                                        },
                                    ),
                                )
                            {
                                return boundary;
                            }
                        } else {
                            match pending.try_publish(controller) {
                                LegacyConnectableAdvertisingActiveResponsePublication::Published(active) => self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                ),
                                LegacyConnectableAdvertisingActiveResponsePublication::Pending(pending) => self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                ),
                                LegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(pending) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                                }
                                LegacyConnectableAdvertisingActiveResponsePublication::Fault { pending, error } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    return self.retain_boundary(ControllerCommandBoundary::HciFault(error));
                                }
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingStopping(_)
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping) =
                            self.owner.current()
                        else {
                            unreachable!("the selected connectable stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                                wakers.wait_scheduler_ready(wake).await
                            }
                            Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                                let _ = wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            None => {}
                        }
                        let ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited connectable stopping owner did not change")
                        };
                        match drive_legacy_connectable_advertising_stopping_ready(stopping) {
                            LegacyConnectableAdvertisingStoppingStep::Continue(stopping)
                            | LegacyConnectableAdvertisingStoppingStep::Waiting(stopping) => self.store_retained_state(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping),
                            ),
                            LegacyConnectableAdvertisingStoppingStep::UnrelatedList { stopping, observed } => return self.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                UnownedFinishedListOwner::LegacyConnectableAdvertisingStopping { _stopping: stopping, observed },
                            ),
                            LegacyConnectableAdvertisingStoppingStep::NoConnection(completed) => {
                                if let Some(boundary) = self.store_connectable_recurring_stop_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    finish_legacy_connectable_advertising_no_connection_stopping_with(
                                        completed,
                                        &ConnectableRecurringStopDriveHandler,
                                    ),
                                ) {
                                    return boundary;
                                }
                            }
                            LegacyConnectableAdvertisingStoppingStep::ConnectionAccepted(accepted) => {
                                if let Some(boundary) = self.store_peripheral_connection_stopping_step(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_stopping(accepted),
                                ) {
                                    return boundary;
                                }
                            }
                            LegacyConnectableAdvertisingStoppingStep::FailStop(fault) => return self.terminal_boundary(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandBoundary::LegacyConnectableAdvertisingStoppingFailStop(fault),
                            ),
                        }
                        continue;
                    }

                    let ControllerCommandState::LegacyConnectableAdvertisingActive(active) =
                        self.owner.current()
                    else {
                        unreachable!("the selected connectable active owner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        None => true,
                    };
                    let ControllerCommandState::LegacyConnectableAdvertisingActive(active) =
                        self.owner.take()
                    else {
                        unreachable!("the driven connectable active owner did not change")
                    };
                    if radio_ready {
                        match drive_legacy_connectable_advertising_active_ready(active) {
                            LegacyConnectableAdvertisingHciActiveStep::Continue(active)
                            | LegacyConnectableAdvertisingHciActiveStep::Waiting(active) => self.store_retained_state(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandState::LegacyConnectableAdvertisingActive(active),
                            ),
                            LegacyConnectableAdvertisingHciActiveStep::UnrelatedList { session: active, observed } => return self.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                UnownedFinishedListOwner::LegacyConnectableAdvertisingActive { _active: active, observed },
                            ),
                            LegacyConnectableAdvertisingHciActiveStep::NoConnection(completed) => {
                                let delay = advertising_delay.next_advertising_delay();
                                if let Some(boundary) = recurring::begin_command(self, ControllerCommandPhase::LegacyConnectableAdvertisingActive, completed, delay) {
                                    return boundary;
                                }
                            }
                            LegacyConnectableAdvertisingHciActiveStep::ConnectionAccepted(accepted) => {
                                if let Some(boundary) = self.store_peripheral_connection_first_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_command_ready(accepted),
                                ) {
                                    return boundary;
                                }
                            }
                            LegacyConnectableAdvertisingHciActiveStep::FailStop(fault) => return self.terminal_boundary(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandBoundary::LegacyConnectableAdvertisingActiveFailStop(fault),
                            ),
                        }
                    } else {
                        let buffer = packet.take().expect(
                            "connectable advertising intake retains its sole scratch buffer",
                        );
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            LegacyConnectableAdvertisingCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    LegacyConnectableAdvertisingCommandRoute::ResponsePending(pending) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    ),
                                    LegacyConnectableAdvertisingCommandRoute::Stopping(stopping) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping),
                                    ),
                                    LegacyConnectableAdvertisingCommandRoute::EndpointMismatch(mismatch) => return self.terminal_boundary(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandBoundary::LegacyConnectableAdvertisingCommandEndpointMismatch(mismatch),
                                    ),
                                }
                            }
                            LegacyConnectableAdvertisingCommandIntake::Empty { active, buffer } => {
                                packet = Some(buffer);
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                            }
                            LegacyConnectableAdvertisingCommandIntake::EndpointMismatch {
                                active,
                                buffer: _,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyConnectableAdvertisingCommandIntake::Channel {
                                active,
                                buffer: _,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                            LegacyConnectableAdvertisingCommandIntake::NonCommand {
                                active,
                                frame,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                            }
                        }
                    }
                    continue;
                }
                ControllerCommandPhase::PeripheralConnectionFirst => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PeripheralConnectionFirstRetry(_)
                    ) {
                        let ControllerCommandState::PeripheralConnectionFirstRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected peripheral retry did not change")
                        };
                        let retry = match retry.try_publish_response(controller) {
                            LegacyConnectablePeripheralFirstResponsePublication::CommandReady(retry)
                            | LegacyConnectablePeripheralFirstResponsePublication::Published(retry)
                            | LegacyConnectablePeripheralFirstResponsePublication::Pending(retry) => retry,
                            LegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(retry) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionFirst,
                                    ControllerCommandState::PeripheralConnectionFirstRetry(retry),
                                );
                                return self.retain_boundary(
                                    ControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            LegacyConnectablePeripheralFirstResponsePublication::Fault { state: retry, error } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionFirst,
                                    ControllerCommandState::PeripheralConnectionFirstRetry(retry),
                                );
                                return self.retain_boundary(
                                    ControllerCommandBoundary::HciFault(error),
                                );
                            }
                        };
                        if let Some(boundary) = self.store_peripheral_connection_first_drive(
                            ControllerCommandPhase::PeripheralConnectionFirst,
                            retry.retry(),
                        ) {
                            return boundary;
                        }
                        continue;
                    }

                    let ControllerCommandState::PeripheralConnectionFirst(wait) =
                        self.owner.current()
                    else {
                        unreachable!("the selected peripheral first wait did not change")
                    };
                    let controller_time_ready = match wait.hci_axis() {
                        LegacyConnectablePeripheralFirstHciAxis::CommandReady => Some(
                            wait.wait_controller_time(recheck.wait_until_absolute_recheck())
                                .await,
                        ),
                        LegacyConnectablePeripheralFirstHciAxis::ResponsePending => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                    };
                    let ControllerCommandState::PeripheralConnectionFirst(wait) = self.owner.take()
                    else {
                        unreachable!("the awaited peripheral first wait did not change")
                    };
                    if let Some(ready) = controller_time_ready {
                        if let Some(boundary) = self.store_peripheral_connection_first_drive(
                            ControllerCommandPhase::PeripheralConnectionFirst,
                            wait.resume_controller_time(ready),
                        ) {
                            return boundary;
                        }
                    } else {
                        match wait.try_publish_response(controller) {
                            LegacyConnectablePeripheralFirstResponsePublication::CommandReady(wait)
                            | LegacyConnectablePeripheralFirstResponsePublication::Published(wait)
                            | LegacyConnectablePeripheralFirstResponsePublication::Pending(wait) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionFirst,
                                    ControllerCommandState::PeripheralConnectionFirst(wait),
                                );
                            }
                            LegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(wait) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionFirst,
                                    ControllerCommandState::PeripheralConnectionFirst(wait),
                                );
                                return self.retain_boundary(
                                    ControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            LegacyConnectablePeripheralFirstResponsePublication::Fault { state: wait, error } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionFirst,
                                    ControllerCommandState::PeripheralConnectionFirst(wait),
                                );
                                return self.retain_boundary(
                                    ControllerCommandBoundary::HciFault(error),
                                );
                            }
                        }
                    }
                    continue;
                }
                ControllerCommandPhase::PeripheralConnectionActive => {
                    let ControllerCommandState::PeripheralConnectionActive(running) =
                        self.owner.current()
                    else {
                        unreachable!("the selected peripheral active owner did not change")
                    };
                    if running.hci_axis() == LegacyConnectablePeripheralFirstHciAxis::CommandReady {
                        return self.retain_boundary(
                            ControllerCommandBoundary::PeripheralConnectionActive,
                        );
                    }
                    if running.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                    }
                    let ControllerCommandState::PeripheralConnectionActive(running) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited peripheral active owner did not change")
                    };
                    match running.try_publish_response(controller) {
                        LegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(running)
                        | LegacyConnectablePeripheralFirstHciResponsePublication::Published(running)
                        | LegacyConnectablePeripheralFirstHciResponsePublication::Pending(running) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionActive,
                                ControllerCommandState::PeripheralConnectionActive(running),
                            );
                        }
                        LegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(running) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionActive,
                                ControllerCommandState::PeripheralConnectionActive(running),
                            );
                            return self.retain_boundary(
                                ControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        LegacyConnectablePeripheralFirstHciResponsePublication::Fault { state: running, error } => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionActive,
                                ControllerCommandState::PeripheralConnectionActive(running),
                            );
                            return self.retain_boundary(
                                ControllerCommandBoundary::HciFault(error),
                            );
                        }
                    }
                    return self
                        .retain_boundary(ControllerCommandBoundary::PeripheralConnectionActive);
                }
                ControllerCommandPhase::PassiveScanFirst => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanRetry(_)
                    ) {
                        let ControllerCommandState::PassiveScanRetry(failure) = self.owner.take()
                        else {
                            unreachable!("the selected scanner retry did not change")
                        };
                        let runner = failure.retry().unwrap_or_else(|_| {
                            unreachable!("the retained scanner failure is retryable")
                        });
                        if let Some(boundary) = self.store_passive_scan_drive(
                            ControllerCommandPhase::PassiveScanFirst,
                            drive_passive_scan_first_ready(runner),
                        ) {
                            return boundary;
                        }
                        continue;
                    }
                    let ControllerCommandState::PassiveScanFirst(wait) = self.owner.current_mut()
                    else {
                        unreachable!("the selected scanner wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let ControllerCommandState::PassiveScanFirst(wait) = self.owner.take() else {
                        unreachable!("the awaited scanner wait did not change")
                    };
                    match wait.resume() {
                        PassiveScanFirstResume::Ready(drive) => {
                            if let Some(boundary) = self.store_passive_scan_drive(
                                ControllerCommandPhase::PassiveScanFirst,
                                drive,
                            ) {
                                return boundary;
                            }
                        }
                        PassiveScanFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PassiveScanFirst,
                                ControllerCommandState::PassiveScanFirst(wait),
                            );
                        }
                    }
                }
                ControllerCommandPhase::PassiveScanResponse => {
                    let ControllerCommandState::PassiveScanResponse(pending) = self.owner.current()
                    else {
                        unreachable!("the selected scanner response did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                    }
                    let ControllerCommandState::PassiveScanResponse(pending) = self.owner.take()
                    else {
                        unreachable!("the awaited scanner response did not change")
                    };
                    match pending.try_publish(controller) {
                        PassiveScanHciResponsePublication::Published(active) => {
                            self.store_transition(
                                ControllerCommandPhase::PassiveScanResponse,
                                ControllerCommandStimulus::PassiveScanActive,
                                ControllerCommandState::PassiveScanActive(active),
                            );
                            return ControllerCommandBoundary::PassiveScanningActive;
                        }
                        PassiveScanHciResponsePublication::Pending(pending) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PassiveScanResponse,
                                ControllerCommandState::PassiveScanResponse(pending),
                            );
                        }
                        PassiveScanHciResponsePublication::EndpointMismatch(pending) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PassiveScanResponse,
                                ControllerCommandState::PassiveScanResponse(pending),
                            );
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        PassiveScanHciResponsePublication::Fault { pending, error } => {
                            self.store_retained_state(
                                ControllerCommandPhase::PassiveScanResponse,
                                ControllerCommandState::PassiveScanResponse(pending),
                            );
                            return self
                                .retain_boundary(ControllerCommandBoundary::HciFault(error));
                        }
                    }
                }
                ControllerCommandPhase::PassiveScanActive => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanCpuResponse(_)
                    ) {
                        let ControllerCommandState::PassiveScanCpuResponse(pending) =
                            self.owner.current()
                        else {
                            unreachable!("the selected scanner response did not change")
                        };
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        let ControllerCommandState::PassiveScanCpuResponse(pending) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited scanner response did not change")
                        };
                        match pending.try_publish(controller) {
                            PassiveScanHciCpuResponsePublication::Published(completed) => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanComplete(completed));
                            }
                            PassiveScanHciCpuResponsePublication::Pending(pending) => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanCpuResponse(pending));
                            }
                            PassiveScanHciCpuResponsePublication::EndpointMismatch(pending) => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanCpuResponse(pending));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            PassiveScanHciCpuResponsePublication::Fault { pending, error } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanCpuResponse(pending));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanActiveResponse(_)
                    ) {
                        let ControllerCommandState::PassiveScanActiveResponse(pending) =
                            self.owner.current()
                        else {
                            unreachable!("the selected active scanner response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(
                                oer_esp32s31_bluetooth::le::scanning::PassiveScanActiveWait::Scheduler(
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            },
                            Some(
                                oer_esp32s31_bluetooth::le::scanning::PassiveScanActiveWait::PostUnlink(
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            },
                            None => true,
                        };
                        let ControllerCommandState::PassiveScanActiveResponse(pending) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited active scanner response did not change")
                        };
                        if radio_ready {
                            match pending.step_radio() {
                                PassiveScanHciActivePendingRadioStep::Continue(pending)
                                | PassiveScanHciActivePendingRadioStep::Waiting(pending) => {
                                    self.owner.store(
                                        ControllerCommandState::PassiveScanActiveResponse(pending),
                                    )
                                }
                                PassiveScanHciActivePendingRadioStep::UnrelatedList {
                                    pending,
                                    observed,
                                } => {
                                    return self.store_unowned_finished_list(
                                        ControllerCommandPhase::PassiveScanActive,
                                        UnownedFinishedListOwner::PassiveScanPending {
                                            _pending: pending,
                                            observed,
                                        },
                                    );
                                }
                                PassiveScanHciActivePendingRadioStep::CpuOwned(pending) => self
                                    .owner
                                    .store(ControllerCommandState::PassiveScanCpuResponse(pending)),
                                PassiveScanHciActivePendingRadioStep::Fault(fault) => {
                                    return self.terminal_boundary(
                                        ControllerCommandPhase::PassiveScanActive,
                                        ControllerCommandBoundary::PassiveScanPendingFault(fault),
                                    );
                                }
                            }
                        } else {
                            match pending.try_publish(controller) {
                                PassiveScanHciActiveResponsePublication::Published(active) => self
                                    .owner
                                    .store(ControllerCommandState::PassiveScanActive(active)),
                                PassiveScanHciActiveResponsePublication::Pending(pending) => {
                                    self.owner.store(
                                        ControllerCommandState::PassiveScanActiveResponse(pending),
                                    )
                                }
                                PassiveScanHciActiveResponsePublication::EndpointMismatch(
                                    pending,
                                ) => {
                                    self.owner.store(
                                        ControllerCommandState::PassiveScanActiveResponse(pending),
                                    );
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                                PassiveScanHciActiveResponsePublication::Fault {
                                    pending,
                                    error,
                                } => {
                                    self.owner.store(
                                        ControllerCommandState::PassiveScanActiveResponse(pending),
                                    );
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::HciFault(error),
                                    );
                                }
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanStopping(_)
                    ) {
                        let ControllerCommandState::PassiveScanStopping(stopping) =
                            self.owner.current()
                        else {
                            unreachable!("the selected scanner stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(
                                oer_esp32s31_bluetooth::le::scanning::PassiveScanActiveWait::Scheduler(
                                    wake,
                                ),
                            ) => wakers.wait_scheduler_ready(wake).await,
                            Some(
                                oer_esp32s31_bluetooth::le::scanning::PassiveScanActiveWait::PostUnlink(
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
                        let ControllerCommandState::PassiveScanStopping(stopping) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited scanner stopping owner did not change")
                        };
                        match stopping.step() {
                            PassiveScanHciStoppingStep::Continue(stopping)
                            | PassiveScanHciStoppingStep::Waiting(stopping) => self
                                .owner
                                .store(ControllerCommandState::PassiveScanStopping(stopping)),
                            PassiveScanHciStoppingStep::UnrelatedList { stopping, observed } => {
                                return self.store_unowned_finished_list(
                                    ControllerCommandPhase::PassiveScanActive,
                                    UnownedFinishedListOwner::PassiveScanStopping {
                                        _stopping: stopping,
                                        observed,
                                    },
                                );
                            }
                            PassiveScanHciStoppingStep::Disable(pending) => self.store_transition(
                                ControllerCommandPhase::PassiveScanActive,
                                ControllerCommandStimulus::IdleResponse,
                                ControllerCommandState::IdleResponse {
                                    pending,
                                    completion: ControllerIdleCompletion::PassiveScanDisable,
                                },
                            ),
                            PassiveScanHciStoppingStep::Reset(barrier) => self.store_transition(
                                ControllerCommandPhase::PassiveScanActive,
                                ControllerCommandStimulus::IdleReset,
                                ControllerCommandState::IdleReset(barrier),
                            ),
                            PassiveScanHciStoppingStep::Fault(fault) => {
                                return self.terminal_boundary(
                                    ControllerCommandPhase::PassiveScanActive,
                                    ControllerCommandBoundary::PassiveScanStoppingFault(fault),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanComplete(_)
                    ) {
                        let ControllerCommandState::PassiveScanComplete(completed) =
                            self.owner.take()
                        else {
                            unreachable!("the selected complete scanner did not change")
                        };
                        let buffer = packet
                            .take()
                            .expect("scanner command intake retains its sole scratch buffer");
                        match completed.try_route_controller_command_with_buffer(controller, buffer)
                        {
                            PassiveScanHciCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    PassiveScanHciCommandRoute::ResponsePending(pending) => {
                                        self.owner.store(
                                            ControllerCommandState::PassiveScanCpuResponse(pending),
                                        )
                                    }
                                    PassiveScanHciCommandRoute::Disable(pending) => {
                                        self.store_transition(
                                            ControllerCommandPhase::PassiveScanActive,
                                            ControllerCommandStimulus::IdleResponse,
                                            ControllerCommandState::IdleResponse {
                                                pending,
                                                completion:
                                                    ControllerIdleCompletion::PassiveScanDisable,
                                            },
                                        );
                                    }
                                    PassiveScanHciCommandRoute::Reset(barrier) => {
                                        self.store_transition(
                                            ControllerCommandPhase::PassiveScanActive,
                                            ControllerCommandStimulus::IdleReset,
                                            ControllerCommandState::IdleReset(barrier),
                                        );
                                    }
                                    PassiveScanHciCommandRoute::EndpointMismatch(mismatch) => {
                                        return self.terminal_boundary(
                                            ControllerCommandPhase::PassiveScanActive,
                                            ControllerCommandBoundary::PassiveScanCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                            }
                            PassiveScanHciCommandIntake::Empty { completed, buffer } => {
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
                                                PassiveScanRecurringDrive::Failed(failure),
                                            )
                                        {
                                            return boundary;
                                        }
                                    }
                                }
                            }
                            PassiveScanHciCommandIntake::EndpointMismatch {
                                completed,
                                buffer: _,
                            } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanComplete(completed));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            PassiveScanHciCommandIntake::Channel {
                                completed,
                                buffer: _,
                                error,
                            } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanComplete(completed));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                            PassiveScanHciCommandIntake::NonCommand { completed, frame } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanComplete(completed));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanRecurringRetry(_)
                    ) {
                        let ControllerCommandState::PassiveScanRecurringRetry(failure) =
                            self.owner.take()
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
                                    ControllerCommandPhase::PassiveScanActive,
                                    ControllerCommandBoundary::PassiveScanRecurringFault(failure),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PassiveScanRecurring(_)
                    ) {
                        recheck.wait_until_absolute_recheck().await;
                        let ControllerCommandState::PassiveScanRecurring(runner) =
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
                        ControllerCommandState::PassiveScanReports(_)
                    ) {
                        let ControllerCommandState::PassiveScanReports(reports) =
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
                                    let ControllerCommandState::PassiveScanReports(reports) =
                                        self.owner.take()
                                    else {
                                        unreachable!(
                                            "the command-ready scanner reports did not change"
                                        )
                                    };
                                    self.owner
                                        .store(ControllerCommandState::PassiveScanComplete(
                                            reports.discard_remaining(),
                                        ));
                                    continue;
                                }
                                Either::First(Err(_)) | Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        let ControllerCommandState::PassiveScanReports(reports) = self.owner.take()
                        else {
                            unreachable!("the awaited scanner reports did not change")
                        };
                        match reports.step(controller) {
                            PassiveScanHciReportStep::Published(reports)
                            | PassiveScanHciReportStep::Masked(reports) => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanReports(reports));
                            }
                            PassiveScanHciReportStep::Pending { reports, error } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanReports(reports));
                                if error != HciChannelError::Full {
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::HciFault(error),
                                    );
                                }
                            }
                            PassiveScanHciReportStep::IgnoredMalformed { reports, error } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanReports(reports));
                                return self.retain_boundary(
                                    ControllerCommandBoundary::PassiveScanMalformedPdu(error),
                                );
                            }
                            PassiveScanHciReportStep::EncodingFault { reports, error } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanReports(reports));
                                return self.retain_boundary(
                                    ControllerCommandBoundary::PassiveScanReportEncodingFault(
                                        error,
                                    ),
                                );
                            }
                            PassiveScanHciReportStep::EndpointMismatch(reports) => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanReports(reports));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            PassiveScanHciReportStep::Complete(completed) => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanComplete(completed));
                            }
                        }
                        continue;
                    }

                    let ControllerCommandState::PassiveScanActive(active) = self.owner.current()
                    else {
                        unreachable!("the selected active scanner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(
                            oer_esp32s31_bluetooth::le::scanning::PassiveScanActiveWait::Scheduler(
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
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                        },
                        Some(
                            oer_esp32s31_bluetooth::le::scanning::PassiveScanActiveWait::PostUnlink(
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
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                        },
                        None => true,
                    };
                    let ControllerCommandState::PassiveScanActive(active) = self.owner.take()
                    else {
                        unreachable!("the awaited active scanner did not change")
                    };
                    if !radio_ready {
                        let buffer = packet
                            .take()
                            .expect("active scanner intake retains its sole scratch buffer");
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            PassiveScanHciActiveCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    PassiveScanHciActiveCommandRoute::ResponsePending(pending) => {
                                        self.owner.store(
                                            ControllerCommandState::PassiveScanActiveResponse(
                                                pending,
                                            ),
                                        )
                                    }
                                    PassiveScanHciActiveCommandRoute::Stopping(stopping) => {
                                        self.owner.store(
                                            ControllerCommandState::PassiveScanStopping(stopping),
                                        )
                                    }
                                    PassiveScanHciActiveCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            ControllerCommandPhase::PassiveScanActive,
                                            ControllerCommandBoundary::PassiveScanActiveCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                            }
                            PassiveScanHciActiveCommandIntake::Empty { active, buffer } => {
                                packet = Some(buffer);
                                self.owner
                                    .store(ControllerCommandState::PassiveScanActive(active));
                            }
                            PassiveScanHciActiveCommandIntake::EndpointMismatch {
                                active,
                                buffer: _,
                            } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanActive(active));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            PassiveScanHciActiveCommandIntake::Channel {
                                active,
                                buffer: _,
                                error,
                            } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanActive(active));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                            PassiveScanHciActiveCommandIntake::NonCommand { active, frame } => {
                                self.owner
                                    .store(ControllerCommandState::PassiveScanActive(active));
                                return self
                                    .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                            }
                        }
                        continue;
                    }
                    match drive_passive_scan_active_ready(active) {
                        PassiveScanActiveDrive::Waiting(active) => self
                            .owner
                            .store(ControllerCommandState::PassiveScanActive(active)),
                        PassiveScanActiveDrive::Reports(reports) => self
                            .owner
                            .store(ControllerCommandState::PassiveScanReports(reports)),
                        PassiveScanActiveDrive::UnrelatedList { session, observed } => {
                            return self.store_unowned_finished_list(
                                ControllerCommandPhase::PassiveScanActive,
                                UnownedFinishedListOwner::PassiveScan {
                                    _session: session,
                                    observed,
                                },
                            );
                        }
                        PassiveScanActiveDrive::Fault(fault) => {
                            return self.terminal_boundary(
                                ControllerCommandPhase::PassiveScanActive,
                                ControllerCommandBoundary::PassiveScanFault(fault),
                            );
                        }
                    }
                }
                ControllerCommandPhase::LegacyAdvertisingActive => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingCpuResponse(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingCpuResponse(pending) =
                            self.owner.current()
                        else {
                            unreachable!("the selected advertising response did not change")
                        };
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        let ControllerCommandState::LegacyAdvertisingCpuResponse(pending) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited advertising response did not change")
                        };
                        match pending.try_publish(controller) {
                            LegacyAdvertisingCpuOwnedResponsePublication::Published(completed) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuOwned(completed),
                                )
                            }
                            LegacyAdvertisingCpuOwnedResponsePublication::Pending(pending) => self
                                .store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuResponse(pending),
                                ),
                            LegacyAdvertisingCpuOwnedResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuResponse(pending),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyAdvertisingCpuOwnedResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuResponse(pending),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingRecurringStopRestore(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingRecurringStopRestore(restore) =
                            self.owner.current()
                        else {
                            unreachable!("the selected recurring stop restore did not change")
                        };
                        if restore.controller_time_drain_required() {
                            recheck.wait_until_absolute_recheck().await;
                        }
                        let ControllerCommandState::LegacyAdvertisingRecurringStopRestore(restore) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited recurring stop restore did not change")
                        };
                        match restore.step() {
                            LegacyAdvertisingRecurringStopRestoreStep::WaitControllerTime(
                                restore,
                            ) => self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                                    restore,
                                ),
                            ),
                            LegacyAdvertisingRecurringStopRestoreStep::DisableResponse(pending) => {
                                self.store_transition(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                    ControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                    },
                                )
                            }
                            LegacyAdvertisingRecurringStopRestoreStep::ResetCompletion(ready) => {
                                self.store_transition(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                    ControllerCommandState::LegacyAdvertisingResetCompletion {
                                        ready,
                                        origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                    },
                                )
                            }
                            LegacyAdvertisingRecurringStopRestoreStep::Rejected(restore) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                                        restore,
                                    ),
                                );
                                return self.retain_boundary(ControllerCommandBoundary::Retryable(
                                    ControllerRetry::LegacyAdvertisingRecurringStopRestore,
                                ));
                            }
                            LegacyAdvertisingRecurringStopRestoreStep::Fault(fault) => {
                                return self.terminal_boundary(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandBoundary::LegacyAdvertisingRecurringStopFault(
                                        fault,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingDisableRestore(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingDisableRestore(restore) =
                            self.owner.take()
                        else {
                            unreachable!("the selected advertising Disable restore did not change")
                        };
                        match restore.restore() {
                            LegacyAdvertisingDisableRestoreStep::ResponsePending(pending) => self
                                .store_transition(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                    ControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                    },
                                ),
                            LegacyAdvertisingDisableRestoreStep::Rejected(restore) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingDisableRestore(
                                        restore,
                                    ),
                                );
                                return self.retain_boundary(ControllerCommandBoundary::Retryable(
                                    ControllerRetry::LegacyAdvertisingDisableRestore,
                                ));
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingResetRestore(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingResetRestore(restore) =
                            self.owner.take()
                        else {
                            unreachable!("the selected advertising Reset restore did not change")
                        };
                        match restore.restore() {
                            LegacyAdvertisingResetRestoreStep::CompletionReady(ready) => self
                                .store_transition(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::LegacyAdvertisingStopCompletion,
                                    ControllerCommandState::LegacyAdvertisingResetCompletion {
                                        ready,
                                        origin: LegacyAdvertisingStopOrigin::LegacyAdvertising,
                                    },
                                ),
                            LegacyAdvertisingResetRestoreStep::Rejected(restore) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingResetRestore(restore),
                                );
                                return self.retain_boundary(ControllerCommandBoundary::Retryable(
                                    ControllerRetry::LegacyAdvertisingResetRestore,
                                ));
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingCpuOwned(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingCpuOwned(completed) =
                            self.owner.take()
                        else {
                            unreachable!("the selected completed advertising event did not change")
                        };
                        let buffer = packet
                            .take()
                            .expect("advertising command intake retains its sole scratch buffer");
                        let completed = match completed
                            .try_route_controller_command_with_buffer(controller, buffer)
                        {
                            LegacyAdvertisingCpuOwnedCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    LegacyAdvertisingCpuOwnedCommandRoute::ResponsePending(
                                        pending,
                                    ) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingCpuResponse(
                                            pending,
                                        ),
                                    ),
                                    LegacyAdvertisingCpuOwnedCommandRoute::Disable(restore) => self
                                        .store_retained_state(
                                            ControllerCommandPhase::LegacyAdvertisingActive,
                                            ControllerCommandState::LegacyAdvertisingDisableRestore(
                                                restore,
                                            ),
                                        ),
                                    LegacyAdvertisingCpuOwnedCommandRoute::ResetBarrier(
                                        barrier,
                                    ) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingResetRestore(
                                            barrier.begin_restore(),
                                        ),
                                    ),
                                    LegacyAdvertisingCpuOwnedCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            ControllerCommandPhase::LegacyAdvertisingActive,
                                            ControllerCommandBoundary::LegacyAdvertisingCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                                continue;
                            }
                            LegacyAdvertisingCpuOwnedCommandIntake::Empty { completed, buffer } => {
                                packet = Some(buffer);
                                completed
                            }
                            LegacyAdvertisingCpuOwnedCommandIntake::EndpointMismatch {
                                completed,
                                buffer: _,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuOwned(completed),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyAdvertisingCpuOwnedCommandIntake::Channel {
                                completed,
                                buffer: _,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuOwned(completed),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                            LegacyAdvertisingCpuOwnedCommandIntake::NonCommand {
                                completed,
                                frame,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuOwned(completed),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                            }
                        };
                        match completed.begin_recurring(advertising_delay.next_advertising_delay())
                        {
                            LegacyAdvertisingRecurringStart::Runner(runner) => {
                                if let Some(boundary) = self
                                    .store_legacy_advertising_recurring_drive(
                                        drive_legacy_advertising_recurring_ready(runner),
                                    )
                                {
                                    return boundary;
                                }
                            }
                            LegacyAdvertisingRecurringStart::SequenceExhausted(completed) => {
                                let index = completed.hardware_list_index();
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingCpuOwned(completed),
                                );
                                return ControllerCommandBoundary::LegacyAdvertisingSequenceExhausted(index);
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingRecurringRetry(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingRecurringRetry(retry) =
                            self.owner.take()
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
                        ControllerCommandState::LegacyAdvertisingRecurring(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingRecurring(runner) =
                            self.owner.current()
                        else {
                            unreachable!("the selected recurring wait did not change")
                        };
                        if runner.order_state() == LegacyAdvertisingRecurringOrderState::Stopping {
                            let ControllerCommandState::LegacyAdvertisingRecurring(runner) =
                                self.owner.take()
                            else {
                                unreachable!("the stopping recurring owner did not change")
                            };
                            match runner.begin_stopping() {
                                LegacyAdvertisingRecurringStopBegin::Restore(restore) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingRecurringStopRestore(
                                            restore,
                                        ),
                                    );
                                }
                                LegacyAdvertisingRecurringStopBegin::Published(runner) => {
                                    if let Some(boundary) = self
                                        .store_legacy_advertising_recurring_drive(
                                            drive_legacy_advertising_recurring_ready(runner),
                                        )
                                    {
                                        return boundary;
                                    }
                                }
                                LegacyAdvertisingRecurringStopBegin::Fault(fault) => {
                                    return self.terminal_boundary(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandBoundary::LegacyAdvertisingRecurringFault(
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
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                        };
                        let ControllerCommandState::LegacyAdvertisingRecurring(runner) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited recurring owner did not change")
                        };
                        if let Some(progress) = order_progress {
                            match progress {
                                LegacyAdvertisingRecurringOrderProgress::Command => {
                                    let buffer = packet.take().expect(
                                        "recurring command intake retains its sole scratch buffer",
                                    );
                                    match runner.try_route_controller_command_with_buffer(
                                        controller, buffer,
                                    ) {
                                        LegacyAdvertisingRecurringCommandIntake::Routed {
                                            route,
                                            buffer,
                                        } => {
                                            packet = Some(buffer);
                                            match route {
                                                LegacyAdvertisingRecurringCommandRoute::Continue(
                                                    runner,
                                                ) => self.store_retained_state(
                                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                                    ControllerCommandState::LegacyAdvertisingRecurring(
                                                        runner,
                                                    ),
                                                ),
                                                LegacyAdvertisingRecurringCommandRoute::EndpointMismatch(
                                                    mismatch,
                                                ) => {
                                                    return self.terminal_boundary(
                                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                                        ControllerCommandBoundary::LegacyAdvertisingRecurringCommandEndpointMismatch(
                                                            mismatch,
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                        LegacyAdvertisingRecurringCommandIntake::Empty {
                                            runner,
                                            buffer,
                                        } => {
                                            packet = Some(buffer);
                                            self.store_retained_state(
                                                ControllerCommandPhase::LegacyAdvertisingActive,
                                                ControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                        }
                                        LegacyAdvertisingRecurringCommandIntake::EndpointMismatch {
                                            runner,
                                            buffer: _,
                                        } => {
                                            self.store_retained_state(
                                                ControllerCommandPhase::LegacyAdvertisingActive,
                                                ControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                ControllerCommandBoundary::EndpointMismatch,
                                            );
                                        }
                                        LegacyAdvertisingRecurringCommandIntake::Channel {
                                            runner,
                                            buffer: _,
                                            error,
                                        } => {
                                            self.store_retained_state(
                                                ControllerCommandPhase::LegacyAdvertisingActive,
                                                ControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                ControllerCommandBoundary::HciFault(error),
                                            );
                                        }
                                        LegacyAdvertisingRecurringCommandIntake::NonCommand {
                                            runner,
                                            frame,
                                        } => {
                                            self.store_retained_state(
                                                ControllerCommandPhase::LegacyAdvertisingActive,
                                                ControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                ControllerCommandBoundary::NonCommand(frame),
                                            );
                                        }
                                    }
                                }
                                LegacyAdvertisingRecurringOrderProgress::Response => {
                                    match runner.try_publish_response(controller) {
                                        LegacyAdvertisingRecurringResponsePublication::Published(
                                            runner,
                                        )
                                        | LegacyAdvertisingRecurringResponsePublication::Pending(
                                            runner,
                                        ) => self.store_retained_state(
                                            ControllerCommandPhase::LegacyAdvertisingActive,
                                            ControllerCommandState::LegacyAdvertisingRecurring(
                                                runner,
                                            ),
                                        ),
                                        LegacyAdvertisingRecurringResponsePublication::EndpointMismatch(
                                            runner,
                                        ) => {
                                            self.store_retained_state(
                                                ControllerCommandPhase::LegacyAdvertisingActive,
                                                ControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                ControllerCommandBoundary::EndpointMismatch,
                                            );
                                        }
                                        LegacyAdvertisingRecurringResponsePublication::Fault {
                                            runner,
                                            error,
                                        } => {
                                            self.store_retained_state(
                                                ControllerCommandPhase::LegacyAdvertisingActive,
                                                ControllerCommandState::LegacyAdvertisingRecurring(
                                                    runner,
                                                ),
                                            );
                                            return self.retain_boundary(
                                                ControllerCommandBoundary::HciFault(error),
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
                        ControllerCommandState::LegacyAdvertisingActiveResponse(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingActiveResponse(pending) =
                            self.owner.current()
                        else {
                            unreachable!("the selected active advertising response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(LegacyAdvertisingActiveWait::Scheduler(wake)) => match select(
                                wakers.wait_scheduler_ready(wake),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            },
                            Some(LegacyAdvertisingActiveWait::PostUnlink(wake)) => {
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
                                            ControllerCommandBoundary::EndpointMismatch,
                                        );
                                    }
                                }
                            }
                            None => true,
                        };
                        let ControllerCommandState::LegacyAdvertisingActiveResponse(pending) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited active advertising response did not change")
                        };
                        if radio_ready {
                            match pending.step_radio() {
                                LegacyAdvertisingActivePendingRadioStep::Continue(pending) => self
                                    .store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    ),
                                LegacyAdvertisingActivePendingRadioStep::Waiting(pending) => self
                                    .store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    ),
                                LegacyAdvertisingActivePendingRadioStep::UnrelatedList {
                                    pending,
                                    observed,
                                } => {
                                    return self.store_unowned_finished_list(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        UnownedFinishedListOwner::LegacyAdvertisingPending {
                                            _pending: pending,
                                            observed,
                                        },
                                    );
                                }
                                LegacyAdvertisingActivePendingRadioStep::CpuOwned(pending) => self
                                    .store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingCpuResponse(
                                            pending,
                                        ),
                                    ),
                                LegacyAdvertisingActivePendingRadioStep::Fault(fault) => {
                                    return self.terminal_boundary(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandBoundary::LegacyAdvertisingPendingFault(
                                            fault,
                                        ),
                                    );
                                }
                            }
                        } else {
                            match pending.try_publish(controller) {
                                LegacyAdvertisingActiveResponsePublication::Published(active) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActive(active),
                                    )
                                }
                                LegacyAdvertisingActiveResponsePublication::Pending(pending) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    )
                                }
                                LegacyAdvertisingActiveResponsePublication::EndpointMismatch(
                                    pending,
                                ) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    );
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                                LegacyAdvertisingActiveResponsePublication::Fault {
                                    pending,
                                    error,
                                } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    );
                                    return self.retain_boundary(
                                        ControllerCommandBoundary::HciFault(error),
                                    );
                                }
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingStopping(_)
                    ) {
                        let ControllerCommandState::LegacyAdvertisingStopping(stopping) =
                            self.owner.current()
                        else {
                            unreachable!("the selected advertising stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(LegacyAdvertisingActiveWait::Scheduler(wake)) => {
                                wakers.wait_scheduler_ready(wake).await;
                            }
                            Some(LegacyAdvertisingActiveWait::PostUnlink(wake)) => {
                                let _ = wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            None => {}
                        }
                        let ControllerCommandState::LegacyAdvertisingStopping(stopping) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited advertising stopping owner did not change")
                        };
                        match stopping.step() {
                            LegacyAdvertisingStoppingStep::Continue(stopping)
                            | LegacyAdvertisingStoppingStep::Waiting(stopping) => self
                                .store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingStopping(stopping),
                                ),
                            LegacyAdvertisingStoppingStep::UnrelatedList { stopping, observed } => {
                                return self.store_unowned_finished_list(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    UnownedFinishedListOwner::LegacyAdvertisingStopping {
                                        _stopping: stopping,
                                        observed,
                                    },
                                );
                            }
                            LegacyAdvertisingStoppingStep::DisableRestore(restore) => self
                                .store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingDisableRestore(
                                        restore,
                                    ),
                                ),
                            LegacyAdvertisingStoppingStep::ResetRestore(restore) => self
                                .store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingResetRestore(restore),
                                ),
                            LegacyAdvertisingStoppingStep::Fault(fault) => {
                                return self.terminal_boundary(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandBoundary::LegacyAdvertisingStoppingFault(
                                        fault,
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    let ControllerCommandState::LegacyAdvertisingActive(active) =
                        self.owner.current()
                    else {
                        unreachable!("the selected active advertising owner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(LegacyAdvertisingActiveWait::Scheduler(wake)) => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        Some(LegacyAdvertisingActiveWait::PostUnlink(wake)) => {
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
                                        ControllerCommandBoundary::EndpointMismatch,
                                    );
                                }
                            }
                        }
                        None => true,
                    };
                    let ControllerCommandState::LegacyAdvertisingActive(active) = self.owner.take()
                    else {
                        unreachable!("the driven active advertising owner did not change")
                    };
                    if !radio_ready {
                        let buffer = packet
                            .take()
                            .expect("active advertising intake retains its sole scratch buffer");
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            LegacyAdvertisingActiveCommandIntake::Routed { route, buffer } => {
                                packet = Some(buffer);
                                match route {
                                    LegacyAdvertisingActiveCommandRoute::ResponsePending(
                                        pending,
                                    ) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyAdvertisingActive,
                                        ControllerCommandState::LegacyAdvertisingActiveResponse(
                                            pending,
                                        ),
                                    ),
                                    LegacyAdvertisingActiveCommandRoute::Stopping(stopping) => self
                                        .store_retained_state(
                                            ControllerCommandPhase::LegacyAdvertisingActive,
                                            ControllerCommandState::LegacyAdvertisingStopping(
                                                stopping,
                                            ),
                                        ),
                                    LegacyAdvertisingActiveCommandRoute::EndpointMismatch(
                                        mismatch,
                                    ) => {
                                        return self.terminal_boundary(
                                            ControllerCommandPhase::LegacyAdvertisingActive,
                                            ControllerCommandBoundary::LegacyAdvertisingActiveCommandEndpointMismatch(
                                                mismatch,
                                            ),
                                        );
                                    }
                                }
                            }
                            LegacyAdvertisingActiveCommandIntake::Empty { active, buffer } => {
                                packet = Some(buffer);
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingActive(active),
                                );
                            }
                            LegacyAdvertisingActiveCommandIntake::EndpointMismatch {
                                active,
                                buffer: _,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingActive(active),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyAdvertisingActiveCommandIntake::Channel {
                                active,
                                buffer: _,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingActive(active),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                            LegacyAdvertisingActiveCommandIntake::NonCommand { active, frame } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandState::LegacyAdvertisingActive(active),
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                            }
                        }
                        continue;
                    }
                    match drive_legacy_advertising_active_ready(active) {
                        LegacyAdvertisingActiveDrive::Waiting(active) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandState::LegacyAdvertisingActive(active),
                            );
                        }
                        LegacyAdvertisingActiveDrive::CpuOwned(completed) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandState::LegacyAdvertisingCpuOwned(completed),
                            );
                        }
                        LegacyAdvertisingActiveDrive::UnrelatedList { session, observed } => {
                            return self.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyAdvertisingActive,
                                UnownedFinishedListOwner::LegacyAdvertising {
                                    _session: session,
                                    observed,
                                },
                            );
                        }
                        LegacyAdvertisingActiveDrive::Fault(fault) => {
                            return self.terminal_boundary(
                                ControllerCommandPhase::LegacyAdvertisingActive,
                                ControllerCommandBoundary::LegacyAdvertisingFault(fault),
                            );
                        }
                    }
                }
                ControllerCommandPhase::LegacyAdvertisingStopCompletion => {
                    let phase = ControllerCommandPhase::LegacyAdvertisingStopCompletion;
                    if let ControllerCommandState::LegacyAdvertisingDisableResponse {
                        pending,
                        ..
                    } = self.owner.current()
                    {
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        let ControllerCommandState::LegacyAdvertisingDisableResponse {
                            pending,
                            origin,
                        } = self.owner.take()
                        else {
                            unreachable!("the awaited advertising Disable response did not change")
                        };
                        match pending.try_publish(controller) {
                            LegacyAdvertisingDisableResponsePublication::Completed(idle) => {
                                self.store_transition(
                                    phase,
                                    ControllerCommandStimulus::IdleRestored,
                                    ControllerCommandState::Idle(idle),
                                );
                                return ControllerCommandBoundary::IdleRestored(
                                    origin.disable_completion(),
                                );
                            }
                            LegacyAdvertisingDisableResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin,
                                    },
                                );
                            }
                            LegacyAdvertisingDisableResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyAdvertisingDisableResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingDisableResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyAdvertisingResetCompletion { .. }
                    ) {
                        let ControllerCommandState::LegacyAdvertisingResetCompletion {
                            ready,
                            origin,
                        } = self.owner.take()
                        else {
                            unreachable!("the selected advertising Reset completion did not change")
                        };
                        match ready.complete(controller) {
                            LegacyAdvertisingResetCompletion::ResponsePending(pending) => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                            }
                            LegacyAdvertisingResetCompletion::EndpointMismatch(ready) => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingResetCompletion {
                                        ready,
                                        origin,
                                    },
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                        }
                        continue;
                    }

                    if let ControllerCommandState::LegacyAdvertisingResetResponse {
                        pending, ..
                    } = self.owner.current()
                    {
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        let ControllerCommandState::LegacyAdvertisingResetResponse {
                            pending,
                            origin,
                        } = self.owner.take()
                        else {
                            unreachable!("the awaited advertising Reset response did not change")
                        };
                        match pending.try_publish(controller) {
                            LegacyAdvertisingResetResponsePublication::Completed(idle) => {
                                self.store_transition(
                                    phase,
                                    ControllerCommandStimulus::IdleRestored,
                                    ControllerCommandState::Idle(idle),
                                );
                                return ControllerCommandBoundary::IdleRestored(
                                    ControllerIdleCompletion::Reset,
                                );
                            }
                            LegacyAdvertisingResetResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                            }
                            LegacyAdvertisingResetResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                            }
                            LegacyAdvertisingResetResponsePublication::Fault { pending, error } => {
                                self.store_retained_state(
                                    phase,
                                    ControllerCommandState::LegacyAdvertisingResetResponse {
                                        pending,
                                        origin,
                                    },
                                );
                                return self
                                    .retain_boundary(ControllerCommandBoundary::HciFault(error));
                            }
                        }
                        continue;
                    }

                    unreachable!("the selected advertising stop completion did not change")
                }
                ControllerCommandPhase::FirstEvent => {
                    if matches!(self.owner.current(), ControllerCommandState::FirstRetry(_)) {
                        let ControllerCommandState::FirstRetry(retry) = self.owner.take() else {
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
                    if matches!(self.owner.current(), ControllerCommandState::FirstEvent(_)) {
                        let ControllerCommandState::FirstEvent(wait) = self.owner.current_mut()
                        else {
                            unreachable!("the selected first-event wait did not change")
                        };
                        wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                            .await;
                        let ControllerCommandState::FirstEvent(wait) = self.owner.take() else {
                            unreachable!("the awaited first-event wait did not change")
                        };
                        match wait.resume() {
                            DtmFirstResume::Ready(drive) => {
                                if let Some(boundary) = self.store_first_drive(drive) {
                                    return boundary;
                                }
                            }
                            DtmFirstResume::NotReady(wait) => self.store_retained_state(
                                ControllerCommandPhase::FirstEvent,
                                ControllerCommandState::FirstEvent(wait),
                            ),
                        }
                    } else {
                        let ControllerCommandState::FirstCleanup { cleanup, readiness } =
                            self.owner.current()
                        else {
                            unreachable!("the selected first-event cleanup did not change")
                        };
                        if matches!(readiness, FirstCleanupReadiness::RecheckRequired) {
                            let _retained_owner = cleanup;
                            recheck.wait_until_absolute_recheck().await;
                        }
                        let ControllerCommandState::FirstCleanup { cleanup, .. } =
                            self.owner.take()
                        else {
                            unreachable!("the awaited first-event cleanup did not change")
                        };
                        match cleanup.step() {
                            DtmFirstPreparationCleanupStep::Waiting(cleanup) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::FirstEvent,
                                    ControllerCommandState::FirstCleanup {
                                        cleanup,
                                        readiness: FirstCleanupReadiness::RecheckRequired,
                                    },
                                );
                            }
                            DtmFirstPreparationCleanupStep::Continue(cleanup) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::FirstEvent,
                                    ControllerCommandState::FirstCleanup {
                                        cleanup,
                                        readiness: FirstCleanupReadiness::Ready,
                                    },
                                );
                            }
                            DtmFirstPreparationCleanupStep::CleanTask(clean) => {
                                match clean.into_completion() {
                                    DtmFirstPreparationCompletion::ResponsePending(pending) => self
                                        .store_transition(
                                            ControllerCommandPhase::FirstEvent,
                                            ControllerCommandStimulus::IdleResponse,
                                            ControllerCommandState::IdleResponse {
                                                pending,
                                                completion:
                                                    ControllerIdleCompletion::DtmStartRejected,
                                            },
                                        ),
                                    DtmFirstPreparationCompletion::FailStop(fail_stop) => {
                                        return self.terminal_boundary(
                                            ControllerCommandPhase::FirstEvent,
                                            ControllerCommandBoundary::FirstPreparationFailStop(
                                                fail_stop,
                                            ),
                                        );
                                    }
                                }
                            }
                            DtmFirstPreparationCleanupStep::Fault { cleanup, error } => {
                                return self.terminal_boundary(
                                    ControllerCommandPhase::FirstEvent,
                                    ControllerCommandBoundary::FirstPreparationCleanupFault {
                                        cleanup,
                                        error,
                                    },
                                );
                            }
                            DtmFirstPreparationCleanupStep::RestoreRejected(cleanup) => {
                                return self.terminal_boundary(
                                    ControllerCommandPhase::FirstEvent,
                                    ControllerCommandBoundary::FirstPreparationRestoreRejected(
                                        cleanup,
                                    ),
                                );
                            }
                        }
                    }
                }
                ControllerCommandPhase::Active => {
                    let buffer = packet
                        .take()
                        .expect("the active session retains its sole scratch buffer");
                    let ControllerCommandState::Active(active) = self.owner.current_mut() else {
                        unreachable!("the selected active phase did not change")
                    };
                    let boundary = active.run(wakers, controller, buffer, recheck).await;
                    match boundary {
                        DtmSessionBoundary::UnownedFinishedList(index) => {
                            let ControllerCommandState::Active(active) = self.owner.take() else {
                                unreachable!("unowned list retained the selected active task")
                            };
                            return self.store_unowned_finished_list(
                                ControllerCommandPhase::Active,
                                UnownedFinishedListOwner::Active {
                                    _task: active,
                                    index,
                                },
                            );
                        }
                        DtmSessionBoundary::ResetBarrier(barrier) => {
                            let ControllerCommandState::Active(active) = self.owner.take() else {
                                unreachable!("active Reset transferred the selected session")
                            };
                            debug_assert!(active.is_empty());
                            self.store_transition(
                                ControllerCommandPhase::Active,
                                ControllerCommandStimulus::ResetStopping,
                                ControllerCommandState::ResetStopping(barrier.begin_quiescence()),
                            );
                        }
                        DtmSessionBoundary::NonCommand(frame) => {
                            return self
                                .retain_boundary(ControllerCommandBoundary::NonCommand(frame));
                        }
                        DtmSessionBoundary::ControllerCommandEndpointMismatch(mismatch) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                ControllerCommandPhase::Active,
                                ControllerCommandBoundary::ActiveCommandEndpointMismatch(mismatch),
                            );
                        }
                        DtmSessionBoundary::EndpointMismatch => {
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        DtmSessionBoundary::HciFault(error) => {
                            return self
                                .retain_boundary(ControllerCommandBoundary::HciFault(error));
                        }
                        DtmSessionBoundary::Retryable(retry) => {
                            return self.retain_boundary(ControllerCommandBoundary::Retryable(
                                ControllerRetry::Active(retry),
                            ));
                        }
                        DtmSessionBoundary::ControllerTimeExhausted => {
                            return self.retain_boundary(
                                ControllerCommandBoundary::ControllerTimeExhausted,
                            );
                        }
                        DtmSessionBoundary::PendingRadioFault(fault) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                ControllerCommandPhase::Active,
                                ControllerCommandBoundary::PendingRadioFault(fault),
                            );
                        }
                        DtmSessionBoundary::CommandReadyRadioFault(fault) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                ControllerCommandPhase::Active,
                                ControllerCommandBoundary::CommandReadyRadioFault(fault),
                            );
                        }
                        DtmSessionBoundary::StoppingFault(fault) => {
                            let _empty = self.owner.take();
                            return self.terminal_boundary(
                                ControllerCommandPhase::Active,
                                ControllerCommandBoundary::TestEndStoppingFault(fault),
                            );
                        }
                        DtmSessionBoundary::Complete(idle) => {
                            let _empty = self.owner.take();
                            self.store_transition(
                                ControllerCommandPhase::Active,
                                ControllerCommandStimulus::IdleRestored,
                                ControllerCommandState::Idle(idle),
                            );
                            return ControllerCommandBoundary::IdleRestored(
                                ControllerIdleCompletion::TestEnd,
                            );
                        }
                    }
                }
                ControllerCommandPhase::ResetStopping => {
                    self.wait_reset_stopping(wakers, recheck).await;
                    let ControllerCommandState::ResetStopping(runner) = self.owner.take() else {
                        unreachable!("the awaited Reset-stopping phase did not change")
                    };
                    match runner.step() {
                        DtmResetStoppingStep::Continue(runner)
                        | DtmResetStoppingStep::Waiting(runner) => self
                            .owner
                            .store(ControllerCommandState::ResetStopping(runner)),
                        DtmResetStoppingStep::UnrelatedList { runner, observed } => {
                            return self.store_unowned_finished_list(
                                ControllerCommandPhase::ResetStopping,
                                UnownedFinishedListOwner::ResetStopping {
                                    _runner: runner,
                                    observed,
                                },
                            );
                        }
                        DtmResetStoppingStep::Retryable(runner) => {
                            self.owner
                                .store(ControllerCommandState::ResetStopping(runner));
                            return self.retain_boundary(ControllerCommandBoundary::Retryable(
                                ControllerRetry::ResetStopping,
                            ));
                        }
                        DtmResetStoppingStep::CompletionReady(ready) => {
                            self.store_transition(
                                ControllerCommandPhase::ResetStopping,
                                ControllerCommandStimulus::ResetCompletion,
                                ControllerCommandState::ResetCompletion(ready),
                            );
                        }
                        DtmResetStoppingStep::RestoreFailed(failure) => {
                            self.store_transition(
                                ControllerCommandPhase::ResetStopping,
                                ControllerCommandStimulus::ResetRestore,
                                ControllerCommandState::ResetRestore(failure),
                            );
                        }
                        DtmResetStoppingStep::Fault(fault) => {
                            return self.terminal_boundary(
                                ControllerCommandPhase::ResetStopping,
                                ControllerCommandBoundary::ResetStoppingFault(fault),
                            );
                        }
                    }
                }
                ControllerCommandPhase::ResetRestore => {
                    let ControllerCommandState::ResetRestore(failure) = self.owner.take() else {
                        unreachable!("the selected Reset-restore phase did not change")
                    };
                    match failure.retry_restore() {
                        DtmResetRestoreStep::CompletionReady(ready) => {
                            self.store_transition(
                                ControllerCommandPhase::ResetRestore,
                                ControllerCommandStimulus::ResetCompletion,
                                ControllerCommandState::ResetCompletion(ready),
                            );
                        }
                        DtmResetRestoreStep::Rejected(failure) => {
                            self.owner
                                .store(ControllerCommandState::ResetRestore(failure));
                            return self.retain_boundary(ControllerCommandBoundary::Retryable(
                                ControllerRetry::ResetRestore,
                            ));
                        }
                    }
                }
                ControllerCommandPhase::ResetCompletion => {
                    let ControllerCommandState::ResetCompletion(ready) = self.owner.take() else {
                        unreachable!("the selected Reset-completion phase did not change")
                    };
                    match ready.complete(controller) {
                        DtmResetCompletionStart::ResponsePending(pending) => {
                            self.store_transition(
                                ControllerCommandPhase::ResetCompletion,
                                ControllerCommandStimulus::ResetResponse,
                                ControllerCommandState::ResetResponse(pending),
                            );
                        }
                        DtmResetCompletionStart::EndpointMismatch(ready) => {
                            self.owner
                                .store(ControllerCommandState::ResetCompletion(ready));
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                    }
                }
                ControllerCommandPhase::ResetResponse => {
                    let ControllerCommandState::ResetResponse(pending) = self.owner.current()
                    else {
                        unreachable!("the selected Reset-response phase did not change")
                    };
                    if pending.wait_response_capacity(controller).await.is_err() {
                        return self.retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                    }
                    let ControllerCommandState::ResetResponse(pending) = self.owner.take() else {
                        unreachable!("the awaited Reset-response phase did not change")
                    };
                    match pending.try_publish(controller) {
                        DtmResetResponsePublication::Completed(complete) => {
                            self.store_transition(
                                ControllerCommandPhase::ResetResponse,
                                ControllerCommandStimulus::IdleRestored,
                                ControllerCommandState::Idle(complete.into_idle_command_task()),
                            );
                            return ControllerCommandBoundary::IdleRestored(
                                ControllerIdleCompletion::Reset,
                            );
                        }
                        DtmResetResponsePublication::Pending(pending) => self
                            .owner
                            .store(ControllerCommandState::ResetResponse(pending)),
                        DtmResetResponsePublication::EndpointMismatch(pending) => {
                            self.owner
                                .store(ControllerCommandState::ResetResponse(pending));
                            return self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch);
                        }
                        DtmResetResponsePublication::Fault { pending, error } => {
                            self.owner
                                .store(ControllerCommandState::ResetResponse(pending));
                            return self
                                .retain_boundary(ControllerCommandBoundary::HciFault(error));
                        }
                    }
                }
                ControllerCommandPhase::UnownedFinishedList => {
                    return self.retained_unowned_finished_list();
                }
            }
        }
    }
}
