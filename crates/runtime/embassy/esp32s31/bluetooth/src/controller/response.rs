//! Lossless response, retry, and terminal-owner observations.

use super::*;

/// Completion that returned the actor to its sole idle command owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerIdleCompletion {
    ImmediateResponse,
    DtmStartRejected,
    LegacyAdvertisingStartRejected,
    LegacyConnectableAdvertisingStartRejected,
    LegacyAdvertisingDisable,
    LegacyConnectableAdvertisingDisable,
    PassiveScanStartRejected,
    PassiveScanDisable,
    TestEnd,
    Reset,
    /// Peer requested termination; radio resources and next-command authority are idle.
    PeripheralDisconnected {
        reason: u8,
    },
}

/// Recoverable retry boundary while the complete owner remains in the actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRetry {
    FirstEvent,
    LegacyAdvertisingFirst,
    LegacyConnectableAdvertisingFirst,
    #[cfg(target_arch = "riscv32")]
    LegacyConnectableAdvertisingRecurring(
        oer_esp32s31_bluetooth::le::advertising::LegacyConnectableAdvertisingRecurringRetryCause<()>,
    ),
    PeripheralConnectionFirst,
    LegacyAdvertisingRecurring,
    LegacyAdvertisingDisableRestore,
    LegacyAdvertisingResetRestore,
    LegacyAdvertisingRecurringStopRestore,
    PassiveScanFirst,
    PassiveScanRecurring,
    Active(DtmSessionRetry),
    ResetStopping,
    ResetRestore,
}

/// One lossless externally meaningful boundary from the sole Controller actor.
#[cfg(target_arch = "riscv32")]
#[must_use = "handle the observation or retain the exact terminal lower owner"]
pub enum ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// A command lifecycle completed and the actor again owns the idle command token.
    IdleRestored(ControllerIdleCompletion),
    /// A non-command Host frame remains bound to its source HCI epoch and buffer.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
    /// The supplied endpoint does not match the retained transaction.
    EndpointMismatch,
    /// HCI failed while the actor retained the complete transaction.
    HciFault(HciChannelError),
    /// A lower owner remained intact and requires an explicit retry.
    Retryable(ControllerRetry),
    /// The absolute Controller-time schedule is exhausted; the actor retains its owner.
    ControllerTimeExhausted,
    /// The accepted advertising Enable reached scheduler `RUN` and its response was published.
    LegacyAdvertisingActive(BluetoothSchedulerHardwareListIndex),
    /// Connectable advertising reached `RUN`, Success was published, and its
    /// HCI/radio axes remain active in the sole actor.
    LegacyConnectableAdvertisingActive,
    /// Recurrence failed closed while retaining the exact radio and HCI axes.
    LegacyConnectableAdvertisingRecurringFailStop(
        AdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    ),
    /// Sequence-pending recurrence classification belonged to another endpoint.
    LegacyConnectableAdvertisingRecurringSequencePendingCommandEndpointMismatch(
        ConnectableRecurringSequencePendingMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Graph-prepared recurrence classification belonged to another endpoint.
    LegacyConnectableAdvertisingRecurringGraphPreparedCommandEndpointMismatch(
        ConnectableRecurringGraphPreparedMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Candidate recurrence classification belonged to another endpoint.
    LegacyConnectableAdvertisingRecurringCandidateCommandEndpointMismatch(
        ConnectableRecurringCandidateMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Prepared recurrence classification belonged to another endpoint.
    LegacyConnectableAdvertisingRecurringPreparedCommandEndpointMismatch(
        ConnectableRecurringPreparedMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Merged recurrence classification belonged to another endpoint.
    LegacyConnectableAdvertisingRecurringMergedCommandEndpointMismatch(
        ConnectableRecurringMergedMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// A first or successor peripheral event reached scheduler `RUN`; its exact running
    /// owner and HCI-order axis remain inside the sole actor.
    PeripheralConnectionActive,
    /// Completion or recurrence failed closed with radio and HCI owners retained.
    PeripheralConnectionActiveFailStop(PeripheralConnectionActiveFault<'runtime, S, CAPACITY>),
    /// Accepted-connection publication failed closed before the first peripheral `RUN`.
    PeripheralConnectionFirstFailStop(
        LegacyConnectablePeripheralFirstHciFailStop<'runtime, S, CAPACITY>,
    ),
    /// Reset could not retire the accepted request and retains both affine owners.
    PeripheralConnectionResetFailStop(
        LegacyConnectablePeripheralFirstHciResetFailStop<'runtime, S, CAPACITY>,
    ),
    /// Command-ready connectable radio completion failed closed.
    LegacyConnectableAdvertisingActiveFailStop(
        LegacyConnectableAdvertisingHciActiveFailStop<'runtime, S, CAPACITY>,
    ),
    /// Response-pending connectable radio completion failed closed.
    LegacyConnectableAdvertisingPendingFailStop(
        LegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
    ),
    /// Connectable completion failed while Disable or Reset was retained.
    LegacyConnectableAdvertisingStoppingFailStop(
        LegacyConnectableAdvertisingStoppingFailStop<'runtime, S, CAPACITY>,
    ),
    /// Classified active command unexpectedly belonged to another endpoint.
    LegacyConnectableAdvertisingCommandEndpointMismatch(
        LegacyConnectableAdvertisingCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// The accepted passive scanner Enable reached `RUN` and success was published.
    PassiveScanningActive,
    /// One received PDU could not be represented by the legacy scanner parser and was ignored.
    PassiveScanMalformedPdu(oer_bluetooth_ll::scanning::LegacyAdvertisingReportParseError),
    /// A parsed report unexpectedly could not be represented by the standard HCI event.
    PassiveScanReportEncodingFault(oer_bluetooth_hci::LeLegacyAdvertisingReportEventError),
    /// Classified scanner command unexpectedly belonged to another HCI endpoint.
    PassiveScanCommandEndpointMismatch(
        PassiveScanHciCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Classified in-flight scanner command unexpectedly belonged to another endpoint.
    PassiveScanActiveCommandEndpointMismatch(
        PassiveScanHciActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// No installed role owns this scheduler list; its exact owner is quarantined in the actor.
    UnownedFinishedList(BluetoothSchedulerHardwareListIndex),
    /// A non-retryable initial transition failed before scheduler `RUN`.
    ///
    /// Safe lower retries remain stored in the actor and are reported through
    /// [`ControllerRetry::FirstEvent`]. The only automatic
    /// failure response is the separate, typed CleanTask edge after preparation
    /// cleanup has proved the graph idle again.
    FirstEventFailed(DtmFirstRunnerFailure<'runtime, S, CAPACITY>),
    /// Connectable preparation or atomic publication failed closed.
    LegacyConnectableAdvertisingFailStop(
        LegacyConnectableAdvertisingFirstRunnerFailStop<'runtime, S, CAPACITY>,
    ),
    /// Preparation cleanup faulted before it could prove a clean idle task.
    FirstPreparationCleanupFault {
        cleanup: DtmFirstPreparationCleanup<'runtime, S, CAPACITY>,
        error: ControllerSchedulerCurrentError,
    },
    /// The runtime rejected the exact graph during preparation-failure restore.
    FirstPreparationRestoreRejected(DtmFirstPreparationCleanup<'runtime, S, CAPACITY>),
    /// Chip policy classified the restored failure as poisoned and forbade reuse.
    FirstPreparationFailStop(DtmFirstPreparationFailStop<'runtime, S, CAPACITY>),
    /// Idle intake found an impossible post-classification endpoint mismatch.
    IdleCommandEndpointMismatch(ControllerIdleCommandMismatch<'runtime, 'epoch, S, CAPACITY>),
    /// Active intake found an impossible post-classification endpoint mismatch.
    ActiveCommandEndpointMismatch(DtmActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>),
    /// CPU-boundary advertising intake found an impossible endpoint mismatch.
    LegacyAdvertisingCommandEndpointMismatch(
        LegacyAdvertisingCpuOwnedCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// In-flight advertising intake found an impossible endpoint mismatch.
    LegacyAdvertisingActiveCommandEndpointMismatch(
        LegacyAdvertisingActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Recurring preparation intake found an impossible post-classification mismatch.
    LegacyAdvertisingRecurringCommandEndpointMismatch(
        LegacyAdvertisingRecurringCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Active radio failed while its response axis was still pending.
    PendingRadioFault(DtmActiveSessionFault<'runtime, S, CAPACITY, DtmResponsePending<'runtime>>),
    /// Active radio failed after command order became ready.
    CommandReadyRadioFault(DtmActiveSessionFault<'runtime, S, CAPACITY, DtmOrderReady<'runtime>>),
    /// Test End quiescence failed closed with its exact transaction.
    TestEndStoppingFault(oer_esp32s31_bluetooth::le::dtm::DtmStoppingFault<'runtime, S, CAPACITY>),
    /// Reset quiescence failed closed with its exact transaction.
    ResetStoppingFault(DtmResetStoppingFault<'runtime, S, CAPACITY>),
    /// Active advertising failed closed while retaining its complete graph and HCI order.
    LegacyAdvertisingFault(LegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
    /// Active advertising faulted while an ordered response was pending.
    LegacyAdvertisingPendingFault(LegacyAdvertisingActivePendingFault<'runtime, S, CAPACITY>),
    /// Active advertising faulted while Disable or Reset was retained.
    LegacyAdvertisingStoppingFault(LegacyAdvertisingStoppingFault<'runtime, S, CAPACITY>),
    /// Cancelling a pre-HEAD successor could not drain its abandoned time request.
    LegacyAdvertisingRecurringStopFault(LegacyAdvertisingRecurringStopFault<'runtime, S, CAPACITY>),
    /// Recurring advertising failed closed while retaining every owner.
    LegacyAdvertisingRecurringFault(LegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
    /// The passive scanner hardware graph failed closed with every owner retained.
    PassiveScanFault(PassiveScanHciActiveFault<'runtime, S, CAPACITY>),
    /// The scanner faulted while an ordered command response remained pending.
    PassiveScanPendingFault(PassiveScanHciActivePendingFault<'runtime, S, CAPACITY>),
    /// The scanner faulted while Disable or Reset waited for quiescence.
    PassiveScanStoppingFault(PassiveScanHciStoppingFault<'runtime, S, CAPACITY>),
    /// Recurring passive-scan preparation failed closed with every owner retained.
    PassiveScanRecurringFault(PassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
    /// The non-repeating advertising event identity space was exhausted.
    LegacyAdvertisingSequenceExhausted(BluetoothSchedulerHardwareListIndex),
}
