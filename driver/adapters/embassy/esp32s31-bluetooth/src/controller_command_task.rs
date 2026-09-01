//! Sole Embassy owner for the ESP32-S31 LE Controller command lifecycle.
//!
//! This actor composes the chip-owned idle command transaction, the bounded
//! DTM/advertising first-event runners and the active-session actors. It does
//! not interpret HCI commands or reproduce radio policy. Every awaited future borrows an owner
//! retained in the actor's affine state slot; cancellation therefore leaves
//! the exact lower transaction available to the next `run` call.

#![forbid(unsafe_code)]

use crate::EmbassyBluetoothDtmSessionRetry;

#[cfg(target_arch = "riscv32")]
use embassy_futures::select::{Either, select};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerIdleCommandIntake, BluetoothControllerIdleCommandMismatch,
    BluetoothControllerIdleCommandRoute, BluetoothControllerIdleCommandTask,
    BluetoothControllerIdleResetBarrier, BluetoothControllerIdleResetCompletion,
    BluetoothControllerIdleResponsePending, BluetoothControllerIdleResponsePublication,
    BluetoothControllerSchedulerCurrentError, BluetoothDtmActiveCommandMismatch,
    BluetoothDtmActiveSessionFault, BluetoothDtmFirstPreparationCleanup,
    BluetoothDtmFirstPreparationCleanupStep, BluetoothDtmFirstPreparationCompletion,
    BluetoothDtmFirstPreparationFailStop, BluetoothDtmFirstRunnerFailure,
    BluetoothDtmFirstRunnerRetry, BluetoothDtmOrderReady, BluetoothDtmResetCompletionReady,
    BluetoothDtmResetCompletionStart, BluetoothDtmResetResponsePending,
    BluetoothDtmResetResponsePublication, BluetoothDtmResetRestoreFailure,
    BluetoothDtmResetRestoreStep, BluetoothDtmResetStoppingFault, BluetoothDtmResetStoppingRunner,
    BluetoothDtmResetStoppingStep, BluetoothDtmResetStoppingWait, BluetoothDtmResponsePending,
    BluetoothLegacyAdvertisingActiveCommandIntake, BluetoothLegacyAdvertisingActiveCommandMismatch,
    BluetoothLegacyAdvertisingActiveCommandRoute, BluetoothLegacyAdvertisingActiveFault,
    BluetoothLegacyAdvertisingActivePendingFault, BluetoothLegacyAdvertisingActivePendingRadioStep,
    BluetoothLegacyAdvertisingActiveResponsePending,
    BluetoothLegacyAdvertisingActiveResponsePublication, BluetoothLegacyAdvertisingActiveSession,
    BluetoothLegacyAdvertisingActiveWait, BluetoothLegacyAdvertisingCpuOwnedCommandIntake,
    BluetoothLegacyAdvertisingCpuOwnedCommandMismatch,
    BluetoothLegacyAdvertisingCpuOwnedCommandRoute,
    BluetoothLegacyAdvertisingCpuOwnedResponsePending,
    BluetoothLegacyAdvertisingCpuOwnedResponsePublication,
    BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingDisableResponsePublication, BluetoothLegacyAdvertisingDisableRestore,
    BluetoothLegacyAdvertisingDisableRestoreStep, BluetoothLegacyAdvertisingEventCpuOwned,
    BluetoothLegacyAdvertisingFirstRunnerFailure, BluetoothLegacyAdvertisingFirstRunnerRetry,
    BluetoothLegacyAdvertisingRecurringCommandIntake,
    BluetoothLegacyAdvertisingRecurringCommandMismatch,
    BluetoothLegacyAdvertisingRecurringCommandRoute, BluetoothLegacyAdvertisingRecurringFault,
    BluetoothLegacyAdvertisingRecurringOrderProgress,
    BluetoothLegacyAdvertisingRecurringOrderState,
    BluetoothLegacyAdvertisingRecurringResponsePublication,
    BluetoothLegacyAdvertisingRecurringRetry, BluetoothLegacyAdvertisingRecurringRunner,
    BluetoothLegacyAdvertisingRecurringStart, BluetoothLegacyAdvertisingRecurringStopBegin,
    BluetoothLegacyAdvertisingRecurringStopFault, BluetoothLegacyAdvertisingRecurringStopRestore,
    BluetoothLegacyAdvertisingRecurringStopRestoreStep, BluetoothLegacyAdvertisingResetCompletion,
    BluetoothLegacyAdvertisingResetCompletionReady, BluetoothLegacyAdvertisingResetResponsePending,
    BluetoothLegacyAdvertisingResetResponsePublication, BluetoothLegacyAdvertisingResetRestore,
    BluetoothLegacyAdvertisingResetRestoreStep, BluetoothLegacyAdvertisingResponsePendingSession,
    BluetoothLegacyAdvertisingResponsePublication, BluetoothLegacyAdvertisingStopping,
    BluetoothLegacyAdvertisingStoppingFault, BluetoothLegacyAdvertisingStoppingStep,
    BluetoothPassiveScanHciActiveSession, BluetoothPassiveScanHciFirstRunnerFailure,
    BluetoothPassiveScanHciResponsePendingSession, BluetoothPassiveScanHciResponsePublication,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::{
    EmbassyBluetoothDtmControllerTimeRecheck, EmbassyBluetoothDtmControllerTimeRecheckStatus,
    EmbassyBluetoothDtmFirstControllerTimeWait, EmbassyBluetoothDtmFirstDrive,
    EmbassyBluetoothDtmFirstResume, EmbassyBluetoothDtmSessionBoundary,
    EmbassyBluetoothDtmSessionTask, EmbassyBluetoothLegacyAdvertisingActiveDrive,
    EmbassyBluetoothLegacyAdvertisingDelaySource,
    EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait,
    EmbassyBluetoothLegacyAdvertisingFirstDrive, EmbassyBluetoothLegacyAdvertisingFirstResume,
    EmbassyBluetoothLegacyAdvertisingRecurringDrive,
    EmbassyBluetoothPassiveScanFirstControllerTimeWait, EmbassyBluetoothPassiveScanFirstDrive,
    EmbassyBluetoothPassiveScanFirstResume, EmbassyBluetoothRuntimeWakers, drive_dtm_first_ready,
    drive_legacy_advertising_active_ready, drive_legacy_advertising_first_ready,
    drive_legacy_advertising_recurring_ready, drive_passive_scan_first_ready,
};

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
enum ControllerCommandStimulus {
    Retain,
    IdleReset,
    IdleResponse,
    FirstEvent,
    LegacyAdvertisingFirst,
    LegacyAdvertisingResponse,
    LegacyAdvertisingActive,
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
        Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, LegacyAdvertisingActive,
        LegacyAdvertisingFirst, LegacyAdvertisingResponse, PassiveScanActive, PassiveScanFirst,
        PassiveScanResponse, ResetCompletion, ResetResponse, ResetRestore, ResetStopping,
        UnownedFinishedList,
    };
    use EmbassyBluetoothControllerCommandPhase::{
        Active as ActivePhase, FirstEvent as FirstEventPhase, Idle, IdleReset as IdleResetPhase,
        IdleResponse as IdleResponsePhase, LegacyAdvertisingActive as LegacyAdvertisingActivePhase,
        LegacyAdvertisingFirst as LegacyAdvertisingFirstPhase,
        LegacyAdvertisingResponse as LegacyAdvertisingResponsePhase,
        PassiveScanActive as PassiveScanActivePhase, PassiveScanFirst as PassiveScanFirstPhase,
        PassiveScanResponse as PassiveScanResponsePhase, ResetCompletion as ResetCompletionPhase,
        ResetResponse as ResetResponsePhase, ResetRestore as ResetRestorePhase,
        ResetStopping as ResetStoppingPhase, UnownedFinishedList as UnownedFinishedListPhase,
    };

    match (phase, stimulus) {
        (_, ControllerCommandStimulus::Retain) => Retain,
        (Idle, IdleReset) => Advance(IdleResetPhase),
        (Idle, IdleResponse) => Advance(IdleResponsePhase),
        (Idle, FirstEvent) => Advance(FirstEventPhase),
        (Idle, LegacyAdvertisingFirst) => Advance(LegacyAdvertisingFirstPhase),
        (Idle, PassiveScanFirst) => Advance(PassiveScanFirstPhase),
        (LegacyAdvertisingFirstPhase, LegacyAdvertisingResponse) => {
            Advance(LegacyAdvertisingResponsePhase)
        }
        (LegacyAdvertisingResponsePhase, LegacyAdvertisingActive) => {
            Advance(LegacyAdvertisingActivePhase)
        }
        (PassiveScanFirstPhase, PassiveScanResponse) => Advance(PassiveScanResponsePhase),
        (PassiveScanResponsePhase, PassiveScanActive) => Advance(PassiveScanActivePhase),
        (PassiveScanFirstPhase, IdleResponse) => Advance(IdleResponsePhase),
        (LegacyAdvertisingFirstPhase, IdleResponse) => Advance(IdleResponsePhase),
        (Idle | FirstEventPhase, Active) => Advance(ActivePhase),
        (IdleResetPhase, IdleResponse) => Advance(IdleResponsePhase),
        (FirstEventPhase, IdleResponse) => Advance(IdleResponsePhase),
        (ActivePhase, ResetStopping) => Advance(ResetStoppingPhase),
        (ActivePhase | ResetStoppingPhase, UnownedFinishedList) => {
            Advance(UnownedFinishedListPhase)
        }
        (LegacyAdvertisingActivePhase, UnownedFinishedList) => Advance(UnownedFinishedListPhase),
        (UnownedFinishedListPhase, UnownedFinishedList) => Retain,
        (ResetStoppingPhase, ResetRestore) => Advance(ResetRestorePhase),
        (ResetStoppingPhase | ResetRestorePhase, ResetCompletion) => Advance(ResetCompletionPhase),
        (ResetCompletionPhase, ResetResponse) => Advance(ResetResponsePhase),
        (
            IdleResponsePhase
            | LegacyAdvertisingActivePhase
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
enum EmbassyBluetoothUnownedFinishedListOwner<'runtime, S, const CAPACITY: usize>
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
            Self::LegacyAdvertising { observed, .. }
            | Self::LegacyAdvertisingPending { observed, .. }
            | Self::LegacyAdvertisingStopping { observed, .. } => observed.index(),
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
    LegacyAdvertisingDisableResponse(
        BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingResetRestore(BluetoothLegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
    LegacyAdvertisingResetCompletion(
        BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingResetResponse(
        BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingRecurring(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    LegacyAdvertisingRecurringRetry(
        BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>,
    ),
    LegacyAdvertisingRecurringStopRestore(
        BluetoothLegacyAdvertisingRecurringStopRestore<'runtime, S, CAPACITY>,
    ),
    PassiveScanFirst(EmbassyBluetoothPassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
    PassiveScanRetry(BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
    PassiveScanResponse(BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    PassiveScanActive(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
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
            | Self::LegacyAdvertisingDisableResponse(_)
            | Self::LegacyAdvertisingResetRestore(_)
            | Self::LegacyAdvertisingResetCompletion(_)
            | Self::LegacyAdvertisingResetResponse(_)
            | Self::LegacyAdvertisingRecurring(_)
            | Self::LegacyAdvertisingRecurringRetry(_)
            | Self::LegacyAdvertisingRecurringStopRestore(_) => {
                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive
            }
            Self::PassiveScanFirst(_) | Self::PassiveScanRetry(_) => {
                EmbassyBluetoothControllerCommandPhase::PassiveScanFirst
            }
            Self::PassiveScanResponse(_) => {
                EmbassyBluetoothControllerCommandPhase::PassiveScanResponse
            }
            Self::PassiveScanActive(_) => EmbassyBluetoothControllerCommandPhase::PassiveScanActive,
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
    LegacyAdvertisingStartRejected,
    LegacyAdvertisingDisable,
    PassiveScanStartRejected,
    TestEnd,
    Reset,
}

/// Recoverable retry boundary while the complete owner remains in the actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerRetry {
    FirstEvent,
    LegacyAdvertisingFirst,
    LegacyAdvertisingRecurring,
    LegacyAdvertisingDisableRestore,
    LegacyAdvertisingResetRestore,
    LegacyAdvertisingRecurringStopRestore,
    PassiveScanFirst,
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
    /// The accepted advertising Enable reached scheduler `RUN` and its response was published.
    LegacyAdvertisingActive(BluetoothSchedulerHardwareListIndex),
    /// The accepted passive scanner Enable reached `RUN` and success was published.
    PassiveScanningActive,
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
    IdleCommandEndpointMismatch(
        BluetoothControllerIdleCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Active intake found an impossible post-classification endpoint mismatch.
    ActiveCommandEndpointMismatch(BluetoothDtmActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>),
    /// CPU-boundary advertising intake found an impossible endpoint mismatch.
    LegacyAdvertisingCommandEndpointMismatch(
        BluetoothLegacyAdvertisingCpuOwnedCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// In-flight advertising intake found an impossible endpoint mismatch.
    LegacyAdvertisingActiveCommandEndpointMismatch(
        BluetoothLegacyAdvertisingActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Recurring preparation intake found an impossible post-classification mismatch.
    LegacyAdvertisingRecurringCommandEndpointMismatch(
        BluetoothLegacyAdvertisingRecurringCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
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
    /// Active advertising failed closed while retaining its complete graph and HCI order.
    LegacyAdvertisingFault(BluetoothLegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
    /// Active advertising faulted while an ordered response was pending.
    LegacyAdvertisingPendingFault(
        BluetoothLegacyAdvertisingActivePendingFault<'runtime, S, CAPACITY>,
    ),
    /// Active advertising faulted while Disable or Reset was retained.
    LegacyAdvertisingStoppingFault(BluetoothLegacyAdvertisingStoppingFault<'runtime, S, CAPACITY>),
    /// Cancelling a pre-HEAD successor could not drain its abandoned time request.
    LegacyAdvertisingRecurringStopFault(
        BluetoothLegacyAdvertisingRecurringStopFault<'runtime, S, CAPACITY>,
    ),
    /// Recurring advertising failed closed while retaining every owner.
    LegacyAdvertisingRecurringFault(
        BluetoothLegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>,
    ),
    /// The non-repeating advertising event identity space was exhausted.
    LegacyAdvertisingSequenceExhausted(BluetoothSchedulerHardwareListIndex),
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

/// Sole executor-side owner of the idle and active radio lifecycles.
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

    fn store_legacy_advertising_failure<'epoch, 'packet>(
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

    fn store_legacy_advertising_drive<'epoch, 'packet>(
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

    fn store_passive_scan_failure<'epoch, 'packet>(
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

    fn store_passive_scan_drive<'epoch, 'packet>(
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

    fn store_legacy_advertising_recurring_drive<'epoch, 'packet>(
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
                                BluetoothControllerIdleCommandRoute::StartLegacyAdvertising(runner) => {
                                    if let Some(boundary) = self.store_legacy_advertising_drive(
                                        EmbassyBluetoothControllerCommandPhase::Idle,
                                        drive_legacy_advertising_first_ready(runner),
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
                    let EmbassyBluetoothControllerCommandState::PassiveScanActive(_active) =
                        self.owner.current()
                    else {
                        unreachable!("the selected active scanner did not change")
                    };
                    return self.retain_boundary(
                        EmbassyBluetoothControllerCommandBoundary::PassiveScanningActive,
                    );
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
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
                                    pending,
                                ),
                            ),
                            BluetoothLegacyAdvertisingRecurringStopRestoreStep::ResetCompletion(
                                ready,
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion(
                                    ready,
                                ),
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
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
                                    pending,
                                ),
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
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected advertising Disable response did not change")
                        };
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited advertising Disable response did not change")
                        };
                        match pending.try_publish(controller) {
                            BluetoothLegacyAdvertisingDisableResponsePublication::Completed(idle) => {
                                self.store_transition(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::IdleRestored,
                                    EmbassyBluetoothControllerCommandState::Idle(idle),
                                );
                                return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                    EmbassyBluetoothControllerIdleCompletion::LegacyAdvertisingDisable,
                                );
                            }
                            BluetoothLegacyAdvertisingDisableResponsePublication::Pending(
                                pending,
                            ) => self.store_retained_state(
                                EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
                                    pending,
                                ),
                            ),
                            BluetoothLegacyAdvertisingDisableResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
                                        pending,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingDisableResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingDisableResponse(
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
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion(
                                        ready,
                                    ),
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
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion(
                            ready,
                        ) = self.owner.take()
                        else {
                            unreachable!("the selected advertising Reset completion did not change")
                        };
                        match ready.complete(controller) {
                            BluetoothLegacyAdvertisingResetCompletion::ResponsePending(pending) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(
                                        pending,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingResetCompletion::EndpointMismatch(ready) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetCompletion(
                                        ready,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                        }
                        continue;
                    }

                    if matches!(
                        self.owner.current(),
                        EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(_)
                    ) {
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected advertising Reset response did not change")
                        };
                        if pending.wait_response_capacity(controller).await.is_err() {
                            return self.retain_boundary(
                                EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                            );
                        }
                        let EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited advertising Reset response did not change")
                        };
                        match pending.try_publish(controller) {
                            BluetoothLegacyAdvertisingResetResponsePublication::Completed(idle) => {
                                self.store_transition(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    ControllerCommandStimulus::IdleRestored,
                                    EmbassyBluetoothControllerCommandState::Idle(idle),
                                );
                                return EmbassyBluetoothControllerCommandBoundary::IdleRestored(
                                    EmbassyBluetoothControllerIdleCompletion::Reset,
                                );
                            }
                            BluetoothLegacyAdvertisingResetResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(
                                        pending,
                                    ),
                                )
                            }
                            BluetoothLegacyAdvertisingResetResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(
                                        pending,
                                    ),
                                );
                                return self.retain_boundary(
                                    EmbassyBluetoothControllerCommandBoundary::EndpointMismatch,
                                );
                            }
                            BluetoothLegacyAdvertisingResetResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    EmbassyBluetoothControllerCommandPhase::LegacyAdvertisingActive,
                                    EmbassyBluetoothControllerCommandState::LegacyAdvertisingResetResponse(
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
            Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, LegacyAdvertisingActive,
            LegacyAdvertisingFirst, LegacyAdvertisingResponse, PassiveScanActive, PassiveScanFirst,
            PassiveScanResponse, ResetCompletion, ResetResponse, ResetRestore, ResetStopping,
            UnownedFinishedList,
        };
        use EmbassyBluetoothControllerCommandPhase::{
            Active as ActivePhase, FirstEvent as FirstEventPhase, Idle,
            IdleReset as IdleResetPhase, IdleResponse as IdleResponsePhase,
            LegacyAdvertisingActive as LegacyAdvertisingActivePhase,
            LegacyAdvertisingFirst as LegacyAdvertisingFirstPhase,
            LegacyAdvertisingResponse as LegacyAdvertisingResponsePhase,
            PassiveScanActive as PassiveScanActivePhase, PassiveScanFirst as PassiveScanFirstPhase,
            PassiveScanResponse as PassiveScanResponsePhase,
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
            reduce_controller_command_transition(Idle, LegacyAdvertisingFirst),
            Advance(LegacyAdvertisingFirstPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(
                LegacyAdvertisingFirstPhase,
                LegacyAdvertisingResponse,
            ),
            Advance(LegacyAdvertisingResponsePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(
                LegacyAdvertisingResponsePhase,
                LegacyAdvertisingActive,
            ),
            Advance(LegacyAdvertisingActivePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(LegacyAdvertisingFirstPhase, IdleResponse),
            Advance(IdleResponsePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(Idle, PassiveScanFirst),
            Advance(PassiveScanFirstPhase)
        );
        assert_eq!(
            reduce_controller_command_transition(PassiveScanFirstPhase, PassiveScanResponse),
            Advance(PassiveScanResponsePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(PassiveScanResponsePhase, PassiveScanActive),
            Advance(PassiveScanActivePhase)
        );
        assert_eq!(
            reduce_controller_command_transition(PassiveScanActivePhase, IdleRestored),
            Advance(Idle)
        );
        assert_eq!(
            reduce_controller_command_transition(ActivePhase, IdleRestored),
            Advance(Idle)
        );
        assert_eq!(
            reduce_controller_command_transition(LegacyAdvertisingActivePhase, IdleRestored),
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
            reduce_controller_command_transition(LegacyAdvertisingActivePhase, UnownedFinishedList,),
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
