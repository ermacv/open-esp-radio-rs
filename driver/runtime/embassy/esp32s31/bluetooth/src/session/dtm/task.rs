//! Sole Embassy task owner for one active LE DTM session.
//!
//! Every hardware/HCI owner stays in [`EmbassyBluetoothDtmSessionTask`] while
//! readiness futures are pending. The futures borrow only durable wake state,
//! HCI capacity/intake and a caller-owned absolute Controller-time recheck.

#![forbid(unsafe_code)]

use core::future::Future;

#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::{HciChannelError, HciEpochBound, HostToControllerFrame};

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerIdleCommandTask, BluetoothDtmActiveCommandIntake,
    BluetoothDtmActiveCommandMismatch, BluetoothDtmActiveControllerCommandRoute,
    BluetoothDtmActiveResetBarrier, BluetoothDtmActiveSessionFault,
    BluetoothDtmActiveSessionRadioStep, BluetoothDtmCommandReadySession, BluetoothDtmOrderReady,
    BluetoothDtmResponsePending, BluetoothDtmResponsePendingSession,
    BluetoothDtmResponsePublication, BluetoothDtmStoppingFault, BluetoothDtmStoppingRunner,
    BluetoothDtmStoppingStep, BluetoothDtmTestEndResponsePending,
    BluetoothDtmTestEndResponsePublication, BluetoothDtmTestEndRestoreFailure,
    BluetoothDtmTestEndRestoreStep, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::{
    EmbassyBluetoothDtmActiveCommandSignal, EmbassyBluetoothDtmActivePendingSignal,
    EmbassyBluetoothDtmActiveWait, EmbassyBluetoothDtmActiveWaitError,
    EmbassyBluetoothDtmStoppingWait, EmbassyBluetoothDtmTestEndResponseWait,
    EmbassyBluetoothDtmTestEndResponseWaitError, EmbassyBluetoothRuntimeWakers,
};

/// Factory for cooperative rechecks of an externally anchored absolute deadline.
///
/// The task asks for a fresh borrowed future whenever the core exposes a
/// Controller-time wait. Implementations must keep the deadline outside the
/// future so cancellation or another readiness edge cannot extend it.
pub trait EmbassyBluetoothDtmControllerTimeRecheck {
    type Recheck<'borrow>: Future<Output = ()> + 'borrow
    where
        Self: 'borrow;

    /// Whether another finite absolute recheck can still be constructed.
    fn status(&self) -> EmbassyBluetoothDtmControllerTimeRecheckStatus;

    fn wait_until_absolute_recheck(&mut self) -> Self::Recheck<'_>;
}

/// Availability of the next absolute Controller-time recheck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmControllerTimeRecheckStatus {
    /// One representable absolute deadline remains available.
    Scheduled,
    /// Advancing the absolute schedule exceeded the monotonic timeline.
    TimelineExhausted,
}

/// Phase retained after a recoverable transition asks the supervisor to retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmSessionRetry {
    ActiveRadio,
    Stopping,
    IdleRestore,
}

#[cfg(target_arch = "riscv32")]
enum EmbassyBluetoothDtmSessionState<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    PendingResponse(BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>),
    CommandReady(BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>),
    Stopping(BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>),
    TestEndResponse(BluetoothDtmTestEndResponsePending<'runtime, S, CAPACITY>),
    Restore(BluetoothDtmTestEndRestoreFailure<'runtime, S, CAPACITY>),
    UnownedPendingResponse {
        _session: BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    UnownedCommandReady {
        _session: BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    UnownedStopping {
        _runner: BluetoothDtmStoppingRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

#[cfg(target_arch = "riscv32")]
impl<S, const CAPACITY: usize> EmbassyBluetoothDtmSessionState<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    const fn phase(&self) -> EmbassyBluetoothDtmSessionPhase {
        match self {
            Self::PendingResponse(_) => EmbassyBluetoothDtmSessionPhase::PendingResponse,
            Self::CommandReady(_) => EmbassyBluetoothDtmSessionPhase::CommandReady,
            Self::Stopping(_) => EmbassyBluetoothDtmSessionPhase::Stopping,
            Self::TestEndResponse(_) => EmbassyBluetoothDtmSessionPhase::TestEndResponse,
            Self::Restore(_) => EmbassyBluetoothDtmSessionPhase::Restore,
            Self::UnownedPendingResponse { .. }
            | Self::UnownedCommandReady { .. }
            | Self::UnownedStopping { .. } => EmbassyBluetoothDtmSessionPhase::UnownedFinishedList,
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
enum EmbassyBluetoothDtmSessionPhase {
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
    Advance(EmbassyBluetoothDtmSessionPhase),
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
    phase: EmbassyBluetoothDtmSessionPhase,
    stimulus: DtmSessionStimulus,
) -> DtmSessionAction {
    use DtmSessionAction::{Advance, RetainBoundary, TerminalBoundary, TransferBoundary};
    use DtmSessionStimulus::{
        Completed, Continue, ControllerResponsePending, ControllerTimeExhausted, ResetBarrier,
        RestoreRequired, RetainedEndpointMismatch, RetainedExternalFrame, RetainedFault, Retry,
        StoppingResponseReady, TerminalFault, TestEnd, TransferredControllerEndpointMismatch,
        UnownedFinishedList as UnownedFinishedListStimulus,
    };
    use EmbassyBluetoothDtmSessionPhase::{
        CommandReady, PendingResponse, Restore, Stopping, TestEndResponse,
        UnownedFinishedList as UnownedFinishedListPhase,
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

/// One lossless boundary returned by [`EmbassyBluetoothDtmSessionTask::run`].
///
/// Non-terminal observations leave the complete active owner inside the task.
/// Command-policy transfers and terminal fault/completion variants return the
/// relevant lower owner and leave the task empty, preventing accidental reuse.
#[cfg(target_arch = "riscv32")]
#[must_use = "route the retained observation or terminal owner before running again"]
pub enum EmbassyBluetoothDtmSessionBoundary<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// No installed role owns this scheduler list; the exact owner remains quarantined here.
    UnownedFinishedList(BluetoothSchedulerHardwareListIndex),
    /// An accepted Reset remains paired opaquely with its active radio/order owner.
    ResetBarrier(BluetoothDtmActiveResetBarrier<'runtime, S, CAPACITY>),
    /// A non-command Host frame remains bound to its source HCI epoch and buffer.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
    /// Defensive fail-stop owner for an impossible post-intake endpoint mismatch.
    ControllerCommandEndpointMismatch(
        BluetoothDtmActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// The supplied endpoint does not match the stored response/order epoch.
    EndpointMismatch,
    /// The HCI operation failed while the complete session stayed stored.
    HciFault(HciChannelError),
    /// A finite lower transition retained its owner for an explicit retry.
    Retryable(EmbassyBluetoothDtmSessionRetry),
    /// The absolute Controller-time schedule is exhausted; the complete session stays stored.
    ControllerTimeExhausted,
    /// Active radio progress failed closed with both axes retained in the fault.
    PendingRadioFault(
        BluetoothDtmActiveSessionFault<
            'runtime,
            S,
            CAPACITY,
            BluetoothDtmResponsePending<'runtime>,
        >,
    ),
    /// Active radio progress failed closed after command-order publication.
    CommandReadyRadioFault(
        BluetoothDtmActiveSessionFault<'runtime, S, CAPACITY, BluetoothDtmOrderReady<'runtime>>,
    ),
    /// Test End quiescence failed closed with its exact graph and command.
    StoppingFault(BluetoothDtmStoppingFault<'runtime, S, CAPACITY>),
    /// Test End completed and returned the sole opaque idle command task.
    Complete(BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
type SessionBoundary<'runtime, 'epoch, 'packet, S, const CAPACITY: usize> =
    EmbassyBluetoothDtmSessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>;

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
pub struct EmbassyBluetoothDtmSessionTask<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    owner: SessionOwnerSlot<EmbassyBluetoothDtmSessionState<'runtime, S, CAPACITY>>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize> EmbassyBluetoothDtmSessionTask<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Begin executor ownership after the first radio `RUN` created both axes.
    pub const fn new(session: BluetoothDtmResponsePendingSession<'runtime, S, CAPACITY>) -> Self {
        Self {
            owner: SessionOwnerSlot::new(EmbassyBluetoothDtmSessionState::PendingResponse(session)),
        }
    }

    /// Resume executor ownership after an outer command policy returns the
    /// unchanged active radio/order session.
    pub const fn from_command_ready(
        session: BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>,
    ) -> Self {
        Self {
            owner: SessionOwnerSlot::new(EmbassyBluetoothDtmSessionState::CommandReady(session)),
        }
    }

    /// Whether completion, failure, or external policy transferred ownership
    /// out of this task. An unowned-list quarantine intentionally remains non-empty.
    pub const fn is_empty(&self) -> bool {
        self.owner.is_empty()
    }

    fn phase(&self) -> EmbassyBluetoothDtmSessionPhase {
        self.owner.current().phase()
    }

    fn store_transition(
        &mut self,
        from: EmbassyBluetoothDtmSessionPhase,
        stimulus: DtmSessionStimulus,
        state: EmbassyBluetoothDtmSessionState<'runtime, S, CAPACITY>,
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
        from: EmbassyBluetoothDtmSessionPhase,
        stimulus: DtmSessionStimulus,
        state: EmbassyBluetoothDtmSessionState<'runtime, S, CAPACITY>,
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
        from: EmbassyBluetoothDtmSessionPhase,
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
        let EmbassyBluetoothDtmSessionState::PendingResponse(session) = self.owner.take() else {
            unreachable!("the selected pending state did not change")
        };
        match session.step_radio() {
            BluetoothDtmActiveSessionRadioStep::Continue(session)
            | BluetoothDtmActiveSessionRadioStep::Waiting(session) => {
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::Continue,
                    EmbassyBluetoothDtmSessionState::PendingResponse(session),
                );
                None
            }
            BluetoothDtmActiveSessionRadioStep::UnrelatedList { session, observed } => {
                let index = observed.index();
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::UnownedFinishedList,
                    EmbassyBluetoothDtmSessionState::UnownedPendingResponse {
                        _session: session,
                        observed,
                    },
                );
                Some(EmbassyBluetoothDtmSessionBoundary::UnownedFinishedList(
                    index,
                ))
            }
            BluetoothDtmActiveSessionRadioStep::Retryable(session) => Some(self.retain_transition(
                EmbassyBluetoothDtmSessionPhase::PendingResponse,
                DtmSessionStimulus::Retry,
                EmbassyBluetoothDtmSessionState::PendingResponse(session),
                EmbassyBluetoothDtmSessionBoundary::Retryable(
                    EmbassyBluetoothDtmSessionRetry::ActiveRadio,
                ),
            )),
            BluetoothDtmActiveSessionRadioStep::Fault(fault) => Some(Self::transfer_transition(
                EmbassyBluetoothDtmSessionPhase::PendingResponse,
                DtmSessionStimulus::TerminalFault,
                EmbassyBluetoothDtmSessionBoundary::PendingRadioFault(fault),
            )),
        }
    }

    fn step_command_ready_radio<'epoch, 'packet>(
        &mut self,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let EmbassyBluetoothDtmSessionState::CommandReady(session) = self.owner.take() else {
            unreachable!("the selected command-ready state did not change")
        };
        match session.step_radio() {
            BluetoothDtmActiveSessionRadioStep::Continue(session)
            | BluetoothDtmActiveSessionRadioStep::Waiting(session) => {
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::Continue,
                    EmbassyBluetoothDtmSessionState::CommandReady(session),
                );
                None
            }
            BluetoothDtmActiveSessionRadioStep::UnrelatedList { session, observed } => {
                let index = observed.index();
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::UnownedFinishedList,
                    EmbassyBluetoothDtmSessionState::UnownedCommandReady {
                        _session: session,
                        observed,
                    },
                );
                Some(EmbassyBluetoothDtmSessionBoundary::UnownedFinishedList(
                    index,
                ))
            }
            BluetoothDtmActiveSessionRadioStep::Retryable(session) => Some(self.retain_transition(
                EmbassyBluetoothDtmSessionPhase::CommandReady,
                DtmSessionStimulus::Retry,
                EmbassyBluetoothDtmSessionState::CommandReady(session),
                EmbassyBluetoothDtmSessionBoundary::Retryable(
                    EmbassyBluetoothDtmSessionRetry::ActiveRadio,
                ),
            )),
            BluetoothDtmActiveSessionRadioStep::Fault(fault) => Some(Self::transfer_transition(
                EmbassyBluetoothDtmSessionPhase::CommandReady,
                DtmSessionStimulus::TerminalFault,
                EmbassyBluetoothDtmSessionBoundary::CommandReadyRadioFault(fault),
            )),
        }
    }

    fn step_stopping<'epoch, 'packet>(
        &mut self,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let EmbassyBluetoothDtmSessionState::Stopping(runner) = self.owner.take() else {
            unreachable!("the selected stopping state did not change")
        };
        match runner.step() {
            BluetoothDtmStoppingStep::Continue(runner)
            | BluetoothDtmStoppingStep::Waiting(runner) => {
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::Stopping,
                    DtmSessionStimulus::Continue,
                    EmbassyBluetoothDtmSessionState::Stopping(runner),
                );
                None
            }
            BluetoothDtmStoppingStep::UnrelatedList { runner, observed } => {
                let index = observed.index();
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::Stopping,
                    DtmSessionStimulus::UnownedFinishedList,
                    EmbassyBluetoothDtmSessionState::UnownedStopping {
                        _runner: runner,
                        observed,
                    },
                );
                Some(EmbassyBluetoothDtmSessionBoundary::UnownedFinishedList(
                    index,
                ))
            }
            BluetoothDtmStoppingStep::Retryable(runner) => Some(self.retain_transition(
                EmbassyBluetoothDtmSessionPhase::Stopping,
                DtmSessionStimulus::Retry,
                EmbassyBluetoothDtmSessionState::Stopping(runner),
                EmbassyBluetoothDtmSessionBoundary::Retryable(
                    EmbassyBluetoothDtmSessionRetry::Stopping,
                ),
            )),
            BluetoothDtmStoppingStep::ResponseReady(ready) => {
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::Stopping,
                    DtmSessionStimulus::StoppingResponseReady,
                    EmbassyBluetoothDtmSessionState::TestEndResponse(ready.into_response_pending()),
                );
                None
            }
            BluetoothDtmStoppingStep::Fault(fault) => Some(Self::transfer_transition(
                EmbassyBluetoothDtmSessionPhase::Stopping,
                DtmSessionStimulus::TerminalFault,
                EmbassyBluetoothDtmSessionBoundary::StoppingFault(fault),
            )),
        }
    }

    fn route_controller_command<'epoch, 'packet>(
        &mut self,
        route: BluetoothDtmActiveControllerCommandRoute<'runtime, 'epoch, S, CAPACITY>,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        match route {
            BluetoothDtmActiveControllerCommandRoute::ResponsePending(session) => self
                .store_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::ControllerResponsePending,
                    EmbassyBluetoothDtmSessionState::PendingResponse(session),
                ),
            BluetoothDtmActiveControllerCommandRoute::TestEnd(runner) => self.store_transition(
                EmbassyBluetoothDtmSessionPhase::CommandReady,
                DtmSessionStimulus::TestEnd,
                EmbassyBluetoothDtmSessionState::Stopping(runner),
            ),
            BluetoothDtmActiveControllerCommandRoute::ResetBarrier(barrier) => {
                return Some(Self::transfer_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::ResetBarrier,
                    EmbassyBluetoothDtmSessionBoundary::ResetBarrier(barrier),
                ));
            }
            BluetoothDtmActiveControllerCommandRoute::EndpointMismatch(mismatch) => {
                return Some(Self::transfer_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::TransferredControllerEndpointMismatch,
                    EmbassyBluetoothDtmSessionBoundary::ControllerCommandEndpointMismatch(mismatch),
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
        let EmbassyBluetoothDtmSessionState::PendingResponse(session) = self.owner.take() else {
            unreachable!("the awaited pending state did not change")
        };
        match session.try_publish_response(controller) {
            BluetoothDtmResponsePublication::Published(session) => self.store_transition(
                EmbassyBluetoothDtmSessionPhase::PendingResponse,
                DtmSessionStimulus::ResponsePublished,
                EmbassyBluetoothDtmSessionState::CommandReady(session),
            ),
            BluetoothDtmResponsePublication::Pending(session) => self.store_transition(
                EmbassyBluetoothDtmSessionPhase::PendingResponse,
                DtmSessionStimulus::Continue,
                EmbassyBluetoothDtmSessionState::PendingResponse(session),
            ),
            BluetoothDtmResponsePublication::EndpointMismatch(session) => {
                return Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::RetainedEndpointMismatch,
                    EmbassyBluetoothDtmSessionState::PendingResponse(session),
                    EmbassyBluetoothDtmSessionBoundary::EndpointMismatch,
                ));
            }
            BluetoothDtmResponsePublication::Fault { session, error } => {
                return Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::RetainedFault,
                    EmbassyBluetoothDtmSessionState::PendingResponse(session),
                    EmbassyBluetoothDtmSessionBoundary::HciFault(error),
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
        Recheck: EmbassyBluetoothDtmControllerTimeRecheck,
    >(
        &mut self,
        wakers: &EmbassyBluetoothRuntimeWakers<WakeMutex>,
        controller: &LeControllerCommandEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        recheck: &mut Recheck,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let EmbassyBluetoothDtmSessionState::PendingResponse(session) = self.owner.current() else {
            unreachable!("the selected pending phase did not change")
        };
        let Some(wait) = EmbassyBluetoothDtmActiveWait::from_waiting(session, wakers) else {
            return self.step_pending_radio();
        };
        if recheck.status() == EmbassyBluetoothDtmControllerTimeRecheckStatus::TimelineExhausted {
            return Some(self.retain_existing_transition(
                DtmSessionStimulus::ControllerTimeExhausted,
                EmbassyBluetoothDtmSessionBoundary::ControllerTimeExhausted,
            ));
        }
        match wait
            .wait_next(controller, recheck.wait_until_absolute_recheck())
            .await
        {
            Ok(EmbassyBluetoothDtmActivePendingSignal::Radio(_)) => self.step_pending_radio(),
            Ok(EmbassyBluetoothDtmActivePendingSignal::ResponseCapacity) => {
                self.try_publish_pending_response(controller)
            }
            Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch) => {
                Some(self.retain_existing_transition(
                    DtmSessionStimulus::RetainedEndpointMismatch,
                    EmbassyBluetoothDtmSessionBoundary::EndpointMismatch,
                ))
            }
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
        packet: &mut Option<CommandPacketBuffer<'packet>>,
        recheck: &mut Recheck,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        loop {
            let EmbassyBluetoothDtmSessionState::CommandReady(session) = self.owner.current()
            else {
                unreachable!("the selected command-ready phase did not change")
            };
            let Some(wait) = EmbassyBluetoothDtmActiveWait::from_waiting(session, wakers) else {
                if let Some(boundary) = self.step_command_ready_radio() {
                    return Some(boundary);
                }
                continue;
            };
            if recheck.status() == EmbassyBluetoothDtmControllerTimeRecheckStatus::TimelineExhausted
            {
                return Some(self.retain_existing_transition(
                    DtmSessionStimulus::ControllerTimeExhausted,
                    EmbassyBluetoothDtmSessionBoundary::ControllerTimeExhausted,
                ));
            }
            match wait
                .wait_next(controller, recheck.wait_until_absolute_recheck())
                .await
            {
                Ok(EmbassyBluetoothDtmActiveCommandSignal::Radio(_)) => {
                    if let Some(boundary) = self.step_command_ready_radio() {
                        return Some(boundary);
                    }
                }
                Ok(EmbassyBluetoothDtmActiveCommandSignal::HostReady) => {
                    let EmbassyBluetoothDtmSessionState::CommandReady(session) = self.owner.take()
                    else {
                        unreachable!("the awaited command-ready state did not change")
                    };
                    let CommandPacketBuffer(buffer) = packet
                        .take()
                        .expect("the active task retains its sole HCI receive buffer");
                    match session
                        .try_route_active_controller_command_with_buffer(controller, buffer)
                    {
                        BluetoothDtmActiveCommandIntake::Routed { route, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            if let Some(boundary) = self.route_controller_command(route) {
                                return Some(boundary);
                            }
                            return None;
                        }
                        BluetoothDtmActiveCommandIntake::Empty { session, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            self.store_transition(
                                EmbassyBluetoothDtmSessionPhase::CommandReady,
                                DtmSessionStimulus::Continue,
                                EmbassyBluetoothDtmSessionState::CommandReady(session),
                            );
                        }
                        BluetoothDtmActiveCommandIntake::EndpointMismatch { session, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            return Some(self.retain_transition(
                                EmbassyBluetoothDtmSessionPhase::CommandReady,
                                DtmSessionStimulus::RetainedEndpointMismatch,
                                EmbassyBluetoothDtmSessionState::CommandReady(session),
                                EmbassyBluetoothDtmSessionBoundary::EndpointMismatch,
                            ));
                        }
                        BluetoothDtmActiveCommandIntake::Channel {
                            session,
                            buffer,
                            error,
                        } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            return Some(self.retain_transition(
                                EmbassyBluetoothDtmSessionPhase::CommandReady,
                                DtmSessionStimulus::RetainedFault,
                                EmbassyBluetoothDtmSessionState::CommandReady(session),
                                EmbassyBluetoothDtmSessionBoundary::HciFault(error),
                            ));
                        }
                        BluetoothDtmActiveCommandIntake::NonCommand { session, frame } => {
                            return Some(self.retain_transition(
                                EmbassyBluetoothDtmSessionPhase::CommandReady,
                                DtmSessionStimulus::RetainedExternalFrame,
                                EmbassyBluetoothDtmSessionState::CommandReady(session),
                                EmbassyBluetoothDtmSessionBoundary::NonCommand(frame),
                            ));
                        }
                    }
                }
                Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch) => {
                    return Some(self.retain_existing_transition(
                        DtmSessionStimulus::RetainedEndpointMismatch,
                        EmbassyBluetoothDtmSessionBoundary::EndpointMismatch,
                    ));
                }
            }
        }
    }

    async fn drive_stopping<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        Recheck: EmbassyBluetoothDtmControllerTimeRecheck,
    >(
        &mut self,
        wakers: &EmbassyBluetoothRuntimeWakers<WakeMutex>,
        recheck: &mut Recheck,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let EmbassyBluetoothDtmSessionState::Stopping(runner) = self.owner.current() else {
            unreachable!("the selected stopping phase did not change")
        };
        if let Some(wait) = EmbassyBluetoothDtmStoppingWait::from_waiting(runner, wakers) {
            if recheck.status() == EmbassyBluetoothDtmControllerTimeRecheckStatus::TimelineExhausted
            {
                return Some(self.retain_existing_transition(
                    DtmSessionStimulus::ControllerTimeExhausted,
                    EmbassyBluetoothDtmSessionBoundary::ControllerTimeExhausted,
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
        let EmbassyBluetoothDtmSessionState::TestEndResponse(pending) = self.owner.current() else {
            unreachable!("the selected Test End response phase did not change")
        };
        if let Err(EmbassyBluetoothDtmTestEndResponseWaitError::EndpointMismatch) =
            EmbassyBluetoothDtmTestEndResponseWait::new(pending)
                .wait_next(controller)
                .await
        {
            return Some(self.retain_existing_transition(
                DtmSessionStimulus::RetainedEndpointMismatch,
                EmbassyBluetoothDtmSessionBoundary::EndpointMismatch,
            ));
        }

        let EmbassyBluetoothDtmSessionState::TestEndResponse(pending) = self.owner.take() else {
            unreachable!("the awaited Test End response state did not change")
        };
        match pending.try_publish(controller) {
            BluetoothDtmTestEndResponsePublication::Completed(complete) => {
                Some(Self::transfer_transition(
                    EmbassyBluetoothDtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::Completed,
                    EmbassyBluetoothDtmSessionBoundary::Complete(complete.into_idle_command_task()),
                ))
            }
            BluetoothDtmTestEndResponsePublication::Pending(pending) => {
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::Continue,
                    EmbassyBluetoothDtmSessionState::TestEndResponse(pending),
                );
                None
            }
            BluetoothDtmTestEndResponsePublication::EndpointMismatch(pending) => {
                Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::RetainedEndpointMismatch,
                    EmbassyBluetoothDtmSessionState::TestEndResponse(pending),
                    EmbassyBluetoothDtmSessionBoundary::EndpointMismatch,
                ))
            }
            BluetoothDtmTestEndResponsePublication::Fault { pending, error } => {
                Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::RetainedFault,
                    EmbassyBluetoothDtmSessionState::TestEndResponse(pending),
                    EmbassyBluetoothDtmSessionBoundary::HciFault(error),
                ))
            }
            BluetoothDtmTestEndResponsePublication::RestoreFailed(failure) => {
                self.store_transition(
                    EmbassyBluetoothDtmSessionPhase::TestEndResponse,
                    DtmSessionStimulus::RestoreRequired,
                    EmbassyBluetoothDtmSessionState::Restore(failure),
                );
                None
            }
        }
    }

    fn retry_restore<'epoch, 'packet>(
        &mut self,
    ) -> SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let EmbassyBluetoothDtmSessionState::Restore(failure) = self.owner.take() else {
            unreachable!("the selected restore state did not change")
        };
        match failure.retry_restore() {
            BluetoothDtmTestEndRestoreStep::Completed(complete) => Self::transfer_transition(
                EmbassyBluetoothDtmSessionPhase::Restore,
                DtmSessionStimulus::Completed,
                EmbassyBluetoothDtmSessionBoundary::Complete(complete.into_idle_command_task()),
            ),
            BluetoothDtmTestEndRestoreStep::Rejected(failure) => self.retain_transition(
                EmbassyBluetoothDtmSessionPhase::Restore,
                DtmSessionStimulus::Retry,
                EmbassyBluetoothDtmSessionState::Restore(failure),
                EmbassyBluetoothDtmSessionBoundary::Retryable(
                    EmbassyBluetoothDtmSessionRetry::IdleRestore,
                ),
            ),
        }
    }

    fn unowned_finished_list_boundary<'epoch, 'packet>(
        &self,
    ) -> SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let index = match self.owner.current() {
            EmbassyBluetoothDtmSessionState::UnownedPendingResponse { observed, .. }
            | EmbassyBluetoothDtmSessionState::UnownedCommandReady { observed, .. }
            | EmbassyBluetoothDtmSessionState::UnownedStopping { observed, .. } => observed.index(),
            _ => unreachable!("the selected unowned-list quarantine did not change"),
        };
        self.retain_existing_transition(
            DtmSessionStimulus::UnownedFinishedList,
            EmbassyBluetoothDtmSessionBoundary::UnownedFinishedList(index),
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
    ) -> EmbassyBluetoothDtmSessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY> {
        let mut packet = Some(CommandPacketBuffer(packet));
        loop {
            let boundary = match self.phase() {
                EmbassyBluetoothDtmSessionPhase::PendingResponse => {
                    self.drive_pending_response(wakers, controller, recheck)
                        .await
                }
                EmbassyBluetoothDtmSessionPhase::CommandReady => {
                    self.drive_command_ready(wakers, controller, &mut packet, recheck)
                        .await
                }
                EmbassyBluetoothDtmSessionPhase::Stopping => {
                    self.drive_stopping(wakers, recheck).await
                }
                EmbassyBluetoothDtmSessionPhase::TestEndResponse => {
                    self.drive_test_end_response(controller).await
                }
                EmbassyBluetoothDtmSessionPhase::Restore => return self.retry_restore(),
                EmbassyBluetoothDtmSessionPhase::UnownedFinishedList => {
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
