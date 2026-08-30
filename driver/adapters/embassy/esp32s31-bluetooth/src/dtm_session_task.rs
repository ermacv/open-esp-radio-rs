//! Sole Embassy task owner for one active LE DTM session.
//!
//! Every hardware/HCI owner stays in [`EmbassyBluetoothDtmSessionTask`] while
//! readiness futures are pending. The futures borrow only durable wake state,
//! HCI capacity/intake and a caller-owned absolute Controller-time recheck.

#![forbid(unsafe_code)]

use core::future::Future;

use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciClassifiedCommandIntake, HciEpochBound, HostToControllerFrame,
    LeControllerCommandClassification, LeDtmCommand,
};

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::InProcessHciControllerEndpoint;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmActiveCommandRoute, BluetoothDtmActiveSessionFault,
    BluetoothDtmActiveSessionRadioStep, BluetoothDtmCommandReadySession,
    BluetoothDtmResponsePending, BluetoothDtmResponsePendingSession,
    BluetoothDtmResponsePublication, BluetoothDtmResponsePublished, BluetoothDtmStoppingFault,
    BluetoothDtmStoppingRunner, BluetoothDtmStoppingStep, BluetoothDtmTestEndComplete,
    BluetoothDtmTestEndResponsePending, BluetoothDtmTestEndResponsePublication,
    BluetoothDtmTestEndRestoreFailure, BluetoothDtmTestEndRestoreStep,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
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

    fn wait_until_absolute_recheck(&mut self) -> Self::Recheck<'_>;
}

/// Phase retained after a recoverable transition asks the supervisor to retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmSessionRetry {
    ActiveRadio,
    Stopping,
    IdleRestore,
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production seam is executed only by the S31 target"
    )
)]
enum ClassifiedDtmHostIntake<'epoch, 'packet> {
    Dtm {
        command: HciEpochBound<'epoch, LeDtmCommand>,
        buffer: &'packet mut [u8],
    },
    External {
        command: HciEpochBound<'epoch, LeControllerCommandClassification>,
        buffer: &'packet mut [u8],
    },
    Empty {
        buffer: &'packet mut [u8],
    },
    Channel(HciChannelError),
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
}

#[cfg_attr(
    not(any(target_arch = "riscv32", test)),
    expect(
        dead_code,
        reason = "production seam is executed only by the S31 target"
    )
)]
fn classify_dtm_host_intake<'epoch, 'packet>(
    intake: HciClassifiedCommandIntake<'epoch, 'packet>,
) -> ClassifiedDtmHostIntake<'epoch, 'packet> {
    match intake {
        HciClassifiedCommandIntake::Command { command, buffer } => match command.try_into_dtm() {
            Ok(command) => ClassifiedDtmHostIntake::Dtm { command, buffer },
            Err(command) => ClassifiedDtmHostIntake::External { command, buffer },
        },
        HciClassifiedCommandIntake::Empty { buffer } => ClassifiedDtmHostIntake::Empty { buffer },
        HciClassifiedCommandIntake::Channel(error) => ClassifiedDtmHostIntake::Channel(error),
        HciClassifiedCommandIntake::NonCommand(frame) => ClassifiedDtmHostIntake::NonCommand(frame),
    }
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
    BusyResponse,
    TestEnd,
    StoppingResponseReady,
    RestoreRequired,
    Completed,
    Retry,
    Unrelated,
    RetainedEndpointMismatch,
    RetainedFault,
    RetainedExternalFrame,
    ExternalCommand,
    TransferredEndpointMismatch,
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
        BusyResponse, Completed, Continue, ExternalCommand, RestoreRequired,
        RetainedEndpointMismatch, RetainedExternalFrame, RetainedFault, Retry,
        StoppingResponseReady, TerminalFault, TestEnd, TransferredEndpointMismatch, Unrelated,
    };
    use EmbassyBluetoothDtmSessionPhase::{
        CommandReady, PendingResponse, Restore, Stopping, TestEndResponse,
    };

    match (phase, stimulus) {
        (PendingResponse, DtmSessionStimulus::ResponsePublished) => Advance(CommandReady),
        (CommandReady, BusyResponse) => Advance(PendingResponse),
        (CommandReady, TestEnd) => Advance(Stopping),
        (Stopping, StoppingResponseReady) => Advance(TestEndResponse),
        (TestEndResponse, RestoreRequired) => Advance(Restore),
        (TestEndResponse | Restore, Completed) => TerminalBoundary,
        (PendingResponse | CommandReady | Stopping | TestEndResponse, Continue) => Advance(phase),
        (PendingResponse | CommandReady | Stopping | Restore, Retry) => RetainBoundary,
        (PendingResponse | CommandReady | Stopping, Unrelated) => RetainBoundary,
        (PendingResponse | CommandReady | TestEndResponse, RetainedEndpointMismatch) => {
            RetainBoundary
        }
        (PendingResponse | CommandReady | TestEndResponse, RetainedFault) => RetainBoundary,
        (CommandReady, RetainedExternalFrame) => RetainBoundary,
        (CommandReady, ExternalCommand | TransferredEndpointMismatch) => TransferBoundary,
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
    /// One unrelated scheduler list remains owned by the outer dispatcher.
    UnrelatedList(BluetoothSchedulerFinishedHardwareListObserved),
    /// A non-DTM command and the unchanged active session transferred to its router.
    NonDtmCommand {
        /// Active radio/order owner required by the outer session policy.
        session: BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>,
        /// Exact classified command retaining its source HCI epoch.
        command: HciEpochBound<'epoch, LeControllerCommandClassification>,
    },
    /// A non-command Host frame remains bound to its source HCI epoch and buffer.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
    /// A foreign DTM command and unchanged active session transferred together.
    DtmCommandEndpointMismatch {
        /// Active radio/order owner that rejected the foreign command.
        session: BluetoothDtmCommandReadySession<'runtime, S, CAPACITY>,
        /// Unchanged semantic command retaining its foreign HCI epoch.
        command: HciEpochBound<'epoch, LeDtmCommand>,
    },
    /// The supplied endpoint does not match the stored response/order epoch.
    EndpointMismatch,
    /// The HCI operation failed while the complete session stayed stored.
    HciFault(HciChannelError),
    /// A finite lower transition retained its owner for an explicit retry.
    Retryable(EmbassyBluetoothDtmSessionRetry),
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
        BluetoothDtmActiveSessionFault<
            'runtime,
            S,
            CAPACITY,
            BluetoothDtmResponsePublished<'runtime>,
        >,
    ),
    /// Test End quiescence failed closed with its exact graph and command.
    StoppingFault(BluetoothDtmStoppingFault<'runtime, S, CAPACITY>),
    /// Test End completed, published exactly once and restored the idle graph.
    Complete(BluetoothDtmTestEndComplete<'runtime, S, CAPACITY>),
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
            .expect("a non-terminal DTM task always retains one affine state")
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
    /// out of this task.
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
                Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::PendingResponse,
                    DtmSessionStimulus::Unrelated,
                    EmbassyBluetoothDtmSessionState::PendingResponse(session),
                    EmbassyBluetoothDtmSessionBoundary::UnrelatedList(observed),
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
                Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::Unrelated,
                    EmbassyBluetoothDtmSessionState::CommandReady(session),
                    EmbassyBluetoothDtmSessionBoundary::UnrelatedList(observed),
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
                Some(self.retain_transition(
                    EmbassyBluetoothDtmSessionPhase::Stopping,
                    DtmSessionStimulus::Unrelated,
                    EmbassyBluetoothDtmSessionState::Stopping(runner),
                    EmbassyBluetoothDtmSessionBoundary::UnrelatedList(observed),
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

    fn route_dtm_command<
        'epoch,
        'packet,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &mut self,
        controller: &InProcessHciControllerEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        command: HciEpochBound<'epoch, LeDtmCommand>,
    ) -> Option<SessionBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>> {
        let EmbassyBluetoothDtmSessionState::CommandReady(session) = self.owner.take() else {
            unreachable!("the awaited command-ready state did not change")
        };
        match session.route_active_command(controller, command) {
            BluetoothDtmActiveCommandRoute::ResponsePending(session) => self.store_transition(
                EmbassyBluetoothDtmSessionPhase::CommandReady,
                DtmSessionStimulus::BusyResponse,
                EmbassyBluetoothDtmSessionState::PendingResponse(session),
            ),
            BluetoothDtmActiveCommandRoute::TestEnd(runner) => self.store_transition(
                EmbassyBluetoothDtmSessionPhase::CommandReady,
                DtmSessionStimulus::TestEnd,
                EmbassyBluetoothDtmSessionState::Stopping(runner),
            ),
            BluetoothDtmActiveCommandRoute::EndpointMismatch { session, command } => {
                return Some(Self::transfer_transition(
                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                    DtmSessionStimulus::TransferredEndpointMismatch,
                    EmbassyBluetoothDtmSessionBoundary::DtmCommandEndpointMismatch {
                        session,
                        command,
                    },
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
        controller: &InProcessHciControllerEndpoint<
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
        controller: &InProcessHciControllerEndpoint<
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
        controller: &InProcessHciControllerEndpoint<
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
                    let CommandPacketBuffer(buffer) = packet
                        .take()
                        .expect("the active task retains its sole HCI receive buffer");
                    match classify_dtm_host_intake(
                        controller.try_receive_classified_command_with_buffer(buffer),
                    ) {
                        ClassifiedDtmHostIntake::Dtm { command, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            if let Some(boundary) = self.route_dtm_command(controller, command) {
                                return Some(boundary);
                            }
                            return None;
                        }
                        ClassifiedDtmHostIntake::External { command, buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            let EmbassyBluetoothDtmSessionState::CommandReady(session) =
                                self.owner.take()
                            else {
                                unreachable!(
                                    "the synchronously classified command-ready state did not change"
                                )
                            };
                            return Some(Self::transfer_transition(
                                EmbassyBluetoothDtmSessionPhase::CommandReady,
                                DtmSessionStimulus::ExternalCommand,
                                EmbassyBluetoothDtmSessionBoundary::NonDtmCommand {
                                    session,
                                    command,
                                },
                            ));
                        }
                        ClassifiedDtmHostIntake::Empty { buffer } => {
                            packet.replace(CommandPacketBuffer(buffer));
                            match reduce_dtm_session_transition(
                                EmbassyBluetoothDtmSessionPhase::CommandReady,
                                DtmSessionStimulus::Continue,
                            ) {
                                DtmSessionAction::Advance(
                                    EmbassyBluetoothDtmSessionPhase::CommandReady,
                                ) => {}
                                _ => unreachable!("the DTM reducer rejected stale HCI readiness"),
                            }
                        }
                        ClassifiedDtmHostIntake::Channel(error) => {
                            return Some(self.retain_existing_transition(
                                DtmSessionStimulus::RetainedFault,
                                EmbassyBluetoothDtmSessionBoundary::HciFault(error),
                            ));
                        }
                        ClassifiedDtmHostIntake::NonCommand(frame) => {
                            return Some(self.retain_existing_transition(
                                DtmSessionStimulus::RetainedExternalFrame,
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
        controller: &InProcessHciControllerEndpoint<
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
                    EmbassyBluetoothDtmSessionBoundary::Complete(complete),
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
                EmbassyBluetoothDtmSessionBoundary::Complete(complete),
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
        controller: &InProcessHciControllerEndpoint<
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
            };
            if let Some(boundary) = boundary {
                return boundary;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, pending},
        pin::Pin,
        task::Context,
    };

    use bt_hci::{
        cmd::{Cmd, controller_baseband::Reset, le::LeTestEnd},
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        param::ConnHandle,
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        HciChannelError, HostToControllerFrame, InProcessHciChannel,
        LeControllerCommandClassification, LeDtmCommand,
    };
    use std::{boxed::Box, rc::Rc, task::Waker};

    use super::{
        ClassifiedDtmHostIntake, DtmSessionAction, DtmSessionStimulus,
        EmbassyBluetoothDtmSessionPhase, SessionOwnerSlot, classify_dtm_host_intake,
        reduce_dtm_session_transition,
    };

    type TestChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

    #[derive(Debug, Eq, PartialEq)]
    struct FakeOwner {
        generation: u8,
        drops: Rc<core::cell::Cell<u8>>,
    }

    impl Drop for FakeOwner {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    async fn wait_while_owner_is_stored(owner: &SessionOwnerSlot<FakeOwner>) {
        let _borrowed_owner = owner.current();
        pending::<()>().await;
    }

    #[test]
    fn cancelling_a_borrowed_wait_retains_the_exact_owner() {
        let drops = Rc::new(core::cell::Cell::new(0));
        let slot = SessionOwnerSlot::new(FakeOwner {
            generation: 7,
            drops: Rc::clone(&drops),
        });
        let mut waiting = Box::pin(wait_while_owner_is_stored(&slot));
        let mut context = Context::from_waker(Waker::noop());
        assert!(Future::poll(Pin::as_mut(&mut waiting), &mut context).is_pending());
        drop(waiting);

        assert_eq!(slot.current().generation, 7);
        assert_eq!(drops.get(), 0);
        drop(slot);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn production_slot_transitions_without_copying_or_overwriting_owner() {
        let drops = Rc::new(core::cell::Cell::new(0));
        let mut slot = SessionOwnerSlot::new(FakeOwner {
            generation: 1,
            drops: Rc::clone(&drops),
        });

        let mut owner = slot.take();
        owner.generation = 2;
        let observation = slot.retain(owner, "unrelated");

        assert_eq!(observation, "unrelated");
        assert_eq!(slot.current().generation, 2);
        assert_eq!(drops.get(), 0);
        assert!(!slot.is_empty());
    }

    #[test]
    fn external_boundary_transfers_the_owner_and_empties_the_task_slot() {
        let drops = Rc::new(core::cell::Cell::new(0));
        let mut slot = SessionOwnerSlot::new(FakeOwner {
            generation: 3,
            drops: Rc::clone(&drops),
        });

        let owner = slot.take();

        assert!(slot.is_empty());
        assert_eq!(owner.generation, 3);
        assert_eq!(drops.get(), 0);
        drop(owner);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn production_classifier_preserves_fifo_buffer_and_epoch_for_dtm_and_reset() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let mut foreign_channel = TestChannel::new();
        let (_foreign_host, foreign_controller) = foreign_channel.split();
        block_on(async {
            host.write(&LeTestEnd::new()).await.unwrap();
            host.write(&Reset::new()).await.unwrap();
        });

        let mut storage = [0; 16];
        let storage_address = storage.as_mut_ptr();
        let (test_end, buffer) = match classify_dtm_host_intake(
            controller.try_receive_classified_command_with_buffer(&mut storage),
        ) {
            ClassifiedDtmHostIntake::Dtm { command, buffer } => (command, buffer),
            _ => panic!("the oldest real Host command must classify as DTM Test End"),
        };
        assert!(matches!(test_end.value(), LeDtmCommand::TestEnd(_)));
        assert!(test_end.originates_from(&controller));
        assert!(!test_end.originates_from(&foreign_controller));
        assert_eq!(buffer.as_mut_ptr(), storage_address);

        let (reset, buffer) = match classify_dtm_host_intake(
            controller.try_receive_classified_command_with_buffer(buffer),
        ) {
            ClassifiedDtmHostIntake::External { command, buffer } => (command, buffer),
            _ => panic!("the second real Host command must remain external Reset"),
        };
        assert_eq!(reset.value().opcode(), Reset::OPCODE);
        assert!(matches!(
            reset.value(),
            LeControllerCommandClassification::Bootstrap(_)
        ));
        assert!(reset.originates_from(&controller));
        assert!(!reset.originates_from(&foreign_controller));
        assert_eq!(buffer.as_mut_ptr(), storage_address);

        let buffer = match classify_dtm_host_intake(
            controller.try_receive_classified_command_with_buffer(buffer),
        ) {
            ClassifiedDtmHostIntake::Empty { buffer } => buffer,
            _ => panic!("the drained real FIFO must return its reusable buffer"),
        };
        assert_eq!(buffer.as_mut_ptr(), storage_address);
    }

    #[test]
    fn production_classifier_transfers_exact_acl_and_retains_epoch() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let acl = AclPacket::new(
            ConnHandle::new(5),
            AclPacketBoundary::Complete,
            AclBroadcastFlag::PointToPoint,
            &[11, 17],
        );
        block_on(host.write(&acl)).unwrap();

        let mut storage = [0; 16];
        let frame = match classify_dtm_host_intake(
            controller.try_receive_classified_command_with_buffer(&mut storage),
        ) {
            ClassifiedDtmHostIntake::NonCommand(frame) => frame,
            _ => panic!("the real ACL packet must remain an external data frame"),
        };
        assert!(frame.originates_from(&controller));
        let HostToControllerFrame::Acl(received) = frame.value() else {
            panic!("the production classifier changed the exact ACL packet kind");
        };
        assert_eq!(received.handle(), ConnHandle::new(5));
        assert_eq!(received.boundary_flag(), AclPacketBoundary::Complete);
        assert_eq!(received.broadcast_flag(), AclBroadcastFlag::PointToPoint);
        assert_eq!(received.data(), &[11, 17]);
    }

    #[test]
    fn production_classifier_preserves_the_real_channel_fault() {
        let mut channel = TestChannel::new();
        let (_host, controller) = channel.split();
        let mut undersized = [];

        let error = match classify_dtm_host_intake(
            controller.try_receive_classified_command_with_buffer(&mut undersized),
        ) {
            ClassifiedDtmHostIntake::Channel(error) => error,
            _ => panic!("an undersized production buffer must remain a channel fault"),
        };
        assert_eq!(
            error,
            HciChannelError::DestinationTooSmall {
                required: 16,
                available: 0,
            }
        );
    }

    #[test]
    fn production_reducer_covers_each_task_phase_and_action() {
        use DtmSessionAction::{Advance, RetainBoundary, TerminalBoundary, TransferBoundary};
        use DtmSessionStimulus::{
            BusyResponse, Completed, Continue, ExternalCommand, ResponsePublished, RestoreRequired,
            RetainedEndpointMismatch, RetainedExternalFrame, RetainedFault, Retry,
            StoppingResponseReady, TerminalFault, TestEnd, TransferredEndpointMismatch, Unrelated,
        };
        use EmbassyBluetoothDtmSessionPhase::{
            CommandReady, PendingResponse, Restore, Stopping, TestEndResponse,
        };

        let cases = [
            (PendingResponse, ResponsePublished, Advance(CommandReady)),
            (CommandReady, BusyResponse, Advance(PendingResponse)),
            (CommandReady, TestEnd, Advance(Stopping)),
            (Stopping, StoppingResponseReady, Advance(TestEndResponse)),
            (TestEndResponse, RestoreRequired, Advance(Restore)),
            (TestEndResponse, Completed, TerminalBoundary),
            (Restore, Completed, TerminalBoundary),
            (PendingResponse, Continue, Advance(PendingResponse)),
            (TestEndResponse, Continue, Advance(TestEndResponse)),
            (PendingResponse, Retry, RetainBoundary),
            (Stopping, Unrelated, RetainBoundary),
            (PendingResponse, RetainedEndpointMismatch, RetainBoundary),
            (CommandReady, RetainedFault, RetainBoundary),
            (PendingResponse, TerminalFault, TerminalBoundary),
            (CommandReady, ExternalCommand, TransferBoundary),
            (CommandReady, TransferredEndpointMismatch, TransferBoundary),
            (CommandReady, RetainedExternalFrame, RetainBoundary),
        ];

        for (phase, stimulus, expected) in cases {
            assert_eq!(reduce_dtm_session_transition(phase, stimulus), expected);
        }
    }
}
