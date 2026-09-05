//! Lossless response, retry, and terminal-owner observations.

use super::*;

/// Completion that returned the actor to its sole idle command owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerIdleCompletion {
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
}

/// Recoverable retry boundary while the complete owner remains in the actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothControllerRetry {
    FirstEvent,
    LegacyAdvertisingFirst,
    LegacyConnectableAdvertisingFirst,
    LegacyConnectableAdvertisingRecurring,
    PeripheralConnectionFirst,
    LegacyAdvertisingRecurring,
    LegacyAdvertisingDisableRestore,
    LegacyAdvertisingResetRestore,
    LegacyAdvertisingRecurringStopRestore,
    PassiveScanFirst,
    PassiveScanRecurring,
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
    /// Connectable advertising reached `RUN`, Success was published, and its
    /// HCI/radio axes remain active in the sole actor.
    LegacyConnectableAdvertisingActive,
    /// Recurrence failed closed while retaining the exact radio and HCI axes.
    LegacyConnectableAdvertisingRecurringFailStop(
        EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
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
    /// The first peripheral event reached scheduler `RUN`; its exact running
    /// owner and HCI-order axis remain inside the sole actor.
    PeripheralConnectionActive,
    /// Accepted-connection publication failed closed before the first peripheral `RUN`.
    PeripheralConnectionFirstFailStop(
        BluetoothLegacyConnectablePeripheralFirstHciFailStop<'runtime, S, CAPACITY>,
    ),
    /// Reset could not retire the accepted request and retains both affine owners.
    PeripheralConnectionResetFailStop(
        BluetoothLegacyConnectablePeripheralFirstHciResetFailStop<'runtime, S, CAPACITY>,
    ),
    /// Command-ready connectable radio completion failed closed.
    LegacyConnectableAdvertisingActiveFailStop(
        BluetoothLegacyConnectableAdvertisingHciActiveFailStop<'runtime, S, CAPACITY>,
    ),
    /// Response-pending connectable radio completion failed closed.
    LegacyConnectableAdvertisingPendingFailStop(
        BluetoothLegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
    ),
    /// Connectable completion failed while Disable or Reset was retained.
    LegacyConnectableAdvertisingStoppingFailStop(
        BluetoothLegacyConnectableAdvertisingStoppingFailStop<'runtime, S, CAPACITY>,
    ),
    /// Classified active command unexpectedly belonged to another endpoint.
    LegacyConnectableAdvertisingCommandEndpointMismatch(
        BluetoothLegacyConnectableAdvertisingCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// The accepted passive scanner Enable reached `RUN` and success was published.
    PassiveScanningActive,
    /// One received PDU could not be represented by the legacy scanner parser and was ignored.
    PassiveScanMalformedPdu(
        open_esp_radio_bluetooth_ll::scanning::LegacyAdvertisingReportParseError,
    ),
    /// A parsed report unexpectedly could not be represented by the standard HCI event.
    PassiveScanReportEncodingFault(
        open_esp_radio_bluetooth_hci::LeLegacyAdvertisingReportEventError,
    ),
    /// Classified scanner command unexpectedly belonged to another HCI endpoint.
    PassiveScanCommandEndpointMismatch(
        BluetoothPassiveScanHciCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// Classified in-flight scanner command unexpectedly belonged to another endpoint.
    PassiveScanActiveCommandEndpointMismatch(
        BluetoothPassiveScanHciActiveCommandMismatch<'runtime, 'epoch, S, CAPACITY>,
    ),
    /// No installed role owns this scheduler list; its exact owner is quarantined in the actor.
    UnownedFinishedList(BluetoothSchedulerHardwareListIndex),
    /// A non-retryable initial transition failed before scheduler `RUN`.
    ///
    /// Safe lower retries remain stored in the actor and are reported through
    /// [`EmbassyBluetoothControllerRetry::FirstEvent`]. The only automatic
    /// failure response is the separate, typed CleanTask edge after preparation
    /// cleanup has proved the graph idle again.
    FirstEventFailed(BluetoothDtmFirstRunnerFailure<'runtime, S, CAPACITY>),
    /// Connectable preparation or atomic publication failed closed.
    LegacyConnectableAdvertisingFailStop(
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop<'runtime, S, CAPACITY>,
    ),
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
    /// The passive scanner hardware graph failed closed with every owner retained.
    PassiveScanFault(BluetoothPassiveScanHciActiveFault<'runtime, S, CAPACITY>),
    /// The scanner faulted while an ordered command response remained pending.
    PassiveScanPendingFault(BluetoothPassiveScanHciActivePendingFault<'runtime, S, CAPACITY>),
    /// The scanner faulted while Disable or Reset waited for quiescence.
    PassiveScanStoppingFault(BluetoothPassiveScanHciStoppingFault<'runtime, S, CAPACITY>),
    /// Recurring passive-scan preparation failed closed with every owner retained.
    PassiveScanRecurringFault(BluetoothPassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
    /// The non-repeating advertising event identity space was exhausted.
    LegacyAdvertisingSequenceExhausted(BluetoothSchedulerHardwareListIndex),
}
