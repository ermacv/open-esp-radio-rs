//! Bounded first-event runner for legacy LE Direct Test Mode.
//!
//! The runner owns one semantic HCI command together with every affine
//! Controller typestate needed to reach the first scheduler `RUN`. Each
//! [`BluetoothDtmFirstRunner::step`] call performs exactly one finite lower
//! transition. In particular, a controller-time `Waiting` result is returned
//! to the executor instead of being polled in a hidden loop.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciControllerResponse, InProcessHciControllerEndpoint,
    LeDtmCommandCompleteEvent, LeReceiverTestV1Command, LeTransmitterTestV1Command,
};

use crate::{
    BluetoothAlwaysAwakePostEnableTimeBeginFailure, BluetoothAlwaysAwakePostEnableTimeFailure,
    BluetoothAlwaysAwakePostEnableTimePending, BluetoothAlwaysAwakePostEnableTimeStep,
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginFailure,
    BluetoothControllerSchedulerCurrentError, BluetoothControllerSchedulerCurrentFailure,
    BluetoothControllerSchedulerCurrentPending, BluetoothControllerSchedulerCurrentStep,
    BluetoothControllerSchedulerEpochRetained, BluetoothControllerSchedulerNowReady,
    BluetoothControllerTimeOrphanDrainStep, BluetoothDtmControllerEventPreparationError,
    BluetoothDtmControllerInitialPreparationFailure, BluetoothDtmControllerPreparationOutcome,
    BluetoothDtmControllerPreparationPending, BluetoothDtmControllerPreparationStep,
    BluetoothDtmControllerPreparationTerminal, BluetoothDtmControllerRxPreparationFailure,
    BluetoothDtmControllerTxPreparationFailure, BluetoothDtmEmptySchedulerMergePrepared,
    BluetoothDtmInitialSchedulerItemPhase, BluetoothDtmReceiverEvent,
    BluetoothDtmSchedulerHeadPublicationError, BluetoothDtmSchedulerHeadPublished,
    BluetoothDtmSchedulerRunning, BluetoothDtmSessionIdle, BluetoothDtmTransmitterEvent,
    BluetoothSchedulerRunInterruptStorage,
};

/// One validated legacy LE test command which may start a fresh DTM session.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the semantic HCI command must reach hardware or an owned failure"]
pub enum BluetoothDtmFirstCommand {
    /// Begin an LE 1M transmitter test.
    Transmitter(LeTransmitterTestV1Command),
    /// Begin an LE 1M receiver test.
    Receiver(LeReceiverTestV1Command),
}

/// Bounded first-event Controller runner.
///
/// Every variant retains the sole task service either directly or inside one
/// lower affine state. The private role-specific variants prevent a TX command
/// from being paired with an RX descriptor graph.
#[must_use = "step or explicitly cancel the affine DTM runner"]
pub struct BluetoothDtmFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain complete affine Controller owners"
)]
enum BluetoothDtmFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: BluetoothDtmFirstCommand,
        pending: BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: BluetoothDtmFirstCommand,
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: BluetoothDtmFirstCommand,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: BluetoothDtmFirstCommand,
        pending: BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    TransmitterPrepared {
        command: LeTransmitterTestV1Command,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
    },
    ReceiverPrepared {
        command: LeReceiverTestV1Command,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
    },
    TransmitterHead {
        command: LeTransmitterTestV1Command,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: BluetoothDtmSchedulerHeadPublished<BluetoothDtmTransmitterEvent>,
    },
    ReceiverHead {
        command: LeReceiverTestV1Command,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: BluetoothDtmSchedulerHeadPublished<BluetoothDtmReceiverEvent>,
    },
}

/// Result of one bounded runner step.
#[must_use = "retain pending ownership or handle the terminal first-event result"]
pub enum BluetoothDtmFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// A self-clearing controller-time latch still needs a later observation.
    WaitControllerTime(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// The previous bounded transition completed; another can run immediately.
    Continue(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact first graph reached hardware scheduler `RUN`.
    Running(BluetoothDtmFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    /// A finite transition failed while retaining its exact owner.
    Failed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First DTM event admitted to the hardware scheduler.
///
/// The semantic command remains owned until its Command Complete response is
/// durably published. Hardware progress therefore does not depend on HCI
/// Controller-to-Host queue capacity.
#[must_use = "the running graph and pending HCI response authority must remain owned"]
pub struct BluetoothDtmFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    running: BluetoothDtmFirstRunningPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum BluetoothDtmFirstRunningPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter {
        command: LeTransmitterTestV1Command,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmTransmitterEvent>,
    },
    Receiver {
        command: LeReceiverTestV1Command,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmReceiverEvent>,
    },
}

enum BluetoothDtmFirstActivePhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmTransmitterEvent>,
    },
    Receiver {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmReceiverEvent>,
    },
}

pub(crate) enum BluetoothDtmFirstActiveParts<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmTransmitterEvent>,
    },
    Receiver {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmReceiverEvent>,
    },
}

/// Running first event with its successful Command Complete awaiting durable
/// Controller-to-Host publication.
#[must_use = "publish the response before releasing active-session ownership"]
pub struct BluetoothDtmFirstResponsePending<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    response: LeDtmCommandCompleteEvent,
    active: BluetoothDtmFirstActivePhase<'runtime, S, SCHEDULER_CAPACITY>,
}

/// Active first DTM event after its start response was durably published.
#[must_use = "the active DTM graph must progress through completion or Test End"]
pub struct BluetoothDtmFirstActive<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    active: BluetoothDtmFirstActivePhase<'runtime, S, SCHEDULER_CAPACITY>,
}

/// Result of attempting the sole durable publication of a DTM start response.
#[must_use = "retain response ownership across Controller-to-Host backpressure"]
pub enum BluetoothDtmFirstResponsePublication<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The exact response entered the queue and the active graph may advance.
    Published(BluetoothDtmFirstActive<'runtime, S, SCHEDULER_CAPACITY>),
    /// The bounded queue was full; the unchanged publication owner is retained.
    Pending(BluetoothDtmFirstResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The supplied HCI endpoint belongs to another Controller epoch.
    EndpointMismatch(BluetoothDtmFirstResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// A non-backpressure boundary fault retained the unchanged publication owner.
    Fault {
        /// Exact response and active hardware owner which were not published.
        pending: BluetoothDtmFirstResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        /// Exact HCI validation or transport-boundary failure.
        error: HciChannelError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Consume the exact semantic command into its successful Command Complete
    /// while retaining task and hardware ownership beside the response.
    pub fn into_response_pending(
        self,
    ) -> BluetoothDtmFirstResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        let (response, active) = match self.running {
            BluetoothDtmFirstRunningPhase::Transmitter {
                command,
                task,
                running,
            } => (
                command.into_started_command_complete(),
                BluetoothDtmFirstActivePhase::Transmitter { task, running },
            ),
            BluetoothDtmFirstRunningPhase::Receiver {
                command,
                task,
                running,
            } => (
                command.into_started_command_complete(),
                BluetoothDtmFirstActivePhase::Receiver { task, running },
            ),
        };
        BluetoothDtmFirstResponsePending { response, active }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstResponsePending<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    const fn response(&self) -> &LeDtmCommandCompleteEvent {
        &self.response
    }

    const fn hci_epoch_identity(&self) -> open_esp_radio_bluetooth_hci::HciEpochIdentity<'_> {
        match &self.active {
            BluetoothDtmFirstActivePhase::Transmitter { task, .. }
            | BluetoothDtmFirstActivePhase::Receiver { task, .. } => task.hci_epoch_identity(),
        }
    }

    /// Whether a raw Controller endpoint belongs to this running event's HCI epoch.
    pub fn matches_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.hci_epoch_identity()
            .same_epoch(controller.epoch_identity())
    }

    fn into_active(self) -> BluetoothDtmFirstActive<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothDtmFirstActive {
            active: self.active,
        }
    }

    /// Attempt the sole durable publication without awaiting queue capacity.
    ///
    /// `Full` returns the exact unchanged owner for a later retry. The response
    /// bytes are never exposed, and only a successful queue insertion releases
    /// the active-session state.
    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothDtmFirstResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        if !self.matches_hci_endpoint(controller) {
            return BluetoothDtmFirstResponsePublication::EndpointMismatch(self);
        }
        let result = {
            let response = self.response();
            controller.try_publish(response.kind(), response.as_bytes())
        };
        match result {
            Ok(()) => BluetoothDtmFirstResponsePublication::Published(self.into_active()),
            Err(HciChannelError::Full) => BluetoothDtmFirstResponsePublication::Pending(self),
            Err(error) => BluetoothDtmFirstResponsePublication::Fault {
                pending: self,
                error,
            },
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstActive<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Role of the exact graph currently owned by hardware.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        match &self.active {
            BluetoothDtmFirstActivePhase::Transmitter { running, .. } => running.role(),
            BluetoothDtmFirstActivePhase::Receiver { running, .. } => running.role(),
        }
    }

    /// Durable scheduler notification source for the exact active Controller.
    pub const fn scheduler_wake(&self) -> &crate::BluetoothSchedulerWakeCell {
        match &self.active {
            BluetoothDtmFirstActivePhase::Transmitter { task, .. }
            | BluetoothDtmFirstActivePhase::Receiver { task, .. } => task.scheduler_wake(),
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> BluetoothDtmFirstActiveParts<'runtime, S, SCHEDULER_CAPACITY> {
        match self.active {
            BluetoothDtmFirstActivePhase::Transmitter { task, running } => {
                BluetoothDtmFirstActiveParts::Transmitter { task, running }
            }
            BluetoothDtmFirstActivePhase::Receiver { task, running } => {
                BluetoothDtmFirstActiveParts::Receiver { task, running }
            }
        }
    }
}

/// Finite reason a pre-`RUN` runner transition may be retried unchanged.
#[must_use = "the retry cause should be inspected before advancing the retained runner"]
pub enum BluetoothDtmFirstRunnerRetryCause<E> {
    /// The CPU-owned graph could not become the scheduler hardware-list head.
    HeadPublication(BluetoothDtmSchedulerHeadPublicationError),
    /// Dynamic scheduler interrupt preparation rejected the published head.
    SchedulerStart(E),
}

/// Opaque retry owner for a role-consistent pre-`RUN` state.
///
/// The private fields prevent safe code from pairing a TX command with an RX
/// graph or from pairing a task service with another published head.
#[must_use = "inspect the cause and retain, retry, or cancel the exact runner"]
pub struct BluetoothDtmFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothDtmFirstRunnerRetryCause<S::Error>,
    role: crate::BluetoothDtmRole,
    runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Role of the exact command and graph retained for retry.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.role
    }

    /// Recover the owning cause and the only runner which may retry it.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmFirstRunnerRetryCause<S::Error>,
        BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ) {
        (self.cause, self.runner)
    }
}

/// Opaque cleanup after an initial DTM preparation released its CPU graph.
///
/// The graph remains paired with the exact Controller while an abandoned
/// controller-time request is drained and the runtime idle slot is restored.
#[must_use = "drive cleanup until the exact DTM graph is restored"]
pub struct BluetoothDtmFirstPreparationCleanup<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmFirstPreparationCleanupPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum BluetoothDtmFirstPreparationCleanupPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Drain {
        command: BluetoothDtmFirstCommand,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
        error: BluetoothDtmControllerEventPreparationError,
    },
    Restore {
        command: BluetoothDtmFirstCommand,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
        error: BluetoothDtmControllerEventPreparationError,
    },
}

/// One bounded graph-cleanup transition.
#[must_use = "retain cleanup ownership until the Controller can start another DTM session"]
pub enum BluetoothDtmFirstPreparationCleanupStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The abandoned controller-time latch still owns its result.
    Waiting(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    /// Time cleanup completed; graph restoration is the next bounded step.
    Continue(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact graph is idle again beside the command which did not run.
    CleanTask {
        /// Original semantic command retained for policy or retry.
        command: BluetoothDtmFirstCommand,
        /// Complete Controller with its sole DTM graph restored.
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        /// Exact preparation rejection which required cleanup.
        error: BluetoothDtmControllerEventPreparationError,
    },
    /// A time-worker fault retained the unchanged cleanup owner.
    Fault {
        /// Exact cleanup transaction which may be rechecked or fail-stopped.
        cleanup: BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
        /// Exact orphan-drain failure.
        error: BluetoothControllerSchedulerCurrentError,
    },
    /// The runtime rejected restoration without separating graph and Controller.
    RestoreRejected(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn drain(
        command: BluetoothDtmFirstCommand,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
        error: BluetoothDtmControllerEventPreparationError,
    ) -> Self {
        Self {
            phase: BluetoothDtmFirstPreparationCleanupPhase::Drain {
                command,
                epoch,
                idle,
                error,
            },
        }
    }

    /// Exact lower preparation failure which made cleanup necessary.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        match &self.phase {
            BluetoothDtmFirstPreparationCleanupPhase::Drain { error, .. }
            | BluetoothDtmFirstPreparationCleanupPhase::Restore { error, .. } => *error,
        }
    }

    /// Execute one finite drain or restore operation.
    pub fn step(self) -> BluetoothDtmFirstPreparationCleanupStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothDtmFirstPreparationCleanupPhase::Drain {
                command,
                mut epoch,
                idle,
                error,
            } => match epoch.drain_abandoned_controller_time() {
                Ok(BluetoothControllerTimeOrphanDrainStep::Waiting) => {
                    BluetoothDtmFirstPreparationCleanupStep::Waiting(Self::drain(
                        command, epoch, idle, error,
                    ))
                }
                Ok(
                    BluetoothControllerTimeOrphanDrainStep::Idle
                    | BluetoothControllerTimeOrphanDrainStep::Drained,
                ) => BluetoothDtmFirstPreparationCleanupStep::Continue(Self {
                    phase: BluetoothDtmFirstPreparationCleanupPhase::Restore {
                        command,
                        task: epoch.into_task_service(),
                        idle,
                        error,
                    },
                }),
                Err(drain_error) => BluetoothDtmFirstPreparationCleanupStep::Fault {
                    cleanup: Self::drain(command, epoch, idle, error),
                    error: drain_error,
                },
            },
            BluetoothDtmFirstPreparationCleanupPhase::Restore {
                command,
                mut task,
                idle,
                error,
            } => match task.restore_dtm_session_idle(idle) {
                Ok(()) => BluetoothDtmFirstPreparationCleanupStep::CleanTask {
                    command,
                    task,
                    error,
                },
                Err(idle) => BluetoothDtmFirstPreparationCleanupStep::RestoreRejected(Self {
                    phase: BluetoothDtmFirstPreparationCleanupPhase::Restore {
                        command,
                        task,
                        idle,
                        error,
                    },
                }),
            },
        }
    }
}

/// Fail-stop owner for an impossible command/preparation role mismatch.
///
/// No decomposition API exists because separating the Controller from the raw
/// lower outcome would re-open role cross-wiring in safe code.
#[must_use = "an invariant fault retains the complete Controller and graph owner"]
pub struct BluetoothDtmFirstInvariantFault<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    expected: crate::BluetoothDtmRole,
    observed: crate::BluetoothDtmRole,
    _command: BluetoothDtmFirstCommand,
    _epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    _outcome: BluetoothDtmControllerPreparationOutcome,
}

impl<S, const SCHEDULER_CAPACITY: usize> BluetoothDtmFirstInvariantFault<'_, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Role required by the retained semantic HCI command.
    pub const fn expected_role(&self) -> crate::BluetoothDtmRole {
        self.expected
    }

    /// Role carried by the impossible lower preparation outcome.
    pub const fn observed_role(&self) -> crate::BluetoothDtmRole {
        self.observed
    }
}

/// Opaque task/graph pair rejected by the runtime idle slot.
#[must_use = "retry restoration without separating the exact task and graph"]
pub struct BluetoothDtmFirstIdleRestore<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: BluetoothDtmFirstCommand,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    idle: BluetoothDtmSessionIdle,
}

/// Result of retrying one exact idle-slot restoration.
#[must_use = "retain a rejected task/graph pair or consume the clean task"]
pub enum BluetoothDtmFirstIdleRestoreStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The sole DTM graph is reusable by this exact Controller again.
    CleanTask {
        /// Original semantic command which never reached hardware.
        command: BluetoothDtmFirstCommand,
        /// Complete Controller with its DTM graph restored.
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    },
    /// The slot still rejected the unchanged task/graph pair.
    Rejected(BluetoothDtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn new(
        command: BluetoothDtmFirstCommand,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
    ) -> Self {
        Self {
            command,
            task,
            idle,
        }
    }

    /// Retry one finite restore operation on the same runtime identity.
    pub fn step(mut self) -> BluetoothDtmFirstIdleRestoreStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => BluetoothDtmFirstIdleRestoreStep::CleanTask {
                command: self.command,
                task: self.task,
            },
            Err(idle) => {
                self.idle = idle;
                BluetoothDtmFirstIdleRestoreStep::Rejected(self)
            }
        }
    }
}

/// Lossless first-event runner failure.
#[must_use = "every failure retains command and Controller ownership"]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain complete affine retry and cleanup owners"
)]
pub enum BluetoothDtmFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdBegin {
        command: BluetoothDtmFirstCommand,
        failure: BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        command: BluetoothDtmFirstCommand,
        failure: BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        command: BluetoothDtmFirstCommand,
        failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        command: BluetoothDtmFirstCommand,
        failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    SessionActive {
        command: BluetoothDtmFirstCommand,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    /// A failed initial preparation requires time drain and graph restoration.
    PreparationRejected(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    InvariantFault(BluetoothDtmFirstInvariantFault<'runtime, S, SCHEDULER_CAPACITY>),
    /// A role-consistent graph retained at the exact failed transition.
    Retryable(BluetoothDtmFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of explicitly cancelling a runner before or after head publication.
#[must_use = "cancellation must retain the recovered or irreversible owner"]
pub enum BluetoothDtmFirstRunnerCancel<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    CleanTask {
        command: BluetoothDtmFirstCommand,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CleanEpoch {
        command: BluetoothDtmFirstCommand,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    },
    NeedsColdTimeDrain {
        command: BluetoothDtmFirstCommand,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    },
    NeedsWarmTimeDrain {
        command: BluetoothDtmFirstCommand,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    },
    NeedsPreparationCleanup(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    RestoreRejected(BluetoothDtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
    CancellationRejected(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    HeadPublished(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_phase(phase: BluetoothDtmFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>) -> Self {
        Self { phase }
    }

    /// Begin either the cold first-live or warm fresh-current acquisition.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains exact command and Controller owners"
    )]
    pub fn begin(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: BluetoothDtmFirstCommand,
    ) -> Result<Self, BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending },
                )),
                Err(failure) => Err(BluetoothDtmFirstRunnerFailure::WarmBegin { command, failure }),
            },
            Err(unavailable) => {
                match unavailable
                    .into_task_service()
                    .begin_always_awake_post_enable_time()
                {
                    Ok(pending) => Ok(Self::from_phase(
                        BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending },
                    )),
                    Err(failure) => {
                        Err(BluetoothDtmFirstRunnerFailure::ColdBegin { command, failure })
                    }
                }
            }
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> BluetoothDtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending },
                        ))
                    }
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::CurrentReady {
                                command,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::ColdRecheck { command, failure },
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending },
                        ))
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::CurrentReady { command, current },
                        ))
                    }
                    Err(failure) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::WarmRecheck { command, failure },
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::CurrentReady { command, current } => {
                Self::begin_preparation(command, current)
            }
            BluetoothDtmFirstRunnerPhase::Preparation { command, pending } => {
                match pending.recheck() {
                    BluetoothDtmControllerPreparationStep::Pending(pending) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    BluetoothDtmControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(command, terminal)
                    }
                }
            }
            BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                command,
                mut task,
                merged,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothDtmFirstRunnerPhase::TransmitterHead {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    let merged = failure.into_merged();
                    let runner =
                        Self::from_phase(BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                            command,
                            task,
                            merged,
                        });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::HeadPublication(error),
                            role: crate::BluetoothDtmRole::Transmitter,
                            runner,
                        },
                    ))
                }
            },
            BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                command,
                mut task,
                merged,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothDtmFirstRunnerPhase::ReceiverHead {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    let merged = failure.into_merged();
                    let runner = Self::from_phase(BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                        command,
                        task,
                        merged,
                    });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::HeadPublication(error),
                            role: crate::BluetoothDtmRole::Receiver,
                            runner,
                        },
                    ))
                }
            },
            BluetoothDtmFirstRunnerPhase::TransmitterHead {
                command,
                mut task,
                head,
            } => match task.start_dtm_scheduler(head) {
                Ok(running) => BluetoothDtmFirstRunnerStep::Running(BluetoothDtmFirstRunning {
                    running: BluetoothDtmFirstRunningPhase::Transmitter {
                        command,
                        task,
                        running,
                    },
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    let runner = Self::from_phase(BluetoothDtmFirstRunnerPhase::TransmitterHead {
                        command,
                        task,
                        head,
                    });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::SchedulerStart(error),
                            role: crate::BluetoothDtmRole::Transmitter,
                            runner,
                        },
                    ))
                }
            },
            BluetoothDtmFirstRunnerPhase::ReceiverHead {
                command,
                mut task,
                head,
            } => match task.start_dtm_scheduler(head) {
                Ok(running) => BluetoothDtmFirstRunnerStep::Running(BluetoothDtmFirstRunning {
                    running: BluetoothDtmFirstRunningPhase::Receiver {
                        command,
                        task,
                        running,
                    },
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    let runner = Self::from_phase(BluetoothDtmFirstRunnerPhase::ReceiverHead {
                        command,
                        task,
                        head,
                    });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::SchedulerStart(error),
                            role: crate::BluetoothDtmRole::Receiver,
                            runner,
                        },
                    ))
                }
            },
        }
    }

    fn begin_preparation(
        command: BluetoothDtmFirstCommand,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> BluetoothDtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match command {
            BluetoothDtmFirstCommand::Transmitter(command) => {
                let program = crate::dtm_command::transmitter_program(&command);
                let result = current.begin_dtm_transmitter_first_item(
                    program.pattern,
                    program.length,
                    program.channel,
                    program.phy,
                    program.requested_interval_micros,
                );
                let command = BluetoothDtmFirstCommand::Transmitter(command);
                match result {
                    Ok(pending) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    Err(BluetoothDtmControllerInitialPreparationFailure::SessionActive(
                        current,
                    )) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::SessionActive { command, current },
                    ),
                    Err(BluetoothDtmControllerInitialPreparationFailure::PreparationTerminal(
                        terminal,
                    )) => Self::finish_preparation(command, terminal),
                }
            }
            BluetoothDtmFirstCommand::Receiver(command) => {
                let program = crate::dtm_command::receiver_program(&command);
                let result = current.begin_dtm_receiver_first_item(program.channel, program.phy);
                let command = BluetoothDtmFirstCommand::Receiver(command);
                match result {
                    Ok(pending) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    Err(BluetoothDtmControllerInitialPreparationFailure::SessionActive(
                        current,
                    )) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::SessionActive { command, current },
                    ),
                    Err(BluetoothDtmControllerInitialPreparationFailure::PreparationTerminal(
                        terminal,
                    )) => Self::finish_preparation(command, terminal),
                }
            }
        }
    }

    fn finish_preparation(
        command: BluetoothDtmFirstCommand,
        terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> BluetoothDtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        match (command, outcome) {
            (
                BluetoothDtmFirstCommand::Transmitter(command),
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Ok(merged)),
            ) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                    command,
                    task: epoch.into_task_service(),
                    merged,
                },
            )),
            (
                BluetoothDtmFirstCommand::Receiver(command),
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Ok(merged)),
            ) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                    command,
                    task: epoch.into_task_service(),
                    merged,
                },
            )),
            (
                BluetoothDtmFirstCommand::Transmitter(command),
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerStep::Failed(
                BluetoothDtmFirstRunnerFailure::PreparationRejected(
                    Self::transmitter_preparation_cleanup(command, epoch, failure),
                ),
            ),
            (
                BluetoothDtmFirstCommand::Receiver(command),
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerStep::Failed(
                BluetoothDtmFirstRunnerFailure::PreparationRejected(
                    Self::receiver_preparation_cleanup(command, epoch, failure),
                ),
            ),
            (command, outcome) => {
                BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::InvariantFault(
                    Self::invariant_fault(command, epoch, outcome),
                ))
            }
        }
    }

    fn transmitter_preparation_cleanup(
        command: LeTransmitterTestV1Command,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        failure: BluetoothDtmControllerTxPreparationFailure,
    ) -> BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY> {
        let error = failure.error();
        let (graph, _, _) = failure.into_parts();
        BluetoothDtmFirstPreparationCleanup::drain(
            BluetoothDtmFirstCommand::Transmitter(command),
            epoch,
            BluetoothDtmSessionIdle::new(graph),
            error,
        )
    }

    fn receiver_preparation_cleanup(
        command: LeReceiverTestV1Command,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        failure: BluetoothDtmControllerRxPreparationFailure,
    ) -> BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY> {
        let error = failure.error();
        let (graph, _) = failure.into_owner().into_memory_and_packet_count();
        BluetoothDtmFirstPreparationCleanup::drain(
            BluetoothDtmFirstCommand::Receiver(command),
            epoch,
            BluetoothDtmSessionIdle::new(graph),
            error,
        )
    }

    fn cancelled_preparation(
        command: BluetoothDtmFirstCommand,
        terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> BluetoothDtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        match (command, outcome) {
            (
                BluetoothDtmFirstCommand::Transmitter(command),
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerCancel::NeedsPreparationCleanup(
                Self::transmitter_preparation_cleanup(command, epoch, failure),
            ),
            (
                BluetoothDtmFirstCommand::Receiver(command),
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerCancel::NeedsPreparationCleanup(
                Self::receiver_preparation_cleanup(command, epoch, failure),
            ),
            (command, outcome) => BluetoothDtmFirstRunnerCancel::Failed(
                BluetoothDtmFirstRunnerFailure::InvariantFault(Self::invariant_fault(
                    command, epoch, outcome,
                )),
            ),
        }
    }

    fn invariant_fault(
        command: BluetoothDtmFirstCommand,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothDtmControllerPreparationOutcome,
    ) -> BluetoothDtmFirstInvariantFault<'runtime, S, SCHEDULER_CAPACITY> {
        let expected = match &command {
            BluetoothDtmFirstCommand::Transmitter(_) => crate::BluetoothDtmRole::Transmitter,
            BluetoothDtmFirstCommand::Receiver(_) => crate::BluetoothDtmRole::Receiver,
        };
        let observed = match &outcome {
            BluetoothDtmControllerPreparationOutcome::TransmitterFirst(_)
            | BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(_) => {
                crate::BluetoothDtmRole::Transmitter
            }
            BluetoothDtmControllerPreparationOutcome::ReceiverFirst(_)
            | BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(_) => {
                crate::BluetoothDtmRole::Receiver
            }
        };
        BluetoothDtmFirstInvariantFault {
            expected,
            observed,
            _command: command,
            _epoch: epoch,
            _outcome: outcome,
        }
    }

    /// Explicitly cancel every reversible state without relying on `Drop`.
    pub fn cancel(self) -> BluetoothDtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.cancel() {
                    Ok(task) => BluetoothDtmFirstRunnerCancel::NeedsColdTimeDrain { command, task },
                    Err(failure) => BluetoothDtmFirstRunnerCancel::Failed(
                        BluetoothDtmFirstRunnerFailure::ColdRecheck { command, failure },
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.cancel() {
                    Ok(epoch) => {
                        BluetoothDtmFirstRunnerCancel::NeedsWarmTimeDrain { command, epoch }
                    }
                    Err(failure) => BluetoothDtmFirstRunnerCancel::Failed(
                        BluetoothDtmFirstRunnerFailure::WarmRecheck { command, failure },
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::CurrentReady { command, current } => {
                BluetoothDtmFirstRunnerCancel::CleanEpoch {
                    command,
                    epoch: current.into_retained_epoch(),
                }
            }
            BluetoothDtmFirstRunnerPhase::Preparation { command, pending } => {
                Self::cancelled_preparation(command, pending.cancel())
            }
            BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                command,
                mut task,
                merged,
            } => match task.cancel_dtm_transmitter_first_item(merged) {
                Ok((graph, _, _)) => {
                    let idle = BluetoothDtmSessionIdle::new(graph);
                    let command = BluetoothDtmFirstCommand::Transmitter(command);
                    match task.restore_dtm_session_idle(idle) {
                        Ok(()) => BluetoothDtmFirstRunnerCancel::CleanTask { command, task },
                        Err(idle) => BluetoothDtmFirstRunnerCancel::RestoreRejected(
                            BluetoothDtmFirstIdleRestore::new(command, task, idle),
                        ),
                    }
                }
                Err(merged) => BluetoothDtmFirstRunnerCancel::CancellationRejected(
                    Self::from_phase(BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                        command,
                        task,
                        merged,
                    }),
                ),
            },
            BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                command,
                mut task,
                merged,
            } => match task.cancel_dtm_receiver_first_item(merged) {
                Ok(owner) => {
                    let (graph, _) = owner.into_memory_and_packet_count();
                    let idle = BluetoothDtmSessionIdle::new(graph);
                    let command = BluetoothDtmFirstCommand::Receiver(command);
                    match task.restore_dtm_session_idle(idle) {
                        Ok(()) => BluetoothDtmFirstRunnerCancel::CleanTask { command, task },
                        Err(idle) => BluetoothDtmFirstRunnerCancel::RestoreRejected(
                            BluetoothDtmFirstIdleRestore::new(command, task, idle),
                        ),
                    }
                }
                Err(merged) => BluetoothDtmFirstRunnerCancel::CancellationRejected(
                    Self::from_phase(BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                        command,
                        task,
                        merged,
                    }),
                ),
            },
            head @ (BluetoothDtmFirstRunnerPhase::TransmitterHead { .. }
            | BluetoothDtmFirstRunnerPhase::ReceiverHead { .. }) => {
                BluetoothDtmFirstRunnerCancel::HeadPublished(Self::from_phase(head))
            }
        }
    }
}
