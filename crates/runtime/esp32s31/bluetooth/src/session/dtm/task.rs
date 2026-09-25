//! Sole Embassy task owner for one active LE DTM session.
//!
//! Every hardware/HCI owner stays in [`DtmSessionTask`] while
//! readiness futures are pending. The futures borrow only durable wake state,
//! HCI capacity/intake and a caller-owned absolute Controller-time recheck.

#![forbid(unsafe_code)]

use core::future::Future;

#[cfg(target_arch = "riscv32")]
use crate::{
    notification::RuntimeNotifications,
    session::dtm::{
        DtmActiveCommandSignal, DtmActivePendingSignal, DtmActiveWait, DtmActiveWaitError,
        DtmShutdownWait, DtmTestEndResponseWait, DtmTestEndResponseWaitError,
    },
};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth::{
    controller::{ControllerIdleCommandTask, SchedulerRunInterruptStorage},
    le::dtm::{
        DtmActiveCommandIntake, DtmActiveCommandMismatch, DtmActiveControllerCommandRoute,
        DtmActiveResetBarrier, DtmActiveSessionFault, DtmActiveSessionRadioStep,
        DtmCommandReadySession, DtmOrderReady, DtmResponsePending, DtmResponsePendingSession,
        DtmResponsePublication, DtmStoppingFault, DtmStoppingRunner, DtmStoppingStep,
        DtmTestEndResponsePending, DtmTestEndResponsePublication, DtmTestEndRestoreFailure,
        DtmTestEndRestoreStep,
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    },
};

/// Factory for cooperative rechecks of an externally anchored absolute deadline.
///
/// The task asks for a fresh borrowed future whenever the core exposes a
/// Controller-time wait. Implementations must keep the deadline outside the
/// future so cancellation or another readiness edge cannot extend it.
pub trait DtmControllerTimeRecheck {
    type Recheck<'borrow>: Future<Output = ()> + 'borrow
    where
        Self: 'borrow;

    /// Whether another finite absolute recheck can still be constructed.
    fn status(&self) -> DtmControllerTimeRecheckStatus;

    fn wait_until_absolute_recheck(&mut self) -> Self::Recheck<'_>;
}

/// Availability of the next absolute Controller-time recheck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmControllerTimeRecheckStatus {
    /// One representable absolute deadline remains available.
    Scheduled,
    /// Advancing the absolute schedule exceeded the monotonic timeline.
    TimelineExhausted,
}

/// Phase retained after a recoverable transition asks the supervisor to retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSessionRetry {
    ActiveRadio,
    Stopping,
    IdleRestore,
}

#[cfg(target_arch = "riscv32")]
enum DtmSessionState<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    PendingResponse(DtmResponsePendingSession<'runtime, S, CAPACITY>),
    CommandReady(DtmCommandReadySession<'runtime, S, CAPACITY>),
    Stopping(DtmStoppingRunner<'runtime, S, CAPACITY>),
    TestEndResponse(DtmTestEndResponsePending<'runtime, S, CAPACITY>),
    Restore(DtmTestEndRestoreFailure<'runtime, S, CAPACITY>),
    UnownedPendingResponse {
        _session: DtmResponsePendingSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    UnownedCommandReady {
        _session: DtmCommandReadySession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    UnownedStopping {
        _runner: DtmStoppingRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> DtmSessionState<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    const fn phase(&self) -> DtmSessionPhase {
        match self {
            Self::PendingResponse(_) => DtmSessionPhase::PendingResponse,
            Self::CommandReady(_) => DtmSessionPhase::CommandReady,
            Self::Stopping(_) => DtmSessionPhase::Stopping,
            Self::TestEndResponse(_) => DtmSessionPhase::TestEndResponse,
            Self::Restore(_) => DtmSessionPhase::Restore,
            Self::UnownedPendingResponse { .. }
            | Self::UnownedCommandReady { .. }
            | Self::UnownedStopping { .. } => DtmSessionPhase::UnownedFinishedList,
        }
    }
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production reducer is executed only by the S31 target"
    )
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmSessionPhase {
    PendingResponse,
    CommandReady,
    Stopping,
    TestEndResponse,
    Restore,
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
enum DtmSessionStimulus {
    Continue,
    ResponsePublished,
    ControllerResponsePending,
    TestEnd,
    StoppingResponseReady,
    RestoreRequired,
    Completed,
    Retry,
    UnownedFinishedList,
    RetainedEndpointMismatch,
    RetainedFault,
    RetainedExternalFrame,
    ControllerTimeExhausted,
    ResetBarrier,
    TransferredControllerEndpointMismatch,
    TerminalFault,
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production reducer is executed only by the S31 target"
    )
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmSessionAction {
    Advance(DtmSessionPhase),
    RetainBoundary,
    TransferBoundary,
    TerminalBoundary,
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production reducer is executed only by the S31 target"
    )
)]
const fn reduce_dtm_session_transition(
    phase: DtmSessionPhase,
    stimulus: DtmSessionStimulus,
) -> DtmSessionAction {
    use DtmSessionAction::{Advance, RetainBoundary, TerminalBoundary, TransferBoundary};

    use DtmSessionPhase::{
        CommandReady, PendingResponse, Restore, Stopping, TestEndResponse,
        UnownedFinishedList as UnownedFinishedListPhase,
    };

    use DtmSessionStimulus::{
        Completed, Continue, ControllerResponsePending, ControllerTimeExhausted, ResetBarrier,
        RestoreRequired, RetainedEndpointMismatch, RetainedExternalFrame, RetainedFault, Retry,
        StoppingResponseReady, TerminalFault, TestEnd, TransferredControllerEndpointMismatch,
        UnownedFinishedList as UnownedFinishedListStimulus,
    };

    match (phase, stimulus) {
        (PendingResponse, DtmSessionStimulus::ResponsePublished) => Advance(CommandReady),
        (CommandReady, ControllerResponsePending) => Advance(PendingResponse),
        (CommandReady, TestEnd) => Advance(Stopping),
        (Stopping, StoppingResponseReady) => Advance(TestEndResponse),
        (TestEndResponse, RestoreRequired) => Advance(Restore),
        (TestEndResponse | Restore, Completed) => TerminalBoundary,
        (PendingResponse | CommandReady | Stopping | TestEndResponse, Continue) => Advance(phase),
        (PendingResponse | CommandReady | Stopping | Restore, Retry) => RetainBoundary,
        (PendingResponse | CommandReady | Stopping, UnownedFinishedListStimulus) => {
            Advance(UnownedFinishedListPhase)
        }
        (UnownedFinishedListPhase, UnownedFinishedListStimulus) => RetainBoundary,
        (PendingResponse | CommandReady | Stopping, ControllerTimeExhausted) => RetainBoundary,
        (PendingResponse | CommandReady | TestEndResponse, RetainedEndpointMismatch) => {
            RetainBoundary
        }
        (PendingResponse | CommandReady | TestEndResponse, RetainedFault) => RetainBoundary,
        (CommandReady, RetainedExternalFrame) => RetainBoundary,
        (CommandReady, ResetBarrier | TransferredControllerEndpointMismatch) => TransferBoundary,
        (PendingResponse | CommandReady | Stopping, TerminalFault) => TerminalBoundary,
        _ => panic!("invalid DTM task transition"),
    }
}

/// One lossless boundary returned by [`DtmSessionTask::run`].
///
/// Non-terminal observations leave the complete active owner inside the task.
/// Command-policy transfers and terminal fault/completion variants return the
/// relevant lower owner and leave the task empty, preventing accidental reuse.
#[cfg(target_arch = "riscv32")]
#[must_use = "route the retained observation or terminal owner before running again"]
pub enum DtmSessionBoundary<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// No installed role owns this scheduler list; the exact owner remains quarantined here.
    UnownedFinishedList(BluetoothSchedulerHardwareListIndex),
    /// An accepted Reset remains paired opaquely with its active radio/order owner.
    ResetBarrier(DtmActiveResetBarrier<'runtime, S, CAPACITY>),
    /// A non-command Host frame remains bound to its source HCI epoch and buffer.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
    /// Defensive fail-stop owner for an impossible post-intake endpoint mismatch.
    ControllerCommandEndpointMismatch(DtmActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>),
    /// The supplied endpoint does not match the stored response/order epoch.
    EndpointMismatch,
    /// The HCI operation failed while the complete session stayed stored.
    HciFault(HciChannelError),
    /// A finite lower transition retained its owner for an explicit retry.
    Retryable(DtmSessionRetry),
    /// The absolute Controller-time schedule is exhausted; the complete session stays stored.
    ControllerTimeExhausted,
    /// Active radio progress failed closed with both axes retained in the fault.
    PendingRadioFault(DtmActiveSessionFault<'runtime, S, CAPACITY, DtmResponsePending<'runtime>>),
    /// Active radio progress failed closed after command-order publication.
    CommandReadyRadioFault(DtmActiveSessionFault<'runtime, S, CAPACITY, DtmOrderReady<'runtime>>),
    /// Test End quiescence failed closed with its exact graph and command.
    StoppingFault(DtmStoppingFault<'runtime, S, CAPACITY>),
    /// Test End completed and returned the sole opaque idle command task.
    Complete(ControllerIdleCommandTask<'runtime, S, CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
type SessionBoundary<'runtime, 'epoch, 'packet, S, const CAPACITY: usize> =
    DtmSessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>;

#[cfg(target_arch = "riscv32")]
struct CommandPacketBuffer<'packet>(&'packet mut [u8]);

/// Option-backed affine owner slot shared by production transitions and tests.
///
/// Await sites only call [`Self::current`]. Consuming lower transitions call
/// [`Self::take`] only after readiness has completed and immediately restore a
/// successor through [`Self::store`] before another await or observation.
#[cfg(any(target_arch = "riscv32", test))]
struct SessionOwnerSlot<State> {
    state: Option<State>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<State> SessionOwnerSlot<State> {
    const fn new(state: State) -> Self {
        Self { state: Some(state) }
    }

    fn current(&self) -> &State {
        self.state
            .as_ref()
            .expect("a retained DTM task owns one affine state")
    }

    fn take(&mut self) -> State {
        self.state
            .take()
            .expect("a DTM transition consumes its retained state exactly once")
    }

    fn store(&mut self, state: State) {
        assert!(
            self.state.replace(state).is_none(),
            "a DTM transition cannot overwrite an affine owner"
        );
    }

    fn retain<Observation>(&mut self, state: State, observation: Observation) -> Observation {
        self.store(state);
        observation
    }

    const fn is_empty(&self) -> bool {
        self.state.is_none()
    }
}

/// Sole executor-side owner of an active DTM radio/order transaction.
#[cfg(target_arch = "riscv32")]
#[must_use = "keep the task alive until it returns a terminal completion or fault owner"]
pub struct DtmSessionTask<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    owner: SessionOwnerSlot<DtmSessionState<'runtime, S, CAPACITY>>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize> DtmSessionTask<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Begin executor ownership after the first radio `RUN` created both axes.
    pub const fn new(session: DtmResponsePendingSession<'runtime, S, CAPACITY>) -> Self {
        Self {
            owner: SessionOwnerSlot::new(DtmSessionState::PendingResponse(session)),
        }
    }

    /// Resume executor ownership after an outer command policy returns the
    /// unchanged active radio/order session.
    pub const fn from_command_ready(
        session: DtmCommandReadySession<'runtime, S, CAPACITY>,
    ) -> Self {
        Self {
            owner: SessionOwnerSlot::new(DtmSessionState::CommandReady(session)),
        }
    }

    /// Whether completion, failure, or external policy transferred ownership
    /// out of this task. An unowned-list quarantine intentionally remains non-empty.
    pub const fn is_empty(&self) -> bool {
        self.owner.is_empty()
    }

    fn phase(&self) -> DtmSessionPhase {
        self.owner.current().phase()
    }

    fn store_transition(
        &mut self,
        from: DtmSessionPhase,
        stimulus: DtmSessionStimulus,
        state: DtmSessionState<'runtime, S, CAPACITY>,
    ) {
        match reduce_dtm_session_transition(from, stimulus) {
            DtmSessionAction::Advance(expected) if state.phase() == expected => {
                self.owner.store(state);
            }
            _ => unreachable!("the DTM reducer rejected a stored successor"),
        }
    }

    fn retain_transition<Boundary>(
        &mut self,
        from: DtmSessionPhase,
        stimulus: DtmSessionStimulus,
        state: DtmSessionState<'runtime, S, CAPACITY>,
        boundary: Boundary,
    ) -> Boundary {
        match reduce_dtm_session_transition(from, stimulus) {
            DtmSessionAction::RetainBoundary if state.phase() == from => {
                self.owner.retain(state, boundary)
            }
            _ => unreachable!("the DTM reducer rejected a retained boundary"),
        }
    }

    fn retain_existing_transition<Boundary>(
        &self,
        stimulus: DtmSessionStimulus,
        boundary: Boundary,
    ) -> Boundary {
        match reduce_dtm_session_transition(self.phase(), stimulus) {
            DtmSessionAction::RetainBoundary => boundary,
            _ => unreachable!("the DTM reducer rejected an observed boundary"),
        }
    }

    fn transfer_transition<Boundary>(
        from: DtmSessionPhase,
        stimulus: DtmSessionStimulus,
        boundary: Boundary,
    ) -> Boundary {
        match reduce_dtm_session_transition(from, stimulus) {
            DtmSessionAction::TransferBoundary | DtmSessionAction::TerminalBoundary => boundary,
            _ => unreachable!("the DTM reducer rejected an ownership transfer"),
        }
    }

    fn step_pending_radio<'epoch, 'packet>(
        &mut self,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::PendingResponse(session) = self.owner.take() else {
            unreachable!("the selected pending state did not change")
        };
        match session.step_radio() {
            DtmActiveSessionRadioStep::Continue(session)
            | DtmActiveSessionRadioStep::Waiting(session) => {
                self.store_transition(
                    DtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::Continue,
                    DtmSessionState::PendingResponse(session),
                );
                None
            }
            DtmActiveSessionRadioStep::UnrelatedList { session, observed } => {
                let index = observed.index();
                self.store_transition(
                    DtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::UnownedFinishedList,
                    DtmSessionState::UnownedPendingResponse {
                        _session: session,
                        observed,
                    },
                );
                Some(DtmSessionBoundary::UnownedFinishedList(index))
            }
            DtmActiveSessionRadioStep::Retryable(session) => Some(self.retain_transition(
                DtmSessionPhase::PendingResponse,
                DtmSessionStimulus::Retry,
                DtmSessionState::PendingResponse(session),
                DtmSessionBoundary::Retryable(DtmSessionRetry::ActiveRadio),
            )),
            DtmActiveSessionRadioStep::Fault(fault) => Some(Self::transfer_transition(
                DtmSessionPhase::PendingResponse,
                DtmSessionStimulus::TerminalFault,
                DtmSessionBoundary::PendingRadioFault(fault),
            )),
        }
    }

    fn step_command_ready_radio<'epoch, 'packet>(
        &mut self,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::CommandReady(session) = self.owner.take() else {
            unreachable!("the selected command-ready state did not change")
        };
        match session.step_radio() {
            DtmActiveSessionRadioStep::Continue(session)
            | DtmActiveSessionRadioStep::Waiting(session) => {
                self.store_transition(
                    DtmSessionPhase::CommandReady,
                    DtmSessionStimulus::Continue,
                    DtmSessionState::CommandReady(session),
                );
                None
            }
            DtmActiveSessionRadioStep::UnrelatedList { session, observed } => {
                let index = observed.index();
                self.store_transition(
                    DtmSessionPhase::CommandReady,
                    DtmSessionStimulus::UnownedFinishedList,
                    DtmSessionState::UnownedCommandReady {
                        _session: session,
                        observed,
                    },
                );
                Some(DtmSessionBoundary::UnownedFinishedList(index))
            }
            DtmActiveSessionRadioStep::Retryable(session) => Some(self.retain_transition(
                DtmSessionPhase::CommandReady,
                DtmSessionStimulus::Retry,
                DtmSessionState::CommandReady(session),
                DtmSessionBoundary::Retryable(DtmSessionRetry::ActiveRadio),
            )),
            DtmActiveSessionRadioStep::Fault(fault) => Some(Self::transfer_transition(
                DtmSessionPhase::CommandReady,
                DtmSessionStimulus::TerminalFault,
                DtmSessionBoundary::CommandReadyRadioFault(fault),
            )),
        }
    }

    fn step_stopping<'epoch, 'packet>(
        &mut self,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::Stopping(runner) = self.owner.take() else {
            unreachable!("the selected stopping state did not change")
        };
        match runner.step() {
            DtmStoppingStep::Continue(runner) | DtmStoppingStep::Waiting(runner) => {
                self.store_transition(
                    DtmSessionPhase::Stopping,
                    DtmSessionStimulus::Continue,
                    DtmSessionState::Stopping(runner),
                );
                None
            }
            DtmStoppingStep::UnrelatedList { runner, observed } => {
                let index = observed.index();
                self.store_transition(
                    DtmSessionPhase::Stopping,
                    DtmSessionStimulus::UnownedFinishedList,
                    DtmSessionState::UnownedStopping {
                        _runner: runner,
                        observed,
                    },
                );
                Some(DtmSessionBoundary::UnownedFinishedList(index))
            }
            DtmStoppingStep::Retryable(runner) => Some(self.retain_transition(
                DtmSessionPhase::Stopping,
                DtmSessionStimulus::Retry,
                DtmSessionState::Stopping(runner),
                DtmSessionBoundary::Retryable(DtmSessionRetry::Stopping),
            )),
            DtmStoppingStep::ResponseReady(ready) => {
                self.store_transition(
                    DtmSessionPhase::Stopping,
                    DtmSessionStimulus::StoppingResponseReady,
                    DtmSessionState::TestEndResponse(ready.into_response_pending()),
                );
                None
            }
            DtmStoppingStep::Fault(fault) => Some(Self::transfer_transition(
                DtmSessionPhase::Stopping,
                DtmSessionStimulus::TerminalFault,
                DtmSessionBoundary::StoppingFault(fault),
            )),
        }
    }

    fn route_controller_command<'epoch, 'packet>(
        &mut self,
        route: DtmActiveControllerCommandRoute<'runtime, 'epoch, S, CAPACITY>,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match route {
            DtmActiveControllerCommandRoute::ResponsePending(session) => self.store_transition(
                DtmSessionPhase::CommandReady,
                DtmSessionStimulus::ControllerResponsePending,
                DtmSessionState::PendingResponse(session),
            ),
            DtmActiveControllerCommandRoute::TestEnd(runner) => self.store_transition(
                DtmSessionPhase::CommandReady,
                DtmSessionStimulus::TestEnd,
                DtmSessionState::Stopping(runner),
            ),
            DtmActiveControllerCommandRoute::ResetBarrier(barrier) => {
                return Some(Self::transfer_transition(
                    DtmSessionPhase::CommandReady,
                    DtmSessionStimulus::ResetBarrier,
                    DtmSessionBoundary::ResetBarrier(barrier),
                ));
            }
            DtmActiveControllerCommandRoute::EndpointMismatch(mismatch) => {
                return Some(Self::transfer_transition(
                    DtmSessionPhase::CommandReady,
                    DtmSessionStimulus::TransferredControllerEndpointMismatch,
                    DtmSessionBoundary::ControllerCommandEndpointMismatch(mismatch),
                ));
            }
        }
        None
    }

    fn try_publish_pending_response<
        'epoch,
        'packet,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &mut self,
        controller: &LeControllerCommandEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::PendingResponse(session) = self.owner.take() else {
            unreachable!("the awaited pending state did not change")
        };
        match session.try_publish_response(controller) {
            DtmResponsePublication::Published(session) => self.store_transition(
                DtmSessionPhase::PendingResponse,
                DtmSessionStimulus::ResponsePublished,
                DtmSessionState::CommandReady(session),
            ),
            DtmResponsePublication::Pending(session) => self.store_transition(
                DtmSessionPhase::PendingResponse,
                DtmSessionStimulus::Continue,
                DtmSessionState::PendingResponse(session),
            ),
            DtmResponsePublication::EndpointMismatch(session) => {
                return Some(self.retain_transition(
                    DtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::RetainedEndpointMismatch,
                    DtmSessionState::PendingResponse(session),
                    DtmSessionBoundary::EndpointMismatch,
                ));
            }
            DtmResponsePublication::Fault { session, error } => {
                return Some(self.retain_transition(
                    DtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::RetainedFault,
                    DtmSessionState::PendingResponse(session),
                    DtmSessionBoundary::HciFault(error),
                ));
            }
        }
        None
    }

    async fn drive_pending_response<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        Recheck: DtmControllerTimeRecheck,
    >(
        &mut self,
        wakers: &RuntimeNotifications<WakeMutex>,
        controller: &LeControllerCommandEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        recheck: &mut Recheck,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::PendingResponse(session) = self.owner.current() else {
            unreachable!("the selected pending phase did not change")
        };
        let Some(wait) = DtmActiveWait::from_waiting(session, wakers) else {
            return self.step_pending_radio();
        };
        if recheck.status() == DtmControllerTimeRecheckStatus::TimelineExhausted {
            return Some(self.retain_existing_transition(
                DtmSessionStimulus::ControllerTimeExhausted,
                DtmSessionBoundary::ControllerTimeExhausted,
            ));
        }
        match wait
            .wait_next(controller, recheck.wait_until_absolute_recheck())
            .await
        {
            Ok(DtmActivePendingSignal::Radio(_)) => self.step_pending_radio(),
            Ok(DtmActivePendingSignal::ResponseCapacity) => {
                self.try_publish_pending_response(controller)
            }
            Err(DtmActiveWaitError::EndpointMismatch) => Some(self.retain_existing_transition(
                DtmSessionStimulus::RetainedEndpointMismatch,
                DtmSessionBoundary::EndpointMismatch,
            )),
        }
    }

    async fn drive_command_ready<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        Recheck: DtmControllerTimeRecheck,
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
        packet: &mut Option<CommandPacketBuffer<'packet>>,
        recheck: &mut Recheck,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        loop {
            let DtmSessionState::CommandReady(session) = self.owner.current() else {
                unreachable!("the selected command-ready phase did not change")
            };
            let Some(wait) = DtmActiveWait::from_waiting(session, wakers) else {
                if let Some(boundary) = self.step_command_ready_radio() {
                    return Some(boundary);
                }
                continue;
            };
            if recheck.status() == DtmControllerTimeRecheckStatus::TimelineExhausted {
                return Some(self.retain_existing_transition(
                    DtmSessionStimulus::ControllerTimeExhausted,
                    DtmSessionBoundary::ControllerTimeExhausted,
                ));
            }
            match wait
                .wait_next(controller, recheck.wait_until_absolute_recheck())
                .await
            {
                Ok(DtmActiveCommandSignal::Radio(_)) => {
                    if let Some(boundary) = self.step_command_ready_radio() {
                        return Some(boundary);
                    }
                }
                Ok(DtmActiveCommandSignal::HostReady) => {
                    let DtmSessionState::CommandReady(session) = self.owner.take() else {
                        unreachable!("the awaited command-ready state did not change")
                    };
                    let CommandPacketBuffer(buffer) = packet
                        .take()
                        .expect("the active task retains its sole HCI receive buffer");
                    match session
                        .try_route_active_controller_command_with_buffer(controller, buffer)
                    {
                        DtmActiveCommandIntake::Routed { route, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            if let Some(boundary) = self.route_controller_command(route) {
                                return Some(boundary);
                            }
                            return None;
                        }
                        DtmActiveCommandIntake::Empty { session, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            self.store_transition(
                                DtmSessionPhase::CommandReady,
                                DtmSessionStimulus::Continue,
                                DtmSessionState::CommandReady(session),
                            );
                        }
                        DtmActiveCommandIntake::EndpointMismatch { session, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            return Some(self.retain_transition(
                                DtmSessionPhase::CommandReady,
                                DtmSessionStimulus::RetainedEndpointMismatch,
                                DtmSessionState::CommandReady(session),
                                DtmSessionBoundary::EndpointMismatch,
                            ));
                        }
                        DtmActiveCommandIntake::Channel {
                            session,
                            buffer,
                            error,
                        } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            return Some(self.retain_transition(
                                DtmSessionPhase::CommandReady,
                                DtmSessionStimulus::RetainedFault,
                                DtmSessionState::CommandReady(session),
                                DtmSessionBoundary::HciFault(error),
                            ));
                        }
                        DtmActiveCommandIntake::NonCommand { session, frame } => {
                            return Some(self.retain_transition(
                                DtmSessionPhase::CommandReady,
                                DtmSessionStimulus::RetainedExternalFrame,
                                DtmSessionState::CommandReady(session),
                                DtmSessionBoundary::NonCommand(frame),
                            ));
                        }
                    }
                }
                Err(DtmActiveWaitError::EndpointMismatch) => {
                    return Some(self.retain_existing_transition(
                        DtmSessionStimulus::RetainedEndpointMismatch,
                        DtmSessionBoundary::EndpointMismatch,
                    ));
                }
            }
        }
    }

    async fn drive_stopping<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        Recheck: DtmControllerTimeRecheck,
    >(
        &mut self,
        wakers: &RuntimeNotifications<WakeMutex>,
        recheck: &mut Recheck,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::Stopping(runner) = self.owner.current() else {
            unreachable!("the selected stopping phase did not change")
        };
        if let Some(wait) = DtmShutdownWait::from_waiting(runner, wakers) {
            if recheck.status() == DtmControllerTimeRecheckStatus::TimelineExhausted {
                return Some(self.retain_existing_transition(
                    DtmSessionStimulus::ControllerTimeExhausted,
                    DtmSessionBoundary::ControllerTimeExhausted,
                ));
            }
            wait.wait_next(recheck.wait_until_absolute_recheck()).await;
        }
        self.step_stopping()
    }

    async fn drive_test_end_response<
        'epoch,
        'packet,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &mut self,
        controller: &LeControllerCommandEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let DtmSessionState::TestEndResponse(pending) = self.owner.current() else {
            unreachable!("the selected Test End response phase did not change")
        };
        if let Err(DtmTestEndResponseWaitError::EndpointMismatch) =
            DtmTestEndResponseWait::new(pending)
                .wait_next(controller)
                .await
        {
            return Some(self.retain_existing_transition(
                DtmSessionStimulus::RetainedEndpointMismatch,
                DtmSessionBoundary::EndpointMismatch,
            ));
        }

        let DtmSessionState::TestEndResponse(pending) = self.owner.take() else {
            unreachable!("the awaited Test End response state did not change")
        };
        match pending.try_publish(controller) {
            DtmTestEndResponsePublication::Completed(complete) => Some(Self::transfer_transition(
                DtmSessionPhase::TestEndResponse,
                DtmSessionStimulus::Completed,
                DtmSessionBoundary::Complete(complete.into_idle_command_task()),
            )),
            DtmTestEndResponsePublication::Pending(pending) => {
                self.store_transition(
                    DtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::Continue,
                    DtmSessionState::TestEndResponse(pending),
                );
                None
            }
            DtmTestEndResponsePublication::EndpointMismatch(pending) => {
                Some(self.retain_transition(
                    DtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::RetainedEndpointMismatch,
                    DtmSessionState::TestEndResponse(pending),
                    DtmSessionBoundary::EndpointMismatch,
                ))
            }
            DtmTestEndResponsePublication::Fault { pending, error } => {
                Some(self.retain_transition(
                    DtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::RetainedFault,
                    DtmSessionState::TestEndResponse(pending),
                    DtmSessionBoundary::HciFault(error),
                ))
            }
            DtmTestEndResponsePublication::RestoreFailed(failure) => {
                self.store_transition(
                    DtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::RestoreRequired,
                    DtmSessionState::Restore(failure),
                );
                None
            }
        }
    }

    fn retry_restore<'epoch, 'packet>(
        &mut self,
    ) -> SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let DtmSessionState::Restore(failure) = self.owner.take() else {
            unreachable!("the selected restore state did not change")
        };
        match failure.retry_restore() {
            DtmTestEndRestoreStep::Completed(complete) => Self::transfer_transition(
                DtmSessionPhase::Restore,
                DtmSessionStimulus::Completed,
                DtmSessionBoundary::Complete(complete.into_idle_command_task()),
            ),
            DtmTestEndRestoreStep::Rejected(failure) => self.retain_transition(
                DtmSessionPhase::Restore,
                DtmSessionStimulus::Retry,
                DtmSessionState::Restore(failure),
                DtmSessionBoundary::Retryable(DtmSessionRetry::IdleRestore),
            ),
        }
    }

    fn unowned_finished_list_boundary<'epoch, 'packet>(
        &self,
    ) -> SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let index = match self.owner.current() {
            DtmSessionState::UnownedPendingResponse { observed, .. }
            | DtmSessionState::UnownedCommandReady { observed, .. }
            | DtmSessionState::UnownedStopping { observed, .. } => observed.index(),
            _ => unreachable!("the selected unowned-list quarantine did not change"),
        };
        self.retain_existing_transition(
            DtmSessionStimulus::UnownedFinishedList,
            DtmSessionBoundary::UnownedFinishedList(index),
        )
    }

    /// Run until an externally meaningful lossless boundary.
    ///
    /// The packet buffer belongs to the caller because a returned non-command
    /// frame borrows it. `recheck` owns the absolute Controller-time anchor and
    /// may create as many cancellation-safe borrowed waits as this run needs.
    pub async fn run<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        Recheck: DtmControllerTimeRecheck,
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
    ) -> DtmSessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let mut packet = Some(CommandPacketBuffer(packet));
        loop {
            let boundary = match self.phase() {
                DtmSessionPhase::PendingResponse => {
                    self.drive_pending_response(wakers, controller, recheck)
                        .await
                }
                DtmSessionPhase::CommandReady => {
                    self.drive_command_ready(wakers, controller, &mut packet, recheck)
                        .await
                }
                DtmSessionPhase::Stopping => self.drive_stopping(wakers, recheck).await,
                DtmSessionPhase::TestEndResponse => self.drive_test_end_response(controller).await,
                DtmSessionPhase::Restore => return self.retry_restore(),
                DtmSessionPhase::UnownedFinishedList => {
                    return self.unowned_finished_list_boundary();
                }
            };
            if let Some(boundary) = boundary {
                return boundary;
            }
        }
    }
}

#[cfg(test)]
mod tests;
